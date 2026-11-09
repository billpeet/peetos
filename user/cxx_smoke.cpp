// C++ runtime smoke test for PeetOS: libstdc++, libsupc++ and libgcc_s on
// musl, dynamically linked. Prints "[cxx] <check> ok" per check, then
// "[cxx] pass" or "[cxx] fail (N)". A static destructor prints a final marker
// after main returns.

#include "cxx/peetcxx.h"

#include <dlfcn.h>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <condition_variable>
#include <exception>
#include <filesystem>
#include <format>
#include <functional>
#include <future>
#include <iomanip>
#include <iostream>
#include <map>
#include <memory>
#include <mutex>
#include <new>
#include <numeric>
#include <sstream>
#include <string>
#include <thread>
#include <typeinfo>
#include <unordered_map>
#include <vector>

namespace {

int g_failures = 0;

void check(bool ok, const std::string &name) {
    if (ok) {
        std::cout << "[cxx] " << name << " ok" << std::endl;
    } else {
        std::cout << "[cxx] FAIL " << name << std::endl;
        ++g_failures;
    }
}

// --- static construction / destruction ------------------------------------

int g_exe_ctor_ran = 0;

struct ExeStatic {
    ExeStatic() { g_exe_ctor_ran = 1; }
    ~ExeStatic() {
        // Runs from __cxa_atexit after main returns.
        std::cout << "[cxx] static destructor ran" << std::endl;
    }
};

ExeStatic g_exe_static;

void test_static_init() {
    check(g_exe_ctor_ran == 1, "executable static constructor ran before main");
    check(peetcxx::library_ctor_count() == 1, "shared library static constructor ran once");
}

// --- exceptions -----------------------------------------------------------

int g_raii_destroyed = 0;

struct Raii {
    ~Raii() { ++g_raii_destroyed; }
};

[[noreturn]] void throw_deep(int depth) {
    Raii guard;
    if (depth == 0) {
        throw std::runtime_error("deep");
    }
    throw_deep(depth - 1);
}

void test_exceptions() {
    bool caught = false;
    try {
        throw std::runtime_error("local");
    } catch (const std::exception &e) {
        caught = std::string(e.what()) == "local";
    }
    check(caught, "throw and catch by base class");

    g_raii_destroyed = 0;
    caught = false;
    try {
        throw_deep(5);
    } catch (const std::runtime_error &) {
        caught = true;
    }
    check(caught && g_raii_destroyed == 6, "unwinding runs destructors in every frame");

    int rethrown = 0;
    try {
        try {
            throw 7;
        } catch (...) {
            throw;
        }
    } catch (int v) {
        rethrown = v;
    }
    check(rethrown == 7, "catch-all rethrow");

    std::exception_ptr saved;
    try {
        throw std::out_of_range("saved");
    } catch (...) {
        saved = std::current_exception();
    }
    caught = false;
    try {
        std::rethrow_exception(saved);
    } catch (const std::out_of_range &e) {
        caught = std::string(e.what()) == "saved";
    }
    check(caught, "exception_ptr rethrow");

    int code = 0;
    try {
        peetcxx::throw_library_error(42);
    } catch (const peetcxx::LibraryError &e) {
        code = e.code();
    }
    check(code == 42, "exception thrown in a shared library caught by its type");

    caught = false;
    try {
        std::vector<int> v(3);
        (void)v.at(10);
    } catch (const std::out_of_range &) {
        caught = true;
    }
    check(caught, "libstdc++ throws std::out_of_range");

    caught = false;
    try {
        // Larger than the 4 GiB per-process mapping budget, so the
        // allocation fails and operator new throws.
        static char *volatile sink;
        sink = new char[size_t(1) << 40];
        delete[] sink;
    } catch (const std::bad_alloc &) {
        caught = true;
    }
    check(caught, "failed allocation throws std::bad_alloc");
}

// --- RTTI and virtual dispatch -------------------------------------------

struct Base {
    virtual ~Base() = default;
};
struct Derived : Base {};
struct Other : Base {};

void test_rtti() {
    std::unique_ptr<Base> d = std::make_unique<Derived>();
    check(dynamic_cast<Derived *>(d.get()) != nullptr && dynamic_cast<Other *>(d.get()) == nullptr,
          "dynamic_cast");
    check(typeid(*d) == typeid(Derived) && typeid(*d) != typeid(Other), "typeid comparison");

    std::unique_ptr<peetcxx::Shape> sq(peetcxx::make_square(3.0));
    check(std::string(sq->name()) == "square" && sq->area() == 9.0,
          "virtual call into a shared library object");
    check(std::string(typeid(*sq).name()).find("Square") != std::string::npos,
          "typeid of a shared library object");
}

// --- standard library -----------------------------------------------------

void test_stl() {
    std::vector<int> v{5, 3, 9, 1, 7};
    std::sort(v.begin(), v.end());
    check(v == std::vector<int>({1, 3, 5, 7, 9}) && std::accumulate(v.begin(), v.end(), 0) == 25,
          "vector, sort, accumulate");

    std::map<std::string, int> m{{"b", 2}, {"a", 1}, {"c", 3}};
    std::string keys;
    for (const auto &kv : m) keys += kv.first;
    std::unordered_map<int, std::string> um;
    for (int i = 0; i < 1000; ++i) um[i] = std::to_string(i * i);
    check(keys == "abc" && um.size() == 1000 && um[31] == "961", "map and unordered_map");

    std::string s = peetcxx::library_greeting("peetos");
    check(s == "hello, peetos, from libpeetcxx" && std::stoi("1234") == 1234,
          "std::string across a shared library");

    std::ostringstream os;
    os << std::fixed << std::setprecision(3) << 3.14159 << ' ' << std::hex << 255;
    check(os.str() == "3.142 ff", "ostringstream formatting");

    check(std::format("{} + {} = {:.1f}", 2, 3, 5.0) == "2 + 3 = 5.0", "std::format");

    auto sp = std::make_shared<int>(10);
    std::weak_ptr<int> wp = sp;
    std::function<int(int)> add = [sp](int x) { return *sp + x; };
    bool alive = !wp.expired();
    int r = add(5);
    add = nullptr;
    sp.reset();
    check(alive && r == 15 && wp.expired(), "shared_ptr, weak_ptr, std::function");
}

// --- threads ---------------------------------------------------------------

std::atomic<int> g_tls_destroyed{0};

struct TlsObject {
    int value = 0;
    ~TlsObject() { g_tls_destroyed.fetch_add(1); }
};

thread_local TlsObject t_object;
thread_local int t_counter = 0;

std::atomic<int> g_static_ctor_count{0};

struct LazyStatic {
    LazyStatic() {
        g_static_ctor_count.fetch_add(1);
        std::this_thread::sleep_for(std::chrono::milliseconds(5));
    }
};

LazyStatic &lazy_static() {
    static LazyStatic instance; // __cxa_guard_acquire/release
    return instance;
}

void test_threads() {
    constexpr int kThreads = 4;
    constexpr int kIters = 20000;

    std::atomic<int> atomic_total{0};
    int locked_total = 0;
    std::mutex mu;
    std::vector<int> tls_seen(kThreads, -1);

    g_tls_destroyed = 0;
    std::vector<std::thread> threads;
    for (int t = 0; t < kThreads; ++t) {
        threads.emplace_back([&, t] {
            lazy_static();
            for (int i = 0; i < kIters; ++i) {
                atomic_total.fetch_add(1, std::memory_order_relaxed);
                ++t_counter;
                std::lock_guard<std::mutex> lock(mu);
                ++locked_total;
            }
            t_object.value = t;
            std::lock_guard<std::mutex> lock(mu);
            tls_seen[t] = t_counter;
        });
    }
    for (auto &th : threads) th.join();

    check(atomic_total == kThreads * kIters && locked_total == kThreads * kIters,
          "std::thread with atomics and std::mutex");
    check(std::all_of(tls_seen.begin(), tls_seen.end(), [](int c) { return c == kIters; }) &&
              t_counter == 0,
          "thread_local storage is per thread");
    check(g_tls_destroyed == kThreads, "thread_local destructors run at thread exit");
    check(g_static_ctor_count == 1, "function-local static initialized once under contention");

    std::mutex cv_mu;
    std::condition_variable cv;
    int stage = 0;
    std::thread waiter([&] {
        std::unique_lock<std::mutex> lock(cv_mu);
        cv.wait(lock, [&] { return stage == 1; });
        stage = 2;
        cv.notify_one();
    });
    {
        std::unique_lock<std::mutex> lock(cv_mu);
        stage = 1;
        cv.notify_one();
        cv.wait(lock, [&] { return stage == 2; });
    }
    waiter.join();
    check(stage == 2, "condition_variable handoff");

    std::future<int> sum = std::async(std::launch::async, [] {
        int s = 0;
        for (int i = 1; i <= 100; ++i) s += i;
        return s;
    });
    std::future<void> boom = std::async(std::launch::async, [] {
        throw std::logic_error("from a thread");
    });
    bool boom_caught = false;
    try {
        boom.get();
    } catch (const std::logic_error &e) {
        boom_caught = std::string(e.what()) == "from a thread";
    }
    check(sum.get() == 5050 && boom_caught, "std::async result and exception propagation");

    auto start = std::chrono::steady_clock::now();
    std::this_thread::sleep_for(std::chrono::milliseconds(20));
    auto elapsed = std::chrono::steady_clock::now() - start;
    check(elapsed >= std::chrono::milliseconds(20) && elapsed < std::chrono::seconds(5),
          "steady_clock and sleep_for");
}

// --- dlopen()ed C++ plugin ----------------------------------------------------

void test_plugin() {
    void *handle = dlopen("libpeetcxxplugin.so", RTLD_NOW | RTLD_LOCAL);
    check(handle != nullptr, "dlopen C++ plugin");
    if (!handle) {
        std::cout << "[cxx] dlopen: " << dlerror() << std::endl;
        return;
    }

    auto ctor_ran = reinterpret_cast<int (*)()>(dlsym(handle, "peetcxx_plugin_ctor_ran"));
    auto make_shape =
        reinterpret_cast<peetcxx::Shape *(*)(double)>(dlsym(handle, "peetcxx_plugin_make_shape"));
    check(ctor_ran && make_shape && ctor_ran() == 1, "plugin static constructor ran on dlopen");

    if (make_shape) {
        std::unique_ptr<peetcxx::Shape> c(make_shape(2.0));
        check(std::string(c->name()) == "circle" && std::fabs(c->area() - 4 * M_PI) < 1e-9,
              "virtual call into a plugin object");

        int code = 0;
        try {
            delete make_shape(-1.0);
        } catch (const peetcxx::LibraryError &e) {
            code = e.code();
        }
        check(code == -1, "exception thrown in a dlopen()ed plugin caught by its type");
    }

    check(dlclose(handle) == 0, "dlclose C++ plugin");
}

// --- std::filesystem ----------------------------------------------------------

void test_filesystem() {
    namespace fs = std::filesystem;
    bool found = false;
    std::error_code ec;
    for (const auto &entry : fs::directory_iterator("/usr/lib", ec)) {
        if (entry.path().filename() == "libstdc++.so.6") found = true;
    }
    check(!ec && found && fs::is_directory("/etc"), "std::filesystem directory iteration");
}

} // namespace

int main() {
    std::cout << "[cxx] start (libstdc++ " << __GLIBCXX__ << ", GCC " << __VERSION__ << ")"
              << std::endl;

    test_static_init();
    test_exceptions();
    test_rtti();
    test_stl();
    test_threads();
    test_plugin();
    test_filesystem();

    if (g_failures == 0) {
        std::cout << "[cxx] pass" << std::endl;
        return 0;
    }
    std::cout << "[cxx] fail (" << g_failures << ")" << std::endl;
    return 1;
}
