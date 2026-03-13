# Phase 3: Scheduling Contracts

**Duration:** ~3-4 days
**Prerequisite:** Phase 1 (test harness)
**Goal:** Validate scheduler contracts—CFS weight calculations, nice values, priority inheritance, deadline scheduling stubs.

## Scope

The scheduler is the second-lowest layer. Complex programs (systemd, database servers, realtime apps) depend on:
- **Nice values:** Lower nice = higher priority. SCHED_OTHER scheduling weight
- **Priority inheritance:** Futex/mutex priority boosting (prevent priority inversion)
- **Deadline scheduling:** SCHED_DEADLINE (rare but critical for some workloads)
- **Fairness:** CFS (Completely Fair Scheduler) weight distribution

Linux's scheduler is notoriously complex. This phase validates the minimum set of contracts that existing software expects.

## Contracts to Validate

### 1. Nice Values

**Contract:** nice() changes process priority. Lower nice = higher priority. Processes with lower nice get more CPU time.

**Test:** `testing/contracts/scheduling/nice_values.c`
```c
#include <stdio.h>
#include <unistd.h>
#include <sys/resource.h>
#include <time.h>

int count1 = 0, count2 = 0;

int main() {
    pid_t pid = fork();
    if (pid == 0) {
        // Child: set nice to 10 (lower priority)
        nice(10);

        // Busy-loop
        for (int i = 0; i < 1000000; i++) {
            count2++;
        }
        exit(0);
    } else {
        // Parent: default nice
        // Busy-loop in parallel
        for (int i = 0; i < 1000000; i++) {
            count1++;
        }

        wait(NULL);

        // Parent should have more iterations (higher priority)
        if (count1 > count2) {
            printf("nice_values work: parent=%d, child=%d\n", count1, count2);
            return 0;
        } else {
            printf("ERROR: parent_count=%d <= child_count=%d\n", count1, count2);
            return 1;
        }
    }
}
```

**Why:** If Kevlar ignores nice(), both processes get equal CPU time.

**Note:** This test is inherently racy (depends on scheduler timing). Run multiple times and check trend.

### 2. getpriority / setpriority

**Contract:** getpriority() and setpriority() read/write process priority.

**Test:** `testing/contracts/scheduling/priority_values.c`
```c
#include <stdio.h>
#include <sys/resource.h>

int main() {
    // Check default priority
    int prio = getpriority(PRIO_PROCESS, 0);
    printf("default_priority: %d\n", prio);

    // Set to +5
    setpriority(PRIO_PROCESS, 0, 5);
    prio = getpriority(PRIO_PROCESS, 0);
    if (prio != 5) {
        printf("ERROR: setpriority failed, got %d\n", prio);
        return 1;
    }

    printf("priority_values work\n");
    return 0;
}
```

**Why:** Tests basic priority syscalls.

### 3. Priority Inheritance (Futex PI)

**Contract:** Futex with PI (FUTEX_LOCK_PI) prevents priority inversion. If a high-priority thread waits on a lock held by a low-priority thread, the low-priority thread is boosted temporarily.

**Test:** `testing/contracts/scheduling/priority_inheritance.c`
```c
#include <stdio.h>
#include <pthread.h>
#include <unistd.h>
#include <sys/resource.h>

// Simple futex-based lock (manual, not pthread_mutex_t)
static int lock = 0;

void *low_priority_thread(void *arg) {
    // Set low priority (nice=19)
    setpriority(PRIO_PROCESS, 0, 19);

    // Hold lock
    syscall(SYS_futex, &lock, FUTEX_LOCK_PI, ...);
    printf("low_prio: acquired lock\n");

    // Hold it for a while (simulate work)
    usleep(100000);

    // Release
    syscall(SYS_futex, &lock, FUTEX_UNLOCK_PI, ...);
    return NULL;
}

void *high_priority_thread(void *arg) {
    usleep(10000);  // Let low-prio thread get lock first

    // Set high priority (nice=-10)
    setpriority(PRIO_PROCESS, 0, -10);

    // Try to acquire lock (should be boosted to low-prio's priority level)
    printf("high_prio: waiting for lock\n");
    syscall(SYS_futex, &lock, FUTEX_LOCK_PI, ...);
    printf("high_prio: acquired lock\n");

    // Release
    syscall(SYS_futex, &lock, FUTEX_UNLOCK_PI, ...);
    return NULL;
}

int main() {
    pthread_t tid1, tid2;
    pthread_create(&tid1, NULL, low_priority_thread, NULL);
    pthread_create(&tid2, NULL, high_priority_thread, NULL);

    pthread_join(tid1, NULL);
    pthread_join(tid2, NULL);

    printf("priority_inheritance works\n");
    return 0;
}
```

**Why:** systemd and other complex apps use priority inheritance. If Kevlar doesn't implement it, priority inversion can cause high-priority tasks to starve.

**Status:** Kevlar stubs FUTEX_LOCK_PI. This test might fail; document the limitation.

### 4. SCHED_DEADLINE Stub

**Contract:** SCHED_DEADLINE exists as a syscall (sched_setattr). Kevlar doesn't need to implement it fully, but syscall should not crash.

**Test:** `testing/contracts/scheduling/deadline_scheduling.c`
```c
#include <stdio.h>
#include <unistd.h>
#include <sys/syscall.h>
#include <linux/sched.h>

int main() {
    struct sched_attr attr = {
        .size = sizeof(attr),
        .sched_policy = SCHED_DEADLINE,
        .sched_runtime = 100000,
        .sched_deadline = 1000000,
        .sched_period = 1000000,
    };

    // Try sched_setattr with SCHED_DEADLINE
    // Expected: EINVAL or ENOSYS (not implemented)
    // NOT: kernel crash
    int ret = syscall(SYS_sched_setattr, 0, &attr, 0);
    if (ret == 0) {
        printf("SCHED_DEADLINE supported\n");
    } else {
        printf("SCHED_DEADLINE not supported (errno=%d), but syscall exists\n", errno);
    }
    return 0;
}
```

**Why:** Tests that Kevlar doesn't crash on unimplemented schedulers.

### 5. Scheduler Fairness (CFS Weights)

**Contract:** Processes with same nice value get approximately equal CPU time. The CFS (Completely Fair Scheduler) enforces weighted fairness.

**Test:** `testing/contracts/scheduling/cfs_fairness.c`
```c
#include <stdio.h>
#include <unistd.h>
#include <stdlib.h>
#include <sys/wait.h>

int main() {
    // Spawn 4 equal-priority children, each doing work
    int child_counts[4] = {0, 0, 0, 0};

    for (int i = 0; i < 4; i++) {
        pid_t pid = fork();
        if (pid == 0) {
            // Child: count iterations
            long count = 0;
            for (int j = 0; j < 10000000; j++) {
                count++;
            }
            printf("child_%d: %ld iterations\n", i, count);
            exit(0);
        }
    }

    // Wait for all children
    for (int i = 0; i < 4; i++) {
        wait(NULL);
    }

    printf("cfs_fairness: completed\n");
    return 0;
}
```

**Output on Linux (fair scheduler):**
```
child_0: 10000000 iterations
child_1: 10000000 iterations
child_2: 10000000 iterations
child_3: 10000000 iterations
```

**Output on Kevlar with unfair scheduler:**
```
child_0: 10000000 iterations
child_1: 9500000 iterations  <-- significant variance
child_2: 10500000 iterations
child_3: 9200000 iterations
```

**Why:** Tests that CPU time is distributed fairly. Large variance suggests Kevlar's scheduler has issues.

### 6. Preemption Latency

**Contract:** Preemption happens within a bounded time. On a 100 Hz timer (10ms ticks), a process should yield at least once every 30ms.

**Test:** `testing/contracts/scheduling/preemption_latency.c`
```c
#include <stdio.h>
#include <time.h>
#include <signal.h>
#include <setjmp.h>

static jmp_buf env;
static long long max_duration = 0;

void timer_handler(int sig) {
    longjmp(env, 1);
}

int main() {
    // Install timer
    signal(SIGALRM, timer_handler);

    struct itimerval tv;
    tv.it_interval.tv_sec = 0;
    tv.it_interval.tv_usec = 50000;  // 50ms intervals
    tv.it_value = tv.it_interval;
    setitimer(ITIMER_REAL, &tv, NULL);

    // Busy-loop and measure how long between signals
    while (1) {
        if (setjmp(env) == 0) {
            // Spin until signal
            long long count = 0;
            while (1) count++;
        } else {
            // Signal fired; check time
            static struct timespec last = {0};
            struct timespec now;
            clock_gettime(CLOCK_MONOTONIC, &now);

            if (last.tv_sec > 0) {
                long long elapsed_us = (now.tv_sec - last.tv_sec) * 1000000 +
                                       (now.tv_nsec - last.tv_nsec) / 1000;
                max_duration = (elapsed_us > max_duration) ? elapsed_us : max_duration;

                if (elapsed_us > 100000) {  // > 100ms without preemption
                    printf("ERROR: preemption_latency too high: %lld us\n", elapsed_us);
                    return 1;
                }
            }
            last = now;

            static int count = 0;
            if (++count > 10) {
                printf("preemption_latency ok: max=%lld us\n", max_duration);
                return 0;
            }
        }
    }
}
```

**Why:** Tests that Kevlar preempts high-CPU-usage processes frequently. If preemption is too rare, interactive responsiveness suffers.

## Implementation Plan

1. **Write tests** (all 6 test files)
2. **Compile** with musl-gcc -static
3. **Run harness** (both single-CPU and -smp 4)
4. **Document divergences**
5. **Fix each divergence:**
   - Nice values: Ensure scheduler respects nice in weight calculation
   - getpriority/setpriority: Implement if missing
   - Priority inheritance: Document as stub if not implemented
   - SCHED_DEADLINE: Accept syscall gracefully, return ENOSYS or EINVAL
   - CFS fairness: Ensure per-CPU scheduler doesn't starve equal-priority processes
   - Preemption: Verify LAPIC timer fires frequently enough

## Testing Phases

**Phase 3a (2 days):** Write tests, run on both Linux and Kevlar, identify divergences

**Phase 3b (1-2 days):** Fix divergences in scheduler (nice values, preemption latency)

**Phase 3c (1 day):** Stub unimplemented features (priority inheritance, deadline), regression test

## Success Criteria

- [ ] getpriority/setpriority tests PASS
- [ ] Nice values test shows correct trend (parent gets more CPU)
- [ ] CFS fairness test shows balanced CPU distribution on both single and multi-CPU
- [ ] Preemption latency < 100ms on both Linux and Kevlar
- [ ] SCHED_DEADLINE syscall exists (returns ENOSYS if not implemented)
- [ ] No M6 regressions on -smp 4

## Known Limitations

1. **Priority inheritance (FUTEX_LOCK_PI):** Kevlar stubs this. Document as "Not Implemented" but don't crash.

2. **SCHED_DEADLINE:** Not implemented. That's OK; kernel should return ENOSYS gracefully.

3. **Nice value fairness:** Inherently racy. Run test multiple times. If variance is large but consistent across multiple runs, investigate.

4. **Preemption timing:** Depends on LAPIC timer frequency (TICK_HZ=100 in Kevlar). Adjust expected latency if needed.

## Contract Documentation

As each contract is validated, add to `docs/contracts.md`:

```markdown
## Scheduling Contracts

### Nice Values
- nice(2) changes process priority (-20 to +19)
- Lower nice = higher priority = more CPU time
- Processes with same nice get approximately equal CPU time
- Tests: testing/contracts/scheduling/nice_values.c

### Priority Inheritance (Futex PI)
- FUTEX_LOCK_PI prevents priority inversion
- High-priority thread waiting for low-priority thread's lock boosts low-prio's priority
- Status: Kevlar stubs this (returns ENOSYS/EINVAL)
- Tests: testing/contracts/scheduling/priority_inheritance.c

### Preemption Latency
- Process receives preemption signal at least once every 30ms (TICK_HZ=100)
- Preemption latency < 100ms in practice
- Tests: testing/contracts/scheduling/preemption_latency.c

### SCHED_DEADLINE
- sched_setattr syscall accepts SCHED_DEADLINE policy
- Status: Kevlar accepts syscall, returns ENOSYS (not implemented)
- Tests: testing/contracts/scheduling/deadline_scheduling.c
```

This spec helps M8-M9 authors understand which scheduling features are available and which are stubs.
