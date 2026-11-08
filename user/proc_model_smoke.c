/*
 * Process-model smoke test: copy-on-write fork and musl pthreads
 * (CLONE_THREAD thread groups).
 *
 * Prints "[proc-smoke] pass" when every check succeeds, otherwise one
 * "[proc-smoke] FAIL ..." line per failed check, and exits non-zero.
 */
#include <errno.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

static int failures;

#define CHECK(cond, what)                                                       \
    do {                                                                        \
        if (!(cond)) {                                                          \
            printf("[proc-smoke] FAIL %s (line %d, errno %d)\n", (what),        \
                   __LINE__, errno);                                            \
            failures++;                                                         \
        }                                                                       \
    } while (0)

static pid_t gettid_raw(void) {
    return (pid_t)syscall(SYS_gettid);
}

/* Wait for `pid` and return its exit status, or -1. */
static int wait_exit_status(pid_t pid) {
    int status = 0;
    pid_t got = waitpid(pid, &status, 0);
    if (got != pid || !WIFEXITED(status)) {
        return -1;
    }
    return WEXITSTATUS(status);
}

/* ------------------------------------------------------------------------ */
/* Copy-on-write fork                                                        */
/* ------------------------------------------------------------------------ */

#define COW_HEAP_PAGES 64
static char data_page[4096] = "parent-data";
static char cow_target[2 * 4096] __attribute__((aligned(4096)));

static void test_cow_fork_isolation(void) {
    size_t len = COW_HEAP_PAGES * 4096;
    unsigned char *heap = malloc(len);
    CHECK(heap != NULL, "cow: malloc");
    if (heap == NULL) {
        return;
    }
    memset(heap, 0xA5, len);
    volatile int stack_val = 1234;
    /* Touch the page so it is present (and thus COW-shared) at fork time. */
    cow_target[0] = 'P';

    int to_child[2];
    CHECK(pipe(to_child) == 0, "cow: pipe");

    pid_t pid = fork();
    CHECK(pid >= 0, "cow: fork");
    if (pid == 0) {
        close(to_child[1]);
        int ok = 1;
        /* The kernel writes this read() into a page still shared with the
         * parent; it must break COW instead of writing the shared frame. It
         * also orders us after the parent's own writes below. */
        char *dst = cow_target + 100;
        ok &= read(to_child[0], dst, 16) == 16;
        ok &= memcmp(dst, "from-parent-pipe", 16) == 0;

        /* The parent changed its heap before writing to the pipe; our copy
         * must still hold the pre-fork contents. */
        ok &= heap[0] == 0xA5 && heap[len / 2] == 0xA5 && heap[len - 1] == 0xA5;
        ok &= strcmp(data_page, "parent-data") == 0;
        ok &= stack_val == 1234;

        memset(heap, 0x5A, len);
        strcpy(data_page, "child-data");
        stack_val = 99;
        ok &= heap[len - 1] == 0x5A && stack_val == 99;
        _exit(ok ? 0 : 1);
    }

    close(to_child[0]);
    memset(heap, 0x11, len);
    CHECK(write(to_child[1], "from-parent-pipe", 16) == 16, "cow: pipe write");
    close(to_child[1]);

    CHECK(wait_exit_status(pid) == 0, "cow: child saw isolated memory");
    CHECK(heap[0] == 0x11 && heap[len - 1] == 0x11, "cow: parent heap kept");
    CHECK(strcmp(data_page, "parent-data") == 0, "cow: parent .data kept");
    CHECK(stack_val == 1234, "cow: parent stack kept");
    CHECK(cow_target[0] == 'P', "cow: parent page marker kept");
    static const char zeros[16];
    CHECK(memcmp(cow_target + 100, zeros, 16) == 0,
          "cow: kernel write in child did not leak to parent");
    free(heap);
}

/* Several generations of forks sharing pages, each modifying its own copy. */
static void test_cow_fork_chain(void) {
    size_t len = 8 * 4096;
    unsigned char *buf = malloc(len);
    CHECK(buf != NULL, "chain: malloc");
    if (buf == NULL) {
        return;
    }
    memset(buf, 0, len);

    for (int gen = 0; gen < 6; gen++) {
        pid_t pid = fork();
        CHECK(pid >= 0, "chain: fork");
        if (pid < 0) {
            break;
        }
        if (pid == 0) {
            /* Each generation must see exactly the writes of its ancestors. */
            int ok = 1;
            for (int g = 0; g < gen; g++) {
                ok &= buf[g * 4096 / 2] == (unsigned char)(g + 1);
            }
            ok &= buf[gen * 4096 / 2] == 0;
            buf[gen * 4096 / 2] = (unsigned char)(gen + 1);
            if (!ok) {
                _exit(1);
            }
            if (gen == 5) {
                _exit(0);
            }
            continue; /* this child forks the next generation */
        }
        int status = wait_exit_status(pid);
        if (gen == 0) {
            CHECK(status == 0, "chain: descendants saw consistent memory");
            CHECK(buf[0] == 0, "chain: original process untouched");
            free(buf);
            return;
        }
        _exit(status == 0 ? 0 : 1);
    }
    _exit(1);
}

/* Many short-lived children: exercises frame share counts and teardown. */
static void test_fork_many(void) {
    unsigned char *buf = malloc(4 * 4096);
    CHECK(buf != NULL, "many: malloc");
    if (buf == NULL) {
        return;
    }
    memset(buf, 0x33, 4 * 4096);
    int ok_children = 0;
    for (int i = 0; i < 24; i++) {
        pid_t pid = fork();
        if (pid == 0) {
            buf[i % (4 * 4096)] = 0x44;
            _exit(buf[(i + 1) % (4 * 4096)] == 0x33 ? 0 : 1);
        }
        if (pid > 0 && wait_exit_status(pid) == 0) {
            ok_children++;
        }
    }
    CHECK(ok_children == 24, "many: all children ok");
    int intact = 1;
    for (int i = 0; i < 4 * 4096; i++) {
        intact &= buf[i] == 0x33;
    }
    CHECK(intact, "many: parent buffer intact");
    free(buf);
}

/* ------------------------------------------------------------------------ */
/* Threads                                                                   */
/* ------------------------------------------------------------------------ */

#define NTHREADS 4
#define ITERS 2000

static pthread_mutex_t counter_lock = PTHREAD_MUTEX_INITIALIZER;
static long counter;
static __thread int tls_value;
static pid_t worker_tid[NTHREADS];
static pid_t main_pid;
static int shared_pipe[2];

static void *counter_worker(void *arg) {
    int idx = (int)(long)arg;
    tls_value = 1000 + idx;
    worker_tid[idx] = gettid_raw();
    for (int i = 0; i < ITERS; i++) {
        pthread_mutex_lock(&counter_lock);
        counter++;
        pthread_mutex_unlock(&counter_lock);
        if ((i % 256) == 0) {
            sched_yield();
        }
    }
    if (tls_value != 1000 + idx) {
        return (void *)1;
    }
    if (getpid() != main_pid) {
        return (void *)2;
    }
    /* The fd table is shared: write to a pipe the main thread created. */
    char byte = (char)('a' + idx);
    if (write(shared_pipe[1], &byte, 1) != 1) {
        return (void *)3;
    }
    return (void *)(long)(100 + idx);
}

static void test_threads_basic(void) {
    main_pid = getpid();
    tls_value = 7;
    CHECK(pipe(shared_pipe) == 0, "threads: pipe");

    pthread_t threads[NTHREADS];
    for (int i = 0; i < NTHREADS; i++) {
        CHECK(pthread_create(&threads[i], NULL, counter_worker, (void *)(long)i) == 0,
              "threads: create");
    }
    for (int i = 0; i < NTHREADS; i++) {
        void *ret = NULL;
        CHECK(pthread_join(threads[i], &ret) == 0, "threads: join");
        CHECK(ret == (void *)(long)(100 + i), "threads: return value");
    }

    CHECK(counter == (long)NTHREADS * ITERS, "threads: mutex-protected counter");
    CHECK(tls_value == 7, "threads: main TLS untouched");

    pid_t main_tid = gettid_raw();
    CHECK(main_tid == main_pid, "threads: main tid == pid");
    for (int i = 0; i < NTHREADS; i++) {
        CHECK(worker_tid[i] > 0 && worker_tid[i] != main_tid, "threads: distinct tid");
        for (int j = 0; j < i; j++) {
            CHECK(worker_tid[i] != worker_tid[j], "threads: unique tids");
        }
    }

    char bytes[NTHREADS];
    CHECK(read(shared_pipe[0], bytes, NTHREADS) == NTHREADS, "threads: shared fd table");
    close(shared_pipe[0]);
    close(shared_pipe[1]);
}

/* Condition variable handshake (futex wait/wake across threads). */
static pthread_mutex_t cv_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t cv = PTHREAD_COND_INITIALIZER;
static int cv_stage;

static void *cv_worker(void *arg) {
    (void)arg;
    pthread_mutex_lock(&cv_lock);
    while (cv_stage != 1) {
        pthread_cond_wait(&cv, &cv_lock);
    }
    cv_stage = 2;
    pthread_cond_signal(&cv);
    pthread_mutex_unlock(&cv_lock);
    return NULL;
}

static void test_condvar(void) {
    pthread_t t;
    CHECK(pthread_create(&t, NULL, cv_worker, NULL) == 0, "condvar: create");
    usleep(10000);
    pthread_mutex_lock(&cv_lock);
    cv_stage = 1;
    pthread_cond_signal(&cv);
    while (cv_stage != 2) {
        pthread_cond_wait(&cv, &cv_lock);
    }
    pthread_mutex_unlock(&cv_lock);
    CHECK(pthread_join(t, NULL) == 0, "condvar: join");
    CHECK(cv_stage == 2, "condvar: handshake");
}

/* tgkill: a signal sent to one thread runs its handler on that thread. */
static volatile pid_t handler_tid;
static volatile int handler_hits;

static void record_handler(int sig) {
    (void)sig;
    handler_tid = gettid_raw();
    handler_hits++;
}

static volatile int sig_worker_ready;
static volatile pid_t sig_worker_tid;

static void *signal_worker(void *arg) {
    (void)arg;
    sig_worker_tid = gettid_raw();
    sig_worker_ready = 1;
    for (int i = 0; i < 2000 && handler_hits == 0; i++) {
        usleep(1000);
    }
    return NULL;
}

static void test_thread_signals(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = record_handler;
    CHECK(sigaction(SIGUSR1, &sa, NULL) == 0, "signals: sigaction");

    pthread_t t;
    CHECK(pthread_create(&t, NULL, signal_worker, NULL) == 0, "signals: create");
    while (!sig_worker_ready) {
        sched_yield();
    }
    CHECK(syscall(SYS_tgkill, getpid(), sig_worker_tid, SIGUSR1) == 0, "signals: tgkill");
    CHECK(pthread_join(t, NULL) == 0, "signals: join");
    CHECK(handler_hits == 1, "signals: handler ran once");
    CHECK(handler_tid == sig_worker_tid, "signals: handler ran on target thread");

    /* raise() uses tkill on the calling thread. */
    handler_hits = 0;
    CHECK(raise(SIGUSR1) == 0, "signals: raise");
    CHECK(handler_hits == 1 && handler_tid == gettid_raw(), "signals: raise on self");

    CHECK(syscall(SYS_tgkill, getpid() + 1000, gettid_raw(), 0) == -1 && errno == ESRCH,
          "signals: tgkill rejects wrong tgid");
    signal(SIGUSR1, SIG_DFL);
}

/* ------------------------------------------------------------------------ */
/* Thread-group exit                                                         */
/* ------------------------------------------------------------------------ */

static int blocker_pipe[2];
static volatile unsigned long spin_counter;

static void *blocked_reader(void *arg) {
    (void)arg;
    char c;
    (void)read(blocker_pipe[0], &c, 1); /* never satisfied */
    return NULL;
}

static void *spinner(void *arg) {
    (void)arg;
    for (;;) {
        spin_counter++;
    }
    return NULL;
}

static void *exit_from_thread(void *arg) {
    usleep(10000);
    exit((int)(long)arg);
    return NULL;
}

static void *late_exit_worker(void *arg) {
    (void)arg;
    usleep(20000);
    exit(5);
    return NULL;
}

static void test_group_exit(void) {
    /* exit() in the main thread kills a blocked and a CPU-bound thread. */
    pid_t pid = fork();
    if (pid == 0) {
        pthread_t a, b;
        if (pipe(blocker_pipe) != 0) {
            _exit(90);
        }
        if (pthread_create(&a, NULL, blocked_reader, NULL) != 0 ||
            pthread_create(&b, NULL, spinner, NULL) != 0) {
            _exit(91);
        }
        usleep(20000);
        exit(7);
    }
    CHECK(pid > 0 && wait_exit_status(pid) == 7, "group exit: main thread exit status");

    /* exit() in a worker kills the main thread blocked in pthread_join. */
    pid = fork();
    if (pid == 0) {
        pthread_t t;
        if (pthread_create(&t, NULL, exit_from_thread, (void *)9L) != 0) {
            _exit(92);
        }
        pthread_join(t, NULL);
        _exit(93); /* not reached */
    }
    CHECK(pid > 0 && wait_exit_status(pid) == 9, "group exit: worker exit status");

    /* The main thread leaves with pthread_exit; the process lives on until
     * the last thread exits. */
    pid = fork();
    if (pid == 0) {
        pthread_t t;
        if (pthread_create(&t, NULL, late_exit_worker, NULL) != 0) {
            _exit(94);
        }
        pthread_exit(NULL);
    }
    CHECK(pid > 0 && wait_exit_status(pid) == 5, "group exit: leader exits first");

    /* SIGKILL ends a process blocked in a pipe read. */
    pid = fork();
    if (pid == 0) {
        int p[2];
        char c;
        if (pipe(p) != 0) {
            _exit(95);
        }
        (void)read(p[0], &c, 1);
        _exit(96);
    }
    usleep(10000);
    CHECK(pid > 0 && kill(pid, SIGKILL) == 0, "group exit: kill");
    CHECK(wait_exit_status(pid) == 128 + SIGKILL, "group exit: SIGKILL ends blocked read");
}

/* A forked child of a threaded process is single-threaded and works. */
static void *idle_worker(void *arg) {
    (void)arg;
    usleep(30000);
    return NULL;
}

static void test_fork_from_thread_group(void) {
    pthread_t t;
    CHECK(pthread_create(&t, NULL, idle_worker, NULL) == 0, "fork+threads: create");
    pid_t pid = fork();
    if (pid == 0) {
        _exit(getppid() == main_pid ? 0 : 1);
    }
    CHECK(pid > 0 && wait_exit_status(pid) == 0, "fork+threads: child ppid");
    CHECK(pthread_join(t, NULL) == 0, "fork+threads: join");
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("[proc-smoke] start pid=%d\n", (int)getpid());

    test_cow_fork_isolation();
    test_cow_fork_chain();
    test_fork_many();
    printf("[proc-smoke] cow checks done (failures=%d)\n", failures);

    test_threads_basic();
    test_condvar();
    test_thread_signals();
    printf("[proc-smoke] thread checks done (failures=%d)\n", failures);

    test_group_exit();
    test_fork_from_thread_group();

    if (failures == 0) {
        printf("[proc-smoke] pass\n");
        return 0;
    }
    printf("[proc-smoke] %d failure(s)\n", failures);
    return 1;
}
