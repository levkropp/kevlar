# Phase 4: Signal Contracts

**Duration:** ~3 days
**Prerequisite:** Phase 1 (test harness)
**Goal:** Validate signal delivery contracts—delivery order, mask semantics, coredump format, signal safety.

## Scope

Signals are the event delivery mechanism in Linux. Programs (especially daemons like systemd) depend on:
- **Signal delivery order:** Real-time signals (SIGRTMIN+N) are queued and delivered in order. Standard signals are not.
- **Signal masking:** sigprocmask/pthread_sigmask block signals; they're delivered after unmasking.
- **Coredump format:** SIGSEGV produces a coredump in ELF format with memory map and register state.
- **Signal handlers:** sigaction() registers handlers; signal delivery invokes them.
- **Signal safety:** Only async-signal-safe functions can be called from signal handlers.

## Contracts to Validate

### 1. Signal Delivery Order

**Contract:** Standard signals (SIGTERM, SIGINT, etc.) are coalesced; only one delivery if sent multiple times. Real-time signals (SIGRTMIN+N) are queued.

**Test:** `testing/contracts/signals/delivery_order.c`
```c
#include <stdio.h>
#include <signal.h>
#include <unistd.h>
#include <stdlib.h>

static int sigterm_count = 0;
static int sigrtmin_count = 0;

void sigterm_handler(int sig) {
    sigterm_count++;
    write(STDOUT_FILENO, "SIGTERM\n", 8);
}

void sigrtmin_handler(int sig) {
    sigrtmin_count++;
    write(STDOUT_FILENO, "SIGRTMIN\n", 9);
}

int main() {
    signal(SIGTERM, sigterm_handler);
    signal(SIGRTMIN, sigrtmin_handler);

    // Send SIGTERM 5 times rapidly
    for (int i = 0; i < 5; i++) {
        kill(getpid(), SIGTERM);
    }

    // Send SIGRTMIN 5 times rapidly
    for (int i = 0; i < 5; i++) {
        kill(getpid(), SIGRTMIN);
    }

    // Wait a bit for signals to be delivered
    sleep(1);

    printf("SIGTERM delivered: %d times (expected: 1)\n", sigterm_count);
    printf("SIGRTMIN delivered: %d times (expected: 5)\n", sigrtmin_count);

    if (sigterm_count == 1 && sigrtmin_count == 5) {
        printf("delivery_order works\n");
        return 0;
    } else {
        return 1;
    }
}
```

**Why:** Tests standard vs real-time signal queueing. If Kevlar treats all signals the same, sigrtmin_count will be 1 (coalesced).

### 2. Signal Masking

**Contract:** sigprocmask() blocks signal delivery. Signals are queued while masked, delivered after unmasking.

**Test:** `testing/contracts/signals/mask_semantics.c`
```c
#include <stdio.h>
#include <signal.h>
#include <unistd.h>

static int sigterm_count = 0;

void sigterm_handler(int sig) {
    sigterm_count++;
}

int main() {
    signal(SIGTERM, sigterm_handler);

    // Block SIGTERM
    sigset_t set;
    sigemptyset(&set);
    sigaddset(&set, SIGTERM);
    sigprocmask(SIG_BLOCK, &set, NULL);

    // Send SIGTERM (should be blocked)
    kill(getpid(), SIGTERM);

    // Handler should NOT have run yet
    if (sigterm_count != 0) {
        printf("ERROR: signal delivered while blocked\n");
        return 1;
    }

    // Unblock SIGTERM
    sigprocmask(SIG_UNBLOCK, &set, NULL);

    // Handler should run now
    sleep(1);

    if (sigterm_count == 1) {
        printf("mask_semantics works\n");
        return 0;
    } else {
        printf("ERROR: handler count = %d (expected 1)\n", sigterm_count);
        return 1;
    }
}
```

**Why:** Tests that sigprocmask() actually blocks/unblocks signals.

### 3. Coredump Format

**Contract:** SIGSEGV produces a coredump in ELF core format with memory map, registers, and note sections.

**Test:** `testing/contracts/signals/coredump_format.c`
```c
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <signal.h>
#include <sys/resource.h>

int main() {
    // Enable coredumps
    struct rlimit rl;
    rl.rlim_cur = RLIM_INFINITY;
    rl.rlim_max = RLIM_INFINITY;
    setrlimit(RLIMIT_CORE, &rl);

    // Trigger SIGSEGV
    char *p = (char *)0xdeadbeef;
    *p = 'A';

    return 0;
}
```

**Expected behavior:**
1. Process crashes with SIGSEGV
2. core dump file is written to current directory
3. `file core` shows "ELF 64-bit LSB core file"
4. `readelf -l core` shows PT_LOAD segments for memory map
5. `readelf -n core` shows NT_PRPSINFO, NT_PRSTATUS notes

**Why:** Debuggers (gdb, lldb) read coredumps. If format is wrong, debugging is broken.

**Status:** Kevlar doesn't implement coredumps yet. Document as "Not Implemented" and skip for now. Coredumps are needed for M9 (systemd with crash dumps).

### 4. Signal Handler Context

**Contract:** Signal handler receives signal number, sigaction() stores handler, SA_RESTART affects syscall behavior.

**Test:** `testing/contracts/signals/handler_context.c`
```c
#include <stdio.h>
#include <signal.h>
#include <unistd.h>

static int handler_sig = 0;

void handler(int sig) {
    handler_sig = sig;
}

int main() {
    struct sigaction sa;
    sa.sa_handler = handler;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = 0;
    sigaction(SIGUSR1, &sa, NULL);

    // Send SIGUSR1
    kill(getpid(), SIGUSR1);

    // Handler should have run
    sleep(1);

    if (handler_sig == SIGUSR1) {
        printf("handler_context works\n");
        return 0;
    } else {
        printf("ERROR: handler not called (sig=%d)\n", handler_sig);
        return 1;
    }
}
```

**Why:** Tests basic signal handler registration and delivery.

### 5. SA_RESTART Behavior

**Contract:** If signal handler returns while blocking syscall is in progress:
- Without SA_RESTART: syscall returns EINTR
- With SA_RESTART: syscall resumes

**Test:** `testing/contracts/signals/sa_restart.c`
```c
#include <stdio.h>
#include <signal.h>
#include <unistd.h>
#include <errno.h>

static int handler_called = 0;

void handler(int sig) {
    handler_called = 1;
}

int main() {
    struct sigaction sa;
    sa.sa_handler = handler;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = SA_RESTART;  // Enable SA_RESTART
    sigaction(SIGUSR1, &sa, NULL);

    // Fork child to send signal
    pid_t pid = fork();
    if (pid == 0) {
        sleep(1);
        kill(getppid(), SIGUSR1);
        exit(0);
    }

    // Parent: call blocking syscall (read)
    char buf[1024];
    int ret = read(STDIN_FILENO, buf, sizeof(buf));

    if (ret == -1 && errno == EINTR) {
        printf("ERROR: read returned EINTR (SA_RESTART not working)\n");
        return 1;
    } else if (ret > 0) {
        printf("read restarted after signal\n");
        return 0;
    } else {
        printf("read failed: %d\n", ret);
        return 1;
    }
}
```

**Why:** Tests that SA_RESTART prevents EINTR from blocking syscalls.

### 6. Real-time Signal Arguments

**Contract:** Real-time signals can carry a 32-bit value via sigqueue(). Signal handler receives value in si_value.

**Test:** `testing/contracts/signals/sigqueue.c`
```c
#include <stdio.h>
#include <signal.h>
#include <unistd.h>
#include <string.h>

static int received_value = 0;

void handler(int sig, siginfo_t *info, void *ctx) {
    if (info) {
        received_value = info->si_value.sival_int;
    }
}

int main() {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_sigaction = handler;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = SA_SIGINFO;
    sigaction(SIGRTMIN, &sa, NULL);

    // Send real-time signal with value
    union sigval value;
    value.sival_int = 0x12345678;
    sigqueue(getpid(), SIGRTMIN, value);

    sleep(1);

    if (received_value == 0x12345678) {
        printf("sigqueue works\n");
        return 0;
    } else {
        printf("ERROR: received value = 0x%x (expected 0x12345678)\n", received_value);
        return 1;
    }
}
```

**Why:** Tests real-time signal payload delivery. Needed for IPC.

## Implementation Plan

1. **Write tests** (all 6 test files)
2. **Compile** with musl-gcc -static
3. **Run harness**
4. **Document divergences**
5. **Fix each divergence:**
   - Delivery order: Ensure real-time signals are queued, standard signals coalesced
   - Masking: Verify sigprocmask blocks/unblocks correctly
   - Coredump: Stub for now (M9 requirement)
   - Handler context: Ensure sigaction and handler invocation work
   - SA_RESTART: Implement in syscall return paths
   - sigqueue: Ensure real-time signals carry values

## Testing Phases

**Phase 4a (1 day):** Write tests, run on Linux and Kevlar

**Phase 4b (1-2 days):** Fix divergences (masking, delivery order, SA_RESTART)

**Phase 4c (1 day):** Stub unimplemented features (coredumps, sigqueue), regression test

## Success Criteria

- [ ] Signal masking test PASS
- [ ] Delivery order test shows standard signals coalesced, real-time queued (or documented limitation)
- [ ] Handler context test PASS
- [ ] SA_RESTART test PASS (or documented limitation)
- [ ] sigqueue test PASS (or documented as real-time signal stub)
- [ ] Coredump documented as "Not Implemented for M6.5"
- [ ] No M6 regressions

## Known Limitations

1. **Coredumps:** Not implemented. Defer to M9. Kernel should still deliver signal correctly, just no dump file.

2. **Real-time signal queueing:** Might be partially implemented. Test carefully.

3. **sigqueue:** Stubs might not carry values. Document behavior.

4. **Signal handler atomicity:** Kevlar might have edge cases with signal delivery during syscalls. Run tests with -smp 4 to catch race conditions.

## Contract Documentation

As each contract is validated, add to `docs/contracts.md`:

```markdown
## Signal Contracts

### Signal Masking
- sigprocmask(2) / pthread_sigmask(3) blocks signal delivery
- Signals are queued while masked, delivered after unmasking
- Tests: testing/contracts/signals/mask_semantics.c

### Signal Delivery Order
- Standard signals (SIGTERM, SIGINT, etc.) are coalesced
- Real-time signals (SIGRTMIN+N) are queued and delivered in order
- Status: Standard signal coalescing verified; real-time queueing TBD
- Tests: testing/contracts/signals/delivery_order.c

### Signal Handler Registration
- sigaction(2) registers signal handlers
- Handlers are invoked synchronously on signal delivery
- Tests: testing/contracts/signals/handler_context.c

### SA_RESTART
- If SA_RESTART flag is set, blocking syscalls resume after signal handler returns
- Without SA_RESTART, syscalls return EINTR
- Tests: testing/contracts/signals/sa_restart.c

### Coredumps
- SIGSEGV produces an ELF core dump file with memory map and registers
- Status: Not implemented in Kevlar yet (M9 requirement)
- Tests: testing/contracts/signals/coredump_format.c (expected to fail)

### Real-time Signal Arguments
- sigqueue(2) sends real-time signal with a value
- Handler receives value in siginfo_t.si_value
- Status: Kevlar stubs this (might not carry values)
- Tests: testing/contracts/signals/sigqueue.c
```

This spec helps M8-M9 authors understand signal behavior and what features are available for systemd.
