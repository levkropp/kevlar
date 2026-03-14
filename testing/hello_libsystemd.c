/* Test dynamic linking with libsystemd-shared-245.so specifically.
   This is the library that causes systemd to crash. If this works,
   the crash is in systemd's code. If this crashes, the issue is in
   loading this specific library. */
#include <stdio.h>
#include <unistd.h>
#include <dlfcn.h>

int main(void) {
    /* Try to dlopen libsystemd-shared */
    void *handle = dlopen("libsystemd-shared-245.so", RTLD_NOW);
    if (handle) {
        write(1, "libsystemd-shared loaded ok!\n", 29);
        dlclose(handle);
    } else {
        /* dlerror might not work but try anyway */
        write(1, "libsystemd-shared load failed\n", 30);
    }

    write(1, "hello from libsystemd test!\n", 27);
    return 0;
}
