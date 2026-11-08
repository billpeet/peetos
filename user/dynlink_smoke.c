/*
 * Dynamic-linking smoke test for PeetOS.
 *
 * A dynamically linked PIE that runs through musl's loader
 * (/lib/ld-musl-x86_64.so.1, i.e. musl's libc.so) and checks:
 *   - libc.so itself (stdio, malloc)
 *   - a NEEDED shared library: constructor, functions, exported data, and a
 *     TLS variable in the library, including a fresh copy in a new thread
 *   - dlopen()/dlsym() of a plugin that depends on the first library and has
 *     its own (dynamically allocated) TLS block; dlerror() for a missing one
 *   - fork + execve of itself (a dynamic program exec'ing a dynamic program)
 *   - many distinct mappings at once (well past the old 32-region limit) on
 *     top of the loaded libraries, and an mprotect split + restore
 *   - platform limits: 192 MiB of memory (old physical limit 128 MiB), 100
 *     concurrent child processes (old task limit 64), 120 sockets (old 96)
 *
 * Output: "[dynlink] <check> ok", "[dynlink] FAIL ...", and "[dynlink] pass".
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>

extern int peetdyn_counter;
extern __thread int peetdyn_tls;
int peetdyn_ctor_ran(void);
int peetdyn_add(int a, int b);
int *peetdyn_tls_addr(void);
const char *peetdyn_version(void);

#define CHILD_ARG "--child"
#define ABS_SELF "/mnt/ext2/bin/dynlink_smoke"

static int failures;

static void check(int cond, const char *what) {
    if (cond) {
        printf("[dynlink] %s ok\n", what);
    } else {
        printf("[dynlink] FAIL %s\n", what);
        failures++;
    }
}

#define MANY_MAPS 120

/* Map MANY_MAPS one-page regions with alternating protections, so adjacent
 * ones cannot merge, touch them, then unmap them all. */
static int many_mappings(void) {
    static void *maps[MANY_MAPS];
    int ok = 1;
    for (int i = 0; i < MANY_MAPS; i++) {
        int prot = (i & 1) ? PROT_READ : (PROT_READ | PROT_WRITE);
        maps[i] = mmap(NULL, 4096, prot, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (maps[i] == MAP_FAILED) {
            printf("[dynlink] mmap %d failed\n", i);
            ok = 0;
            for (int j = 0; j < i; j++) {
                munmap(maps[j], 4096);
            }
            return ok;
        }
        if (prot & PROT_WRITE) {
            *(volatile int *)maps[i] = i;
        }
    }
    for (int i = 0; i < MANY_MAPS; i += 2) {
        if (*(volatile int *)maps[i] != i) {
            ok = 0;
        }
    }
    for (int i = 0; i < MANY_MAPS; i++) {
        if (munmap(maps[i], 4096) != 0) {
            ok = 0;
        }
    }
    return ok;
}

/* RELRO-style: make the middle page of a 3-page mapping read-only, then
 * writable again, and check the data survives. */
static int mprotect_split_restore(void) {
    char *p = mmap(NULL, 3 * 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (p == MAP_FAILED) {
        return 0;
    }
    memset(p, 0x5a, 3 * 4096);
    int ok = mprotect(p + 4096, 4096, PROT_READ) == 0 &&
             mprotect(p + 4096, 4096, PROT_READ | PROT_WRITE) == 0;
    if (ok) {
        p[4096] = 1;
        ok = p[0] == 0x5a && p[4096] == 1 && p[2 * 4096] == 0x5a;
    }
    munmap(p, 3 * 4096);
    return ok;
}

#define BIG_MAP_MIB 192

/* Map and touch more memory than the kernel's old 128 MiB physical limit
 * (the runner gives QEMU 512 MiB), in 32 MiB pieces. */
static int big_allocation(void) {
    enum { CHUNKS = BIG_MAP_MIB / 32, CHUNK = 32 << 20 };
    unsigned char *chunks[CHUNKS];
    int ok = 1, mapped = 0;
    for (; mapped < CHUNKS; mapped++) {
        chunks[mapped] = mmap(NULL, CHUNK, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (chunks[mapped] == MAP_FAILED) {
            printf("[dynlink] big mmap chunk %d failed\n", mapped);
            ok = 0;
            break;
        }
        for (size_t off = 0; off < CHUNK; off += 4096) {
            chunks[mapped][off] = (unsigned char)(mapped + off / 4096);
        }
    }
    for (int i = 0; ok && i < mapped; i++) {
        for (size_t off = 0; off < CHUNK; off += 4096) {
            if (chunks[i][off] != (unsigned char)(i + off / 4096)) {
                ok = 0;
                break;
            }
        }
    }
    for (int i = 0; i < mapped; i++) {
        munmap(chunks[i], CHUNK);
    }
    return ok;
}

#define MANY_CHILDREN 100

/* MANY_CHILDREN processes alive at once, each blocked reading a pipe until
 * the parent releases them all. */
static int many_processes(void) {
    int go[2];
    if (pipe(go) != 0) {
        return 0;
    }
    pid_t pids[MANY_CHILDREN];
    int started = 0, ok = 1;
    for (; started < MANY_CHILDREN; started++) {
        pid_t pid = fork();
        if (pid == 0) {
            char c;
            close(go[1]);
            _exit(read(go[0], &c, 1) == 1 ? 0 : 1);
        }
        if (pid < 0) {
            printf("[dynlink] fork %d failed\n", started);
            ok = 0;
            break;
        }
        pids[started] = pid;
    }
    close(go[0]);
    char release[MANY_CHILDREN];
    memset(release, 'x', sizeof(release));
    if (write(go[1], release, (size_t)started) != started) {
        ok = 0;
    }
    close(go[1]);
    for (int i = 0; i < started; i++) {
        int status = 0;
        if (waitpid(pids[i], &status, 0) != pids[i] || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
            ok = 0;
        }
    }
    return ok;
}

#define MANY_SOCKETPAIRS 60

/* 2 * MANY_SOCKETPAIRS AF_UNIX sockets open at once, each pair passing a byte. */
static int many_sockets(void) {
    int sv[MANY_SOCKETPAIRS][2];
    int opened = 0, ok = 1;
    for (; opened < MANY_SOCKETPAIRS; opened++) {
        if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv[opened]) != 0) {
            printf("[dynlink] socketpair %d failed\n", opened);
            ok = 0;
            break;
        }
    }
    for (int i = 0; ok && i < opened; i++) {
        char out = (char)i, in = 0;
        if (write(sv[i][0], &out, 1) != 1 || read(sv[i][1], &in, 1) != 1 || in != out) {
            ok = 0;
        }
    }
    for (int i = 0; i < opened; i++) {
        close(sv[i][0]);
        close(sv[i][1]);
    }
    return ok;
}

struct thread_result {
    int lib_tls;
    int plugin_tls;
};

static int (*plugin_tls_bump)(void);

static void *thread_main(void *arg) {
    struct thread_result *r = arg;
    r->lib_tls = peetdyn_tls;               /* fresh copy: initial value 7 */
    peetdyn_tls = 1000;                      /* must not leak to main thread */
    r->plugin_tls = plugin_tls_bump();       /* fresh dynamic TLS: 5 -> 6 */
    return NULL;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    if (argc > 1 && strcmp(argv[1], CHILD_ARG) == 0) {
        printf("[dynlink] child running via %s\n", peetdyn_version());
        return peetdyn_add(20, 22) == 42 ? 0 : 3;
    }

    char *heap = malloc(64);
    snprintf(heap, 64, "libc.so %s", "works");
    check(strcmp(heap, "libc.so works") == 0, "libc.so stdio/malloc");
    free(heap);

    check(peetdyn_ctor_ran() == 1, "shared library constructor");
    check(peetdyn_add(2, 3) == 5, "shared library function call");
    check(peetdyn_counter == 41, "shared library exported data");
    peetdyn_counter++;
    check(peetdyn_counter == 42, "write to shared library data");
    check(peetdyn_tls == 7 && peetdyn_tls_addr() == &peetdyn_tls, "TLS in shared library");
    peetdyn_tls = 9;

    void *plugin = dlopen("libpeetplugin.so", RTLD_NOW | RTLD_LOCAL);
    if (plugin == NULL) {
        printf("[dynlink] FAIL dlopen: %s\n", dlerror());
        return 1;
    }
    int (*entry)(int) = (int (*)(int))dlsym(plugin, "plugin_entry");
    plugin_tls_bump = (int (*)(void))dlsym(plugin, "plugin_tls_bump");
    check(entry != NULL && plugin_tls_bump != NULL, "dlsym");
    check(entry != NULL && entry(5) == 105, "dlopen'd plugin calls into libpeetdyn");
    check(plugin_tls_bump != NULL && plugin_tls_bump() == 6, "dynamic TLS in dlopen'd plugin");

    check(dlopen("libdoes-not-exist.so", RTLD_NOW) == NULL && dlerror() != NULL,
          "dlopen of a missing library fails cleanly");

    struct thread_result r = {0, 0};
    pthread_t t;
    if (pthread_create(&t, NULL, thread_main, &r) != 0) {
        printf("[dynlink] FAIL pthread_create\n");
        return 1;
    }
    pthread_join(t, NULL);
    check(r.lib_tls == 7, "new thread gets fresh library TLS");
    check(r.plugin_tls == 6, "new thread gets fresh plugin TLS");
    check(peetdyn_tls == 9 && plugin_tls_bump() == 7, "main thread TLS unchanged by thread");

    check(many_mappings(), "120 distinct mappings alongside shared libraries");
    check(mprotect_split_restore(), "mprotect split and restore");
    check(big_allocation(), "192 MiB anonymous memory (beyond the old 128 MiB limit)");
    check(many_processes(), "100 concurrent child processes");
    check(many_sockets(), "120 open sockets");

    fflush(stdout);
    pid_t pid = fork();
    if (pid == 0) {
        char *const args[] = {argv[0], (char *)CHILD_ARG, NULL};
        execv(argv[0], args);
        execv(ABS_SELF, args);
        _exit(127);
    }
    int status = 0;
    waitpid(pid, &status, 0);
    check(WIFEXITED(status) && WEXITSTATUS(status) == 0, "fork + exec of a dynamic program");

    dlclose(plugin);
    if (failures != 0) {
        printf("[dynlink] failures=%d\n", failures);
        return 1;
    }
    printf("[dynlink] pass\n");
    return 0;
}
