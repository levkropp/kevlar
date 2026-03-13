# Phase 4: Integration Testing

**Duration:** ~2-3 days
**Prerequisite:** Phase 2 + Phase 3
**Goal:** Verify /proc + glibc work together in real-world scenarios.

## Scope

- Build test binaries with both musl and glibc
- Verify core system tools work (ps, top, strace, etc.)
- Test complex glibc features (malloc, Python, etc.)
- Ensure no regressions in musl tests

## Test Binaries

### 1. Hello World (glibc)

```c
#include <stdio.h>
int main() {
    printf("Hello from glibc!\n");
    return 0;
}
```

Compile: `gcc -static hello.c -o hello-glibc`
Test: `hello-glibc` should print and exit.

### 2. pthreads (glibc)

Compile our existing `testing/mini_threads.c` with glibc:
```bash
musl-gcc -static -O2 -pthread -o mini-threads.musl testing/mini_threads.c
gcc -static -O2 -pthread -o mini-threads.glibc testing/mini_threads.c
```

Run both under `-smp 4`. Acceptance: 14/14 pass for both.

### 3. ps / top / htop

Build BusyBox with procfs support, or use Alpine Linux's ps/top.
Test:
- `ps aux` lists processes correctly
- `top -n 1` shows CPU/memory stats
- `ps -elf` shows full output (glibc-friendly columns)
- `strace -e trace=file /bin/ls /` traces file operations

### 4. Python 3 (glibc)

If time permits, compile Python 3 as static glibc binary.
Test:
```python
import sys
print(sys.version)
import ctypes
# This will fail if /proc/self/maps is missing
lib = ctypes.CDLL("libc.so.6")
```

This tests /proc/self/maps interaction with glibc malloc.

### 5. strrace (glibc)

glibc-linked strace binary. Test:
```bash
strace -e open,openat /bin/ls /tmp 2>&1 | head -20
```

Requires /proc/[pid]/syscall and other strace-specific /proc entries (Phase 2).

## Regression Testing

Run the M6 test suite to ensure no regressions:
- `make test-threads-smp` (14/14 musl tests must still pass)
- `make test-regression-smp` (M4 mini_systemd must still pass)

If any fail, the M7 implementation broke backward compatibility.

## Build System Changes

### Update Makefile

Add targets:
```makefile
.PHONY: build-glibc-mini-threads
build-glibc-mini-threads:
	gcc -static -O2 -pthread -o testing/mini-threads.glibc testing/mini_threads.c

.PHONY: test-glibc-threads
test-glibc-threads: build-glibc-mini-threads
	# Copy to initramfs and run
	$(MAKE) build INIT_SCRIPT="/bin/mini-threads.glibc"
	timeout 120 $(PYTHON3) tools/run-qemu.py ... -- -smp 4

.PHONY: test-m7
test-m7: test-threads-smp test-glibc-threads test-regression-smp
	@echo "M7 tests complete"
```

### Docker

Build glibc tools in the testing Docker image (if needed).

## Success Criteria

- [ ] glibc hello world runs
- [ ] 14/14 glibc pthreads tests pass
- [ ] 14/14 musl pthreads tests still pass (no regressions)
- [ ] 15/15 M4 regression tests still pass
- [ ] `ps aux` lists processes
- [ ] `cat /proc/self/maps` shows correct memory layout
- [ ] `/proc/cpuinfo` reports correct CPU count
- [ ] `strace` traces system calls correctly

## Known Issues & Workarounds

1. **glibc malloc might be slow:** glibc's malloc reads /proc/self/maps on every
   allocation in some configurations. If tests timeout, implement /proc caching
   or optimize malloc interaction.

2. **Stack traces in error messages:** glibc prints stack traces on abort. If
   tests fail early, the backtrace might be incomplete (depends on Phase 2 fd/
   symlinks being correct).

3. **Timeout increases:** tests might be slower with /proc overhead. Increase
   test timeouts from 120s to 180s if needed.

## Future Work

After M7, the next priorities are:
- **M8: /sys + cgroups + namespaces** — needed by systemd
- **M9: Full systemd** — boot systemd as init

This phase focuses on correctness, not systemd readiness yet.
