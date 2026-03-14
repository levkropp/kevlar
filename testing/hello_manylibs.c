/* Test dynamic linking with many shared libraries (similar count to systemd).
   Links against: libc, libm, libpthread, librt, libdl, libcap */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <math.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

int main(void) {
    /* Touch various libraries to ensure they're loaded */
    double x = sqrt(144.0);
    pthread_t self = pthread_self();
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    void *handle = dlopen(NULL, RTLD_NOW);

    /* Also touch librt via clock_getres */
    struct timespec res;
    clock_getres(CLOCK_MONOTONIC, &res);

    int ok = ((int)x == 12) && (self != 0) && (ts.tv_sec >= 0)
             && (handle != NULL) && (res.tv_nsec > 0);
    if (ok) {
        write(1, "hello from manylibs glibc!\n", 26);
    }
    if (handle) dlclose(handle);
    return ok ? 0 : 1;
}
