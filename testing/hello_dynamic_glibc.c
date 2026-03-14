#include <stdio.h>
#include <unistd.h>

int main(void) {
    write(1, "hello from dynamic glibc!\n", 25);
    return 0;
}
