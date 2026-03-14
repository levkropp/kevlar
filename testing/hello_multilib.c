/* Test dynamic linking with multiple shared libraries.
   Links against libm and libpthread to test multi-library relocation. */
#define _GNU_SOURCE
#include <math.h>
#include <pthread.h>
#include <stdio.h>
#include <unistd.h>

int main(void) {
    /* Use libm */
    double x = sqrt(144.0);

    /* Use libpthread */
    pthread_t self = pthread_self();

    write(1, "hello from multilib glibc!\n", 26);
    return (int)x == 12 && self != 0 ? 0 : 1;
}
