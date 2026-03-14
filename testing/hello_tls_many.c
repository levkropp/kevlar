/* Test dynamic linking with multiple libraries including TLS + math + pthreads.
   Bridges the gap between hello_tls (1 TLS lib) and libsystemd (many deps + TLS).
   Links: libtlstest.so + libm + libpthread + libdl */
#include <dlfcn.h>
#include <math.h>
#include <pthread.h>
#include <unistd.h>

extern int tls_get(void);
extern int tls_inc(void);

int main(void) {
    /* Test TLS access */
    int val = tls_get();
    if (val != 42) {
        write(1, "FAIL: tls_get() != 42\n", 22);
        return 1;
    }

    /* Touch libm */
    double x = sqrt(144.0);
    if ((int)x != 12) {
        write(1, "FAIL: sqrt\n", 11);
        return 1;
    }

    /* Touch libpthread */
    pthread_t self = pthread_self();
    if (self == 0) {
        write(1, "FAIL: pthread_self\n", 19);
        return 1;
    }

    /* Touch libdl */
    void *handle = dlopen(NULL, RTLD_NOW);
    if (!handle) {
        write(1, "FAIL: dlopen\n", 13);
        return 1;
    }
    dlclose(handle);

    /* All passed */
    write(1, "hello from TLS+many test!\n", 26);
    return 0;
}
