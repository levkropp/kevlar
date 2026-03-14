/* Test dynamic linking with a shared library that has TLS (__thread).
   This isolates TLS loading from the complexity of libsystemd-shared.
   If this works but libsystemd crashes, the issue is library-specific.
   If this also crashes, the issue is in basic TLS support. */
#include <unistd.h>

/* Defined in libtlstest.so */
extern int tls_get(void);
extern int tls_inc(void);

int main(void) {
    int val = tls_get();
    if (val != 42) {
        write(1, "FAIL: tls_get() != 42\n", 22);
        return 1;
    }

    int val2 = tls_inc();
    if (val2 != 43) {
        write(1, "FAIL: tls_inc() != 43\n", 22);
        return 1;
    }

    write(1, "hello from TLS test!\n", 21);
    return 0;
}
