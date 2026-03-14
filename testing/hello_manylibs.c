/* Test dynamic linking with many shared libraries (similar count to systemd).
   Links against: libc, libm, libpthread, librt, libdl, libcap, libmount,
   libselinux, libblkid, libuuid, libcrypt, libgcrypt */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <math.h>
#include <mntent.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <sys/mount.h>
#include <uuid/uuid.h>

int main(void) {
    /* Touch various libraries to ensure they're loaded */
    double x = sqrt(144.0);
    pthread_t self = pthread_self();
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    uuid_t uuid;
    uuid_generate(uuid);
    void *handle = dlopen(NULL, RTLD_NOW);

    int ok = ((int)x == 12) && (self != 0) && (ts.tv_sec >= 0) && (handle != NULL);
    if (ok) {
        write(1, "hello from manylibs glibc!\n", 26);
    }
    if (handle) dlclose(handle);
    return ok ? 0 : 1;
}
