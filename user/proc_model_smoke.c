/*
 * Process-model smoke test: copy-on-write fork, musl pthreads (CLONE_THREAD
 * thread groups), futex timeouts, thread cancellation, Linux signal frames
 * and syscall restart, execve from a thread, and interruptible socket waits.
 *
 * Prints "[proc-smoke] pass" when every check succeeds, otherwise one
 * "[proc-smoke] FAIL ..." line per failed check, and exits non-zero.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <time.h>
#include <ucontext.h>
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

/* ------------------------------------------------------------------------ */
/* Futex timeouts                                                            */
/* ------------------------------------------------------------------------ */

static long elapsed_ms(const struct timespec *a, const struct timespec *b) {
    return (b->tv_sec - a->tv_sec) * 1000L + (b->tv_nsec - a->tv_nsec) / 1000000L;
}

static void deadline_in_ms(struct timespec *ts, long ms) {
    clock_gettime(CLOCK_REALTIME, ts);
    ts->tv_sec += ms / 1000;
    ts->tv_nsec += (ms % 1000) * 1000000L;
    if (ts->tv_nsec >= 1000000000L) {
        ts->tv_sec++;
        ts->tv_nsec -= 1000000000L;
    }
}

static pthread_mutex_t tw_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t tw_cond = PTHREAD_COND_INITIALIZER;
static int tw_flag;

static void *tw_signaller(void *arg) {
    (void)arg;
    usleep(10000);
    pthread_mutex_lock(&tw_lock);
    tw_flag = 1;
    pthread_cond_signal(&tw_cond);
    pthread_mutex_unlock(&tw_lock);
    return NULL;
}

static void test_timed_waits(void) {
    struct timespec deadline, t0, t1;

    /* Nobody signals: the wait must time out after about 50 ms. */
    deadline_in_ms(&deadline, 50);
    clock_gettime(CLOCK_MONOTONIC, &t0);
    pthread_mutex_lock(&tw_lock);
    int rc = pthread_cond_timedwait(&tw_cond, &tw_lock, &deadline);
    pthread_mutex_unlock(&tw_lock);
    clock_gettime(CLOCK_MONOTONIC, &t1);
    long ms = elapsed_ms(&t0, &t1);
    CHECK(rc == ETIMEDOUT, "timedwait: times out");
    CHECK(ms >= 40 && ms < 2000, "timedwait: waited about the timeout");

    /* Signalled well before a long deadline: returns 0 early. */
    pthread_t t;
    tw_flag = 0;
    CHECK(pthread_create(&t, NULL, tw_signaller, NULL) == 0, "timedwait: create");
    deadline_in_ms(&deadline, 5000);
    clock_gettime(CLOCK_MONOTONIC, &t0);
    pthread_mutex_lock(&tw_lock);
    rc = 0;
    while (!tw_flag && rc == 0) {
        rc = pthread_cond_timedwait(&tw_cond, &tw_lock, &deadline);
    }
    pthread_mutex_unlock(&tw_lock);
    clock_gettime(CLOCK_MONOTONIC, &t1);
    CHECK(rc == 0 && tw_flag == 1, "timedwait: woken by signal");
    CHECK(elapsed_ms(&t0, &t1) < 2000, "timedwait: woken early");
    CHECK(pthread_join(t, NULL) == 0, "timedwait: join");

    /* A mutex timed lock that cannot succeed. */
    pthread_mutex_t m = PTHREAD_MUTEX_INITIALIZER;
    pthread_mutex_lock(&m);
    deadline_in_ms(&deadline, 30);
    /* Locking again from the same thread on a normal mutex would deadlock
     * forever without timeouts; with them it reports ETIMEDOUT. */
    CHECK(pthread_mutex_timedlock(&m, &deadline) == ETIMEDOUT, "timedlock: times out");
    pthread_mutex_unlock(&m);
}

/* ------------------------------------------------------------------------ */
/* Signal masks, real-time signals and siginfo                               */
/* ------------------------------------------------------------------------ */

static volatile int mask_hits;
static void count_handler(int sig) {
    (void)sig;
    mask_hits++;
}

static volatile int info_signo, info_code, info_pid;
static volatile unsigned long info_rip;

static void info_handler(int sig, siginfo_t *si, void *ucv) {
    ucontext_t *uc = ucv;
    info_signo = sig;
    info_code = si->si_code;
    info_pid = si->si_pid;
    info_rip = (unsigned long)uc->uc_mcontext.gregs[REG_RIP];
    /* Edits to the saved context take effect on return. */
    sigaddset(&uc->uc_sigmask, SIGUSR2);
}

static void test_signal_masks_and_info(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = count_handler;
    CHECK(sigaction(SIGUSR1, &sa, NULL) == 0, "mask: sigaction");

    /* A blocked signal stays pending until it is unblocked. */
    sigset_t set, old;
    sigemptyset(&set);
    sigaddset(&set, SIGUSR1);
    CHECK(sigprocmask(SIG_BLOCK, &set, &old) == 0, "mask: block");
    mask_hits = 0;
    raise(SIGUSR1);
    CHECK(mask_hits == 0, "mask: blocked signal not delivered");
    CHECK(sigprocmask(SIG_SETMASK, &old, NULL) == 0, "mask: unblock");
    CHECK(mask_hits == 1, "mask: delivered once unblocked");

    /* Real-time signals (>= 32) can be caught. */
    int rt = SIGRTMIN + 1;
    CHECK(sigaction(rt, &sa, NULL) == 0, "rt: sigaction");
    mask_hits = 0;
    raise(rt);
    CHECK(mask_hits == 1, "rt: handler ran");

    /* SA_SIGINFO handlers get siginfo_t and a Linux ucontext_t. */
    memset(&sa, 0, sizeof(sa));
    sa.sa_sigaction = info_handler;
    sa.sa_flags = SA_SIGINFO;
    CHECK(sigaction(SIGUSR1, &sa, NULL) == 0, "siginfo: sigaction");
    info_signo = 0;
    CHECK(kill(getpid(), SIGUSR1) == 0, "siginfo: kill");
    CHECK(info_signo == SIGUSR1, "siginfo: signo");
    CHECK(info_code == SI_USER, "siginfo: SI_USER from kill");
    CHECK(info_pid == getpid(), "siginfo: sender pid");
    CHECK(info_rip != 0, "siginfo: ucontext RIP");
    sigset_t now;
    CHECK(sigprocmask(SIG_BLOCK, NULL, &now) == 0 && sigismember(&now, SIGUSR2),
          "siginfo: uc_sigmask edit applied on return");
    sigemptyset(&set);
    sigaddset(&set, SIGUSR2);
    sigprocmask(SIG_UNBLOCK, &set, NULL);

    info_code = 0;
    CHECK(syscall(SYS_tgkill, getpid(), gettid_raw(), SIGUSR1) == 0, "siginfo: tgkill");
    CHECK(info_code == SI_TKILL, "siginfo: SI_TKILL from tgkill");
    sigprocmask(SIG_UNBLOCK, &set, NULL);

    signal(SIGUSR1, SIG_DFL);
    signal(rt, SIG_DFL);
}

/* ------------------------------------------------------------------------ */
/* Interrupted syscalls: EINTR vs SA_RESTART                                 */
/* ------------------------------------------------------------------------ */

static int child_read_with_handler(int restart) {
    int p[2];
    if (pipe(p) != 0) {
        return -1;
    }
    pid_t pid = fork();
    if (pid == 0) {
        struct sigaction sa;
        memset(&sa, 0, sizeof(sa));
        sa.sa_handler = count_handler;
        sa.sa_flags = restart ? SA_RESTART : 0;
        sigaction(SIGUSR1, &sa, NULL);
        close(p[1]);
        mask_hits = 0;
        char c = 0;
        ssize_t n = read(p[0], &c, 1);
        if (restart) {
            /* Handler ran, then the read resumed and got the byte. */
            _exit(n == 1 && c == 'x' && mask_hits == 1 ? 0 : 1);
        }
        _exit(n == -1 && errno == EINTR && mask_hits == 1 ? 0 : 1);
    }
    /* Keep our read end open: in the EINTR case the child is gone before
     * this write, which would otherwise raise SIGPIPE here. */
    usleep(20000);
    kill(pid, SIGUSR1);
    usleep(20000);
    (void)write(p[1], "x", 1);
    close(p[0]);
    close(p[1]);
    return wait_exit_status(pid);
}

static void test_interrupted_syscalls(void) {
    CHECK(child_read_with_handler(0) == 0, "eintr: read interrupted without SA_RESTART");
    CHECK(child_read_with_handler(1) == 0, "restart: read resumed with SA_RESTART");

    /* nanosleep reports EINTR and the time left. */
    pid_t pid = fork();
    if (pid == 0) {
        struct sigaction sa;
        memset(&sa, 0, sizeof(sa));
        sa.sa_handler = count_handler;
        sigaction(SIGUSR1, &sa, NULL);
        struct timespec req = {5, 0}, rem = {0, 0};
        int r = nanosleep(&req, &rem);
        _exit(r == -1 && errno == EINTR && rem.tv_sec >= 1 ? 0 : 1);
    }
    usleep(20000);
    kill(pid, SIGUSR1);
    CHECK(wait_exit_status(pid) == 0, "eintr: nanosleep interrupted with time left");
}

/* ------------------------------------------------------------------------ */
/* Thread cancellation                                                       */
/* ------------------------------------------------------------------------ */

static volatile int cleanup_runs;
static void count_cleanup(void *arg) {
    (void)arg;
    cleanup_runs++;
}

static int cancel_pipe[2];

static void *cancel_in_read(void *arg) {
    (void)arg;
    char c;
    pthread_cleanup_push(count_cleanup, NULL);
    (void)read(cancel_pipe[0], &c, 1);
    pthread_cleanup_pop(0);
    return (void *)1;
}

static pthread_mutex_t cancel_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t cancel_cond = PTHREAD_COND_INITIALIZER;

static void unlock_cleanup(void *arg) {
    pthread_mutex_unlock(arg);
    cleanup_runs++;
}

static void *cancel_in_cond_wait(void *arg) {
    (void)arg;
    pthread_mutex_lock(&cancel_lock);
    pthread_cleanup_push(unlock_cleanup, &cancel_lock);
    for (;;) {
        pthread_cond_wait(&cancel_cond, &cancel_lock);
    }
    pthread_cleanup_pop(1);
    return (void *)1;
}

static void *cancel_in_sleep(void *arg) {
    (void)arg;
    for (;;) {
        usleep(1000);
    }
    return (void *)1;
}

static void *cancel_in_testcancel(void *arg) {
    (void)arg;
    for (;;) {
        spin_counter++;
        pthread_testcancel();
    }
    return (void *)1;
}

static void cancel_one(void *(*fn)(void *), const char *what) {
    pthread_t t;
    void *ret = NULL;
    char label[96];
    snprintf(label, sizeof(label), "cancel: %s", what);
    if (pthread_create(&t, NULL, fn, NULL) != 0) {
        CHECK(0, label);
        return;
    }
    usleep(20000);
    CHECK(pthread_cancel(t) == 0, label);
    CHECK(pthread_join(t, &ret) == 0 && ret == PTHREAD_CANCELED, label);
}

static void test_cancellation(void) {
    CHECK(pipe(cancel_pipe) == 0, "cancel: pipe");
    cleanup_runs = 0;
    cancel_one(cancel_in_read, "blocked in read");
    CHECK(cleanup_runs == 1, "cancel: cleanup handler ran (read)");
    cancel_one(cancel_in_cond_wait, "blocked in pthread_cond_wait");
    CHECK(cleanup_runs == 2, "cancel: cleanup handler ran (condvar)");
    CHECK(pthread_mutex_trylock(&cancel_lock) == 0, "cancel: condvar mutex released");
    pthread_mutex_unlock(&cancel_lock);
    cancel_one(cancel_in_sleep, "sleeping");
    cancel_one(cancel_in_testcancel, "pthread_testcancel loop");
    close(cancel_pipe[0]);
    close(cancel_pipe[1]);
}

/* ------------------------------------------------------------------------ */
/* execve from a non-leader thread                                           */
/* ------------------------------------------------------------------------ */

static const char *self_path = "/mnt/ext2/bin/proc_model_smoke";

static void *exec_from_thread(void *arg) {
    (void)arg;
    char pidbuf[16];
    snprintf(pidbuf, sizeof(pidbuf), "%d", (int)getpid());
    char *argv[] = {(char *)"proc_model_smoke", (char *)"--exec-child", pidbuf, NULL};
    usleep(10000);
    execv(self_path, argv);
    execv("bin/proc_model_smoke", argv);
    _exit(97);
    return NULL;
}

static void test_exec_from_thread(void) {
    pid_t pid = fork();
    if (pid == 0) {
        pthread_t blocked, execer;
        if (pipe(blocker_pipe) != 0) {
            _exit(90);
        }
        /* A sibling blocked in the kernel must not survive the exec. */
        if (pthread_create(&blocked, NULL, blocked_reader, NULL) != 0 ||
            pthread_create(&execer, NULL, exec_from_thread, NULL) != 0) {
            _exit(91);
        }
        pthread_join(execer, NULL);
        _exit(98); /* not reached: the process image is replaced */
    }
    CHECK(pid > 0 && wait_exit_status(pid) == 0, "exec from thread: new image keeps the pid");
}

/* Mode used by test_exec_from_thread for the exec'd image. */
static int exec_child_main(const char *expected_pid) {
    int ok = getpid() == atoi(expected_pid) && gettid_raw() == getpid();
    printf("[proc-smoke] exec-child %s (pid=%d tid=%d)\n", ok ? "ok" : "BAD", (int)getpid(),
           (int)gettid_raw());
    return ok ? 0 : 1;
}

/* ------------------------------------------------------------------------ */
/* Socket waits and process exit                                             */
/* ------------------------------------------------------------------------ */

static int accept_fd = -1;

static void *blocked_accept(void *arg) {
    (void)arg;
    (void)accept(accept_fd, NULL, NULL);
    return NULL;
}

static void test_socket_waits(void) {
    /* exit() while a sibling thread is blocked in accept(). */
    pid_t pid = fork();
    if (pid == 0) {
        struct sockaddr_un addr;
        memset(&addr, 0, sizeof(addr));
        addr.sun_family = AF_UNIX;
        snprintf(addr.sun_path, sizeof(addr.sun_path), "/temp/proc-smoke-%d.sock", (int)getpid());
        accept_fd = socket(AF_UNIX, SOCK_STREAM, 0);
        if (accept_fd < 0 || bind(accept_fd, (struct sockaddr *)&addr, sizeof(addr)) != 0 ||
            listen(accept_fd, 1) != 0) {
            _exit(80);
        }
        pthread_t t;
        if (pthread_create(&t, NULL, blocked_accept, NULL) != 0) {
            _exit(81);
        }
        usleep(20000);
        exit(3);
    }
    CHECK(pid > 0 && wait_exit_status(pid) == 3, "socket: exit with thread blocked in accept");

    /* A handled signal interrupts recv() with EINTR. */
    int sv[2];
    CHECK(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0, "socket: socketpair");
    pid = fork();
    if (pid == 0) {
        struct sigaction sa;
        memset(&sa, 0, sizeof(sa));
        sa.sa_handler = count_handler;
        sigaction(SIGUSR1, &sa, NULL);
        char c;
        ssize_t n = recv(sv[1], &c, 1, 0);
        _exit(n == -1 && errno == EINTR ? 0 : 1);
    }
    usleep(20000);
    kill(pid, SIGUSR1);
    CHECK(wait_exit_status(pid) == 0, "socket: recv interrupted by signal");

    /* SIGKILL ends a process blocked in recv(). */
    pid = fork();
    if (pid == 0) {
        char c;
        (void)recv(sv[1], &c, 1, 0);
        _exit(1);
    }
    usleep(20000);
    kill(pid, SIGKILL);
    CHECK(wait_exit_status(pid) == 128 + SIGKILL, "socket: SIGKILL ends blocked recv");
    close(sv[0]);
    close(sv[1]);
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    if (argc >= 3 && strcmp(argv[1], "--exec-child") == 0) {
        return exec_child_main(argv[2]);
    }
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
    printf("[proc-smoke] exit checks done (failures=%d)\n", failures);

    test_timed_waits();
    test_signal_masks_and_info();
    test_interrupted_syscalls();
    printf("[proc-smoke] signal checks done (failures=%d)\n", failures);

    test_cancellation();
    test_exec_from_thread();
    test_socket_waits();

    if (failures == 0) {
        printf("[proc-smoke] pass\n");
        return 0;
    }
    printf("[proc-smoke] %d failure(s)\n", failures);
    return 1;
}
