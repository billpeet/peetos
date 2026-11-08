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
 *
 * Output: "[dynlink] <check> ok", "[dynlink] FAIL ...", and "[dynlink] pass".
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
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
