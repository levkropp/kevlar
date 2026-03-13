# Phase 2: VM Contracts

**Duration:** ~4-5 days
**Prerequisite:** Phase 1 (test harness)
**Goal:** Validate memory management contracts—demand paging, page cache, page flags, TLB semantics, fork() copy-on-write.

## Scope

Memory management is the lowest layer. GPU drivers, systemd, and complex programs all depend on:
- **Demand paging:** Pages loaded on demand, not pre-allocated
- **Copy-on-write:** fork() doesn't copy pages; shared pages break on write
- **Page flags:** Dirty, referenced, locked, etc.
- **TLB semantics:** Invalidation order, coherency after TLB shootdown
- **mmap behavior:** MAP_SHARED vs MAP_PRIVATE, protection changes (mprotect)

These are enforced by the hardware (x86/ARM TLB, page table). Linux has specific policies for managing them.

## Contracts to Validate

### 1. Demand Paging

**Contract:** Pages are allocated lazily on fault, not eagerly on mmap.

**Test:** `testing/contracts/vm/demand_paging.c`
```c
#include <stdio.h>
#include <unistd.h>
#include <sys/mman.h>

int main() {
    // Allocate 1MB, but don't touch
    char *p = mmap(NULL, 1024*1024, PROT_READ|PROT_WRITE,
                   MAP_PRIVATE|MAP_ANONYMOUS, -1, 0);

    // Check RSS before and after
    // (use /proc/self/stat: VmRSS field)
    // Before touch: ~0 KB
    // After touch: ~1 MB

    printf("mmap done\n");
    return 0;
}
```

**Why:** If Kevlar pre-allocates on mmap(), RSS will spike immediately. Linux lazily faults pages in.

### 2. Copy-on-Write (fork)

**Contract:** fork() creates a new process with the same address space, but pages are shared read-only until one process writes.

**Test:** `testing/contracts/vm/fork_cow.c`
```c
#include <stdio.h>
#include <unistd.h>
#include <sys/wait.h>

int global = 0x12345678;

int main() {
    pid_t pid = fork();
    if (pid == 0) {
        // Child: read global
        printf("child_read: 0x%x\n", global);

        // Child: modify global
        global = 0x87654321;
        printf("child_modified: 0x%x\n", global);
        exit(0);
    } else {
        // Parent: wait for child to modify
        wait(NULL);

        // Parent should still see original value
        printf("parent_sees: 0x%x\n", global);
        if (global != 0x12345678) {
            printf("ERROR: parent's global was modified by child\n");
            return 1;
        }
        return 0;
    }
}
```

**Expected output:**
```
child_read: 0x12345678
child_modified: 0x87654321
parent_sees: 0x12345678
```

**Why:** If Kevlar doesn't implement CoW properly, parent's global will be 0x87654321 (both see same page).

### 3. mprotect Semantics

**Contract:** mprotect() changes page protection atomically. Subsequent accesses see new permissions.

**Test:** `testing/contracts/vm/mprotect.c`
```c
#include <stdio.h>
#include <unistd.h>
#include <signal.h>
#include <setjmp.h>
#include <sys/mman.h>

static jmp_buf env;
void segv_handler(int sig) {
    longjmp(env, 1);
}

int main() {
    // Allocate page, make it PROT_NONE
    char *p = mmap(NULL, 4096, PROT_READ|PROT_WRITE,
                   MAP_PRIVATE|MAP_ANONYMOUS, -1, 0);
    mprotect(p, 4096, PROT_NONE);

    // Try to read: should SIGSEGV
    signal(SIGSEGV, segv_handler);
    if (setjmp(env) == 0) {
        char x = p[0];  // Should fault
        printf("ERROR: read succeeded when PROT_NONE\n");
        return 1;
    }

    // Now make it PROT_READ
    mprotect(p, 4096, PROT_READ);
    char x = p[0];  // Should succeed
    printf("mprotect works\n");
    return 0;
}
```

**Why:** Tests that permission changes are enforced by TLB/page table.

### 4. TLB Shootdown Ordering

**Contract:** After TLB shootdown, all CPUs see the new page table state. Memory barriers are inserted correctly.

**Test:** `testing/contracts/vm/tlb_shootdown.c`
```c
#include <stdio.h>
#include <pthread.h>
#include <string.h>
#include <unistd.h>
#include <sys/mman.h>

char *shared_page;
int flag = 0;

void *thread_func(void *arg) {
    // Spin until page is readable
    while (1) {
        char x = shared_page[0];
        if (x == 'A') {
            flag = 1;
            break;
        }
        usleep(1);
    }
    return NULL;
}

int main() {
    // Create shared page, protected PROT_NONE
    shared_page = mmap(NULL, 4096, PROT_READ|PROT_WRITE,
                       MAP_PRIVATE|MAP_ANONYMOUS, -1, 0);
    mprotect(shared_page, 4096, PROT_NONE);

    // Start thread that waits for page to become readable
    pthread_t tid;
    pthread_create(&tid, NULL, thread_func, NULL);
    usleep(100);  // Let thread start spinning

    // Change protection to PROT_READ and write
    mprotect(shared_page, 4096, PROT_READ|PROT_WRITE);
    shared_page[0] = 'A';
    mprotect(shared_page, 4096, PROT_READ);

    // Wait for thread to see the change
    for (int i = 0; i < 100; i++) {
        if (flag) {
            printf("tlb_shootdown works\n");
            pthread_join(tid, NULL);
            return 0;
        }
        usleep(10);
    }

    printf("ERROR: thread never saw the write\n");
    return 1;
}
```

**Why:** If Kevlar's TLB shootdown doesn't properly synchronize, the other thread might not see the mprotect() change.

### 5. mmap Hint Flags

**Contract:** Hint flags like MAP_STACK, MAP_GROWSDOWN, MAP_HUGETLB are optional; kernel can ignore them or implement them.

**Test:** `testing/contracts/vm/mmap_hints.c`
```c
#include <stdio.h>
#include <sys/mman.h>

int main() {
    // These should all succeed (even if hints are ignored)
    char *p1 = mmap(NULL, 4096, PROT_READ|PROT_WRITE,
                    MAP_PRIVATE|MAP_ANONYMOUS|MAP_STACK, -1, 0);
    if (p1 == MAP_FAILED) {
        printf("MAP_STACK failed\n");
        return 1;
    }

    char *p2 = mmap(NULL, 4096, PROT_READ|PROT_WRITE,
                    MAP_PRIVATE|MAP_ANONYMOUS|MAP_GROWSDOWN, -1, 0);
    if (p2 == MAP_FAILED) {
        printf("MAP_GROWSDOWN failed\n");
        return 1;
    }

    printf("mmap hints work\n");
    return 0;
}
```

**Why:** glibc uses these hints. If Kevlar returns EINVAL, glibc falls back, but it's slower.

### 6. File Mapping (mmap with file)

**Contract:** mmap with a file descriptor maps file contents. Changes to the file are visible if MAP_SHARED.

**Test:** `testing/contracts/vm/mmap_file.c`
```c
#include <stdio.h>
#include <unistd.h>
#include <fcntl.h>
#include <sys/mman.h>

int main() {
    // Create temp file with known content
    int fd = open("/tmp/testfile", O_CREAT|O_RDWR, 0644);
    write(fd, "Hello", 5);
    fsync(fd);

    // Map it
    char *p = mmap(NULL, 5, PROT_READ, MAP_SHARED, fd, 0);
    if (p == MAP_FAILED) {
        printf("mmap failed\n");
        return 1;
    }

    // Read mapped content
    char buf[6] = {0};
    memcpy(buf, p, 5);
    if (strcmp(buf, "Hello") != 0) {
        printf("mmap content mismatch: %s\n", buf);
        return 1;
    }

    printf("mmap_file works\n");
    munmap(p, 5);
    close(fd);
    return 0;
}
```

**Why:** Kevlar's mmap might not properly integrate with the filesystem.

## Implementation Plan

1. **Write tests** (all 6 test files)
2. **Compile** with musl-gcc -static
3. **Run harness** from Phase 1
4. **Document divergences** in contract-results.json
5. **Fix each divergence** in Kevlar
   - Demand paging: Ensure mmap doesn't allocate if not needed
   - CoW: Verify fork() sets pages read-only, handles write faults
   - mprotect: Check page table and TLB are updated atomically
   - TLB shootdown: Verify IPIs complete before mprotect() returns
   - Hints: Accept all hint flags gracefully
   - File mapping: Ensure mmap finds file contents in inode

6. **Regression test:** Ensure M6 tests still pass

## Testing Phases

**Phase 2a (2 days):** Write + run initial tests, identify divergences

**Phase 2b (2 days):** Fix high-impact divergences (demand paging, CoW, mprotect)

**Phase 2c (1 day):** Fix minor divergences (hints, file mapping) + regression test

## Success Criteria

- [ ] All 6 tests PASS on both Linux and Kevlar
- [ ] No M6 regressions (14/14 threading, 15/15 regression tests)
- [ ] Divergences documented if any remain
- [ ] CoW and demand paging are correctly implemented
- [ ] TLB shootdown synchronizes properly on -smp 4

## Known Issues & Workarounds

1. **Demand paging baseline:** Linux might not page in strictly on first access (kernel read-ahead). Test might not be deterministic. Solution: Run multiple times, check trends.

2. **CoW page size:** On x86, page size is 4KB. On ARM with hugepages, might differ. Pin test to 4KB pages.

3. **TLB shootdown timing:** Thread might not spin fast enough to catch the race. Add longer delays if needed.

4. **File mapping requires working filesystem:** Ensure M5 (ext2 + block device) still works.

## Contract Documentation

As each contract is validated, add to `docs/contracts.md`:

```markdown
## VM Contracts

### Demand Paging
- mmap(2) does not allocate physical pages
- Pages are allocated on first access (page fault)
- RSS grows incrementally as pages are touched
- Tests: testing/contracts/vm/demand_paging.c

### Copy-on-Write (fork)
- fork(2) creates a new process with shared, read-only pages
- Writing to a page triggers a page fault, copies page, updates page table
- Parent and child see independent address spaces
- Tests: testing/contracts/vm/fork_cow.c

### mprotect Atomicity
- mprotect(2) changes permissions atomically
- Subsequent accesses see new permissions immediately
- Tests: testing/contracts/vm/mprotect.c

### TLB Shootdown Ordering
- TLB invalidation propagates to all CPUs before mprotect(2) returns
- Threads on other CPUs see new page permissions after mprotect(2)
- Tests: testing/contracts/vm/tlb_shootdown.c (on -smp 4)
```

This becomes the spec for M10 GPU drivers: if they rely on these contracts, they'll work on Kevlar.
