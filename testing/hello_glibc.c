#include <stdio.h>
#include <unistd.h>

int main(void) {
    printf("hello from glibc (pid=%d)\n", getpid());
    printf("TEST_PASS hello_glibc\n");
    printf("TEST_END 1/1\n");
    return 0;
}
