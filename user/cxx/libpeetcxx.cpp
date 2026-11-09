// libpeetcxx.so: a C++ shared library linked by cxx_smoke (DT_NEEDED).

#include "peetcxx.h"

namespace peetcxx {

namespace {

int g_ctor_count = 0;

struct LibraryInit {
    LibraryInit() { ++g_ctor_count; }
};

// Runs from the library's .init_array before main().
LibraryInit g_library_init;

class Square final : public Shape {
public:
    explicit Square(double side) : side_(side) {}
    const char *name() const override { return "square"; }
    double area() const override { return side_ * side_; }

private:
    double side_;
};

} // namespace

// Key function: emits Shape's vtable and type_info in this library.
Shape::~Shape() = default;

Shape *make_square(double side) { return new Square(side); }

void throw_library_error(int code) {
    throw LibraryError("thrown from libpeetcxx.so", code);
}

int library_ctor_count() { return g_ctor_count; }

std::string library_greeting(const std::string &who) {
    return "hello, " + who + ", from libpeetcxx";
}

} // namespace peetcxx
