/* Contract: sigaltstack + SA_ONSTACK runs handler on alt stack. */
#include <errno.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static void *alt_stack_base;
static size_t alt_stack_size;
static volatile int handler_on_altstack = 0;

static void handler(int sig) {
    (void)sig;
    int local;
    uintptr_t sp = (uintptr_t)&local;
    uintptr_t base = (uintptr_t)alt_stack_base;
    if (sp >= base && sp < base + alt_stack_size) {
        handler_on_altstack = 1;
    }
}

int main(void) {
    /* Allocate alternate signal stack */
    alt_stack_size = SIGSTKSZ;
    alt_stack_base = malloc(alt_stack_size);
    if (!alt_stack_base) {
        printf("CONTRACT_FAIL malloc: errno=%d\n", errno);
        return 1;
    }

    stack_t ss = {0};
    ss.ss_sp = alt_stack_base;
    ss.ss_size = alt_stack_size;
    ss.ss_flags = 0;

    if (sigaltstack(&ss, NULL) != 0) {
        printf("CONTRACT_FAIL sigaltstack: errno=%d\n", errno);
        return 1;
    }
    printf("sigaltstack: ok\n");

    /* Install handler with SA_ONSTACK */
    struct sigaction sa = {0};
    sa.sa_handler = handler;
    sa.sa_flags = SA_ONSTACK;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGUSR1, &sa, NULL);

    /* Raise signal */
    raise(SIGUSR1);

    if (!handler_on_altstack) {
        printf("CONTRACT_FAIL handler_altstack: ran on normal stack\n");
        free(alt_stack_base);
        return 1;
    }
    printf("handler_altstack: ok\n");
    printf("CONTRACT_PASS\n");

    free(alt_stack_base);
    return 0;
}
