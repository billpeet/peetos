/*
 * FPU/SSE context-switch smoke test for PeetOS.
 *
 * Verifies that per-process x87/SSE register state survives:
 *   1. preemption  - several processes hold distinct XMM0-15 patterns,
 *                    MXCSR rounding modes and x87 control words while
 *                    spinning across many timer-driven context switches;
 *   2. signals     - a handler that trashes XMM registers and MXCSR must not
 *                    leak into the interrupted code, and must itself start
 *                    with the default MXCSR;
 *   3. execve      - a freshly exec'd image starts with default MXCSR/FCW
 *                    even if the pre-exec image changed them.
 *
 * Output markers: "[fpu-smoke] <phase> ok", "[fpu-smoke] FAIL ...",
 * and a final "[fpu-smoke] pass" on success.
 */
#define _GNU_SOURCE
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define NPROC 4
#define RUN_MS 1500
#define MIN_ROUNDS 16
#define SPIN_ITERS 2000000UL
#define EXEC_CHECK_ARG "--exec-check"
#define ABS_SELF "/mnt/ext2/bin/fpu_context_smoke"

#define DEFAULT_MXCSR 0x1F80u
#define DEFAULT_FCW 0x037Fu

static uint32_t read_mxcsr(void) {
    uint32_t v;
    __asm__ volatile("stmxcsr %0" : "=m"(v));
    return v;
}

static void write_mxcsr(uint32_t v) {
    __asm__ volatile("ldmxcsr %0" : : "m"(v));
}

static uint16_t read_fcw(void) {
    uint16_t v;
    __asm__ volatile("fnstcw %0" : "=m"(v));
    return v;
}

static void write_fcw(uint16_t v) {
    __asm__ volatile("fldcw %0" : : "m"(v));
}

static uint64_t now_ms(void) {
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) {
        return 0;
    }
    return (uint64_t)ts.tv_sec * 1000u + (uint64_t)ts.tv_nsec / 1000000u;
}

/*
 * Load XMM0-15 from `in`, spin `iters` times using only integer registers,
 * then store XMM0-15 to `out`. Any context switch during the spin that fails
 * to preserve this process's XMM state shows up as a mismatch.
 */
static void hold_xmm_across_spin(const uint8_t in[16][16], uint8_t out[16][16], unsigned long iters) {
    __asm__ volatile(
        "movdqu   0(%[in]), %%xmm0\n\t"
        "movdqu  16(%[in]), %%xmm1\n\t"
        "movdqu  32(%[in]), %%xmm2\n\t"
        "movdqu  48(%[in]), %%xmm3\n\t"
        "movdqu  64(%[in]), %%xmm4\n\t"
        "movdqu  80(%[in]), %%xmm5\n\t"
        "movdqu  96(%[in]), %%xmm6\n\t"
        "movdqu 112(%[in]), %%xmm7\n\t"
        "movdqu 128(%[in]), %%xmm8\n\t"
        "movdqu 144(%[in]), %%xmm9\n\t"
        "movdqu 160(%[in]), %%xmm10\n\t"
        "movdqu 176(%[in]), %%xmm11\n\t"
        "movdqu 192(%[in]), %%xmm12\n\t"
        "movdqu 208(%[in]), %%xmm13\n\t"
        "movdqu 224(%[in]), %%xmm14\n\t"
        "movdqu 240(%[in]), %%xmm15\n\t"
        "1:\n\t"
        "dec %[n]\n\t"
        "jnz 1b\n\t"
        "movdqu %%xmm0,    0(%[out])\n\t"
        "movdqu %%xmm1,   16(%[out])\n\t"
        "movdqu %%xmm2,   32(%[out])\n\t"
        "movdqu %%xmm3,   48(%[out])\n\t"
        "movdqu %%xmm4,   64(%[out])\n\t"
        "movdqu %%xmm5,   80(%[out])\n\t"
        "movdqu %%xmm6,   96(%[out])\n\t"
        "movdqu %%xmm7,  112(%[out])\n\t"
        "movdqu %%xmm8,  128(%[out])\n\t"
        "movdqu %%xmm9,  144(%[out])\n\t"
        "movdqu %%xmm10, 160(%[out])\n\t"
        "movdqu %%xmm11, 176(%[out])\n\t"
        "movdqu %%xmm12, 192(%[out])\n\t"
        "movdqu %%xmm13, 208(%[out])\n\t"
        "movdqu %%xmm14, 224(%[out])\n\t"
        "movdqu %%xmm15, 240(%[out])\n\t"
        : [n] "+r"(iters)
        : [in] "r"(in), [out] "r"(out)
        : "memory", "cc", "xmm0", "xmm1", "xmm2", "xmm3", "xmm4", "xmm5", "xmm6", "xmm7",
          "xmm8", "xmm9", "xmm10", "xmm11", "xmm12", "xmm13", "xmm14", "xmm15");
}

/* Per-worker MXCSR rounding mode and x87 precision control. */
static const uint32_t worker_mxcsr[NPROC] = {
    0x1F80u, /* round to nearest */
    0x7F80u, /* round toward zero */
    0x3F80u, /* round down */
    0x5F80u, /* round up */
};
static const uint16_t worker_fcw[NPROC] = {0x037Fu, 0x027Fu, 0x007Fu, 0x0F7Fu};

/* Returns 0 on success, 1 on any state corruption. */
static int preempt_worker(int id) {
    uint8_t in[16][16];
    uint8_t out[16][16];
    for (int r = 0; r < 16; r++) {
        for (int b = 0; b < 16; b++) {
            in[r][b] = (uint8_t)((id + 1) * 0x31 + r * 0x11 + b * 0x07);
        }
    }

    write_mxcsr(worker_mxcsr[id]);
    write_fcw(worker_fcw[id]);

    uint64_t start = now_ms();
    unsigned rounds = 0;
    for (;;) {
        memset(out, 0, sizeof(out));
        hold_xmm_across_spin(in, out, SPIN_ITERS);
        rounds++;
        if (memcmp(in, out, sizeof(in)) != 0) {
            for (int r = 0; r < 16; r++) {
                if (memcmp(in[r], out[r], 16) != 0) {
                    printf("[fpu-smoke] FAIL preempt worker %d round %u: xmm%d corrupted "
                           "(want %02x.. got %02x..)\n",
                           id, rounds, r, in[r][0], out[r][0]);
                    break;
                }
            }
            return 1;
        }
        uint32_t mxcsr = read_mxcsr() & ~0x3Fu; /* ignore sticky exception flags */
        if (mxcsr != worker_mxcsr[id]) {
            printf("[fpu-smoke] FAIL preempt worker %d round %u: mxcsr %#x want %#x\n", id,
                   rounds, mxcsr, worker_mxcsr[id]);
            return 1;
        }
        uint16_t fcw = read_fcw();
        if (fcw != worker_fcw[id]) {
            printf("[fpu-smoke] FAIL preempt worker %d round %u: fcw %#x want %#x\n", id, rounds,
                   fcw, worker_fcw[id]);
            return 1;
        }
        uint64_t elapsed = now_ms() - start;
        if (rounds >= MIN_ROUNDS && elapsed >= RUN_MS) {
            break;
        }
    }

    write_mxcsr(DEFAULT_MXCSR);
    write_fcw(DEFAULT_FCW);
    printf("[fpu-smoke] preempt worker %d ok (%u rounds)\n", id, rounds);
    return 0;
}

static int test_preempt(void) {
    pid_t pids[NPROC];
    int fail = 0;

    fflush(stdout);
    for (int i = 1; i < NPROC; i++) {
        pid_t pid = fork();
        if (pid < 0) {
            printf("[fpu-smoke] FAIL preempt: fork failed\n");
            return 1;
        }
        if (pid == 0) {
            int rc = preempt_worker(i);
            fflush(stdout);
            _exit(rc);
        }
        pids[i] = pid;
    }

    fail |= preempt_worker(0);

    for (int i = 1; i < NPROC; i++) {
        int status = 0;
        if (waitpid(pids[i], &status, 0) != pids[i] || !WIFEXITED(status) ||
            WEXITSTATUS(status) != 0) {
            printf("[fpu-smoke] FAIL preempt: worker %d status %#x\n", i, status);
            fail = 1;
        }
    }
    if (!fail) {
        printf("[fpu-smoke] preempt ok\n");
    }
    return fail;
}

static volatile sig_atomic_t handler_ran;
static volatile uint32_t handler_entry_mxcsr;

static void trash_fpu_handler(int sig) {
    (void)sig;
    handler_entry_mxcsr = read_mxcsr();
    static const uint8_t junk[16] = {0xde, 0xad, 0xbe, 0xef, 0xde, 0xad, 0xbe, 0xef,
                                     0xde, 0xad, 0xbe, 0xef, 0xde, 0xad, 0xbe, 0xef};
    __asm__ volatile("movdqu %0, %%xmm6\n\t"
                     "movdqu %0, %%xmm7\n\t"
                     "movdqu %0, %%xmm15\n\t"
                     :
                     : "m"(junk)
                     : "xmm6", "xmm7", "xmm15");
    write_mxcsr(0x3F80u);
    handler_ran = 1;
}

static int test_signal(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = trash_fpu_handler;
    sigemptyset(&sa.sa_mask);
    if (sigaction(SIGUSR1, &sa, NULL) != 0) {
        printf("[fpu-smoke] FAIL signal: sigaction failed\n");
        return 1;
    }

    uint8_t pattern[16];
    uint8_t after[16];
    for (int i = 0; i < 16; i++) {
        pattern[i] = (uint8_t)(0xA0 + i);
    }
    memset(after, 0, sizeof(after));

    const uint32_t interrupted_mxcsr = 0x7F80u;
    uint32_t mxcsr_after = 0;
    long pid = getpid();
    long ret;
    handler_ran = 0;
    handler_entry_mxcsr = 0;

    /*
     * Keep the pattern live in XMM6 and a non-default MXCSR across a raw
     * kill(getpid(), SIGUSR1). The handler runs on the way back from the
     * syscall and trashes both; rt_sigreturn must restore them.
     */
    __asm__ volatile("movdqu (%[pat]), %%xmm6\n\t"
                     "ldmxcsr (%[mx_in])\n\t"
                     "syscall\n\t"
                     "movdqu %%xmm6, (%[out])\n\t"
                     "stmxcsr (%[mx_out])\n\t"
                     : "=a"(ret)
                     : "a"(62L), "D"(pid), "S"((long)SIGUSR1), [pat] "r"(pattern),
                       [out] "r"(after), [mx_in] "r"(&interrupted_mxcsr),
                       [mx_out] "r"(&mxcsr_after)
                     : "rcx", "r11", "memory", "xmm6");
    write_mxcsr(DEFAULT_MXCSR);

    if (ret != 0) {
        printf("[fpu-smoke] FAIL signal: kill returned %ld\n", ret);
        return 1;
    }
    if (!handler_ran) {
        printf("[fpu-smoke] FAIL signal: handler did not run on syscall return\n");
        return 1;
    }
    int fail = 0;
    if (memcmp(pattern, after, sizeof(pattern)) != 0) {
        printf("[fpu-smoke] FAIL signal: xmm6 not restored after handler (got %02x..)\n",
               after[0]);
        fail = 1;
    }
    if ((mxcsr_after & ~0x3Fu) != interrupted_mxcsr) {
        printf("[fpu-smoke] FAIL signal: mxcsr %#x after handler, want %#x\n", mxcsr_after,
               interrupted_mxcsr);
        fail = 1;
    }
    if ((handler_entry_mxcsr & ~0x3Fu) != DEFAULT_MXCSR) {
        printf("[fpu-smoke] FAIL signal: handler entered with mxcsr %#x, want %#x\n",
               handler_entry_mxcsr, DEFAULT_MXCSR);
        fail = 1;
    }
    if (!fail) {
        printf("[fpu-smoke] signal ok\n");
    }
    return fail;
}

static int exec_check_main(void) {
    uint32_t mxcsr = read_mxcsr() & ~0x3Fu;
    uint16_t fcw = read_fcw();
    if (mxcsr != DEFAULT_MXCSR || fcw != DEFAULT_FCW) {
        printf("[fpu-smoke] FAIL exec: new image started with mxcsr %#x fcw %#x\n", mxcsr, fcw);
        return 1;
    }
    return 0;
}

static int test_exec(const char *self) {
    fflush(stdout);
    pid_t pid = fork();
    if (pid < 0) {
        printf("[fpu-smoke] FAIL exec: fork failed\n");
        return 1;
    }
    if (pid == 0) {
        write_mxcsr(0x7F80u);
        write_fcw(0x027Fu);
        char *const args[] = {(char *)self, (char *)EXEC_CHECK_ARG, 0};
        char *const envp[] = {0};
        execve(self, args, envp);
        execve(ABS_SELF, args, envp);
        _exit(127);
    }
    int status = 0;
    if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        printf("[fpu-smoke] FAIL exec: child status %#x\n", status);
        return 1;
    }
    printf("[fpu-smoke] exec ok\n");
    return 0;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);

    if (argc > 1 && strcmp(argv[1], EXEC_CHECK_ARG) == 0) {
        return exec_check_main();
    }

    int fail = 0;
    fail |= test_preempt();
    fail |= test_signal();
    fail |= test_exec(argc > 0 ? argv[0] : ABS_SELF);

    if (fail) {
        printf("[fpu-smoke] failures\n");
        return 1;
    }
    printf("[fpu-smoke] pass\n");
    return 0;
}
