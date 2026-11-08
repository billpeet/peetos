/*
 * libpeetplugin.so: loaded at run time with dlopen() by the dynamic-linking
 * smoke test. It depends on libpeetdyn.so (resolved by the loader) and has its
 * own TLS variable, which needs a dynamically allocated TLS block because the
 * module is loaded after startup.
 */
extern int peetdyn_add(int a, int b);

__thread int plugin_tls = 5;

int plugin_entry(int x) {
    return peetdyn_add(x, 100);
}

int plugin_tls_bump(void) {
    return ++plugin_tls;
}
