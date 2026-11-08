/*
 * libpeetdyn.so: shared library for the dynamic-linking smoke test.
 * Exercises a constructor, exported data, exported functions, and a TLS
 * variable defined in a shared object (initial-exec-free, so the main program
 * reaches it through __tls_get_addr / the dynamic TLS model).
 */
static int ctor_ran;

__attribute__((constructor)) static void peetdyn_init(void) {
    ctor_ran = 1;
}

int peetdyn_counter = 41;
__thread int peetdyn_tls = 7;

int peetdyn_ctor_ran(void) {
    return ctor_ran;
}

int peetdyn_add(int a, int b) {
    return a + b;
}

int *peetdyn_tls_addr(void) {
    return &peetdyn_tls;
}

const char *peetdyn_version(void) {
    return "peetdyn 1";
}
