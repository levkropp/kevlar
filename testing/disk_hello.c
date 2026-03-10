// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// disk_hello: tiny static binary for the exec-from-disk integration test.
// Built with: musl-gcc -static -O2 -o disk_hello disk_hello.c
#include <unistd.h>
int main(void) {
    const char msg[] = "hello from disk!\n";
    write(1, msg, sizeof(msg) - 1);
    return 0;
}
