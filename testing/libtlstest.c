/* Simple shared library with a TLS variable.
   Used to test that glibc's ld.so correctly handles PT_TLS segments
   during dynamic linking. If this library loads and tls_get() works,
   basic TLS support is functional. */

__thread int tls_counter = 42;

int tls_get(void) {
    return tls_counter;
}

int tls_inc(void) {
    return ++tls_counter;
}
