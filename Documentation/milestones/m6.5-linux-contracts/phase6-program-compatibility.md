# Phase 6: Program Compatibility

**Duration:** ~4-5 days
**Prerequisite:** Phases 1-5
**Goal:** Run progressively more complex real Linux programs on Kevlar, using each failure as a specification for a missing contract.

## Scope

Phases 1-5 validate contracts in isolation. Phase 6 validates them compositionally—running real programs that exercise multiple contracts simultaneously. Each program tier is more demanding than the last.

This phase works backwards from the goal (GPU drivers, desktop apps) through increasingly complex programs, identifying gaps and fixing them. Each gap either points to a contract validated in Phases 1-5 (and that fix propagates) or surfaces a new contract not yet captured.

## Program Tiers

### Tier 1: Static musl binaries (already works from M6)

Goal: Confirm no regressions.

Programs:
- BusyBox shell (`sh -c "echo hello"`)
- BusyBox tools (`ls`, `cat`, `find`, `grep`, `awk`)
- mini_threads test suite (14/14)
- mini_systemd test suite (15/15)

**Test:** `make test-regression` — must pass before entering Phase 6.

### Tier 2: Dynamic musl binaries

Goal: Confirm dynamic linker + musl libc work for typical programs.

Programs:
- musl-compiled `hello.c` (dynamic)
- musl-compiled `pthreads.c` (dynamic)
- musl-compiled `curl` (HTTP request)
- musl-compiled `python3` (interpreter startup)

**Harness test:** `testing/contracts/programs/tier2-musl-dynamic.sh`
```bash
#!/bin/sh
# Compile hello.c as dynamic musl binary
cat > /tmp/hello.c <<'EOF'
#include <stdio.h>
int main() { printf("hello from musl dynamic\n"); return 0; }
EOF
musl-gcc -o /tmp/hello /tmp/hello.c
echo hello_output: $(/tmp/hello)
```

### Tier 3: glibc static binaries

Goal: Confirm glibc compatibility. This surfaces the futex, rseq, and clone3 gaps that were investigated before M6.5.

Programs:
- glibc-compiled `hello.c` (static, -static -lc)
- glibc-compiled `pthreads.c` (static)
- glibc `date`, `ls`, `id`

**Key contracts exercised:**
- FUTEX_CMP_REQUEUE (op 4)
- FUTEX_WAKE_OP (op 5)
- rseq syscall (334) — must not crash, ENOSYS OK
- AT_HWCAP, AT_CLKTCK auxv entries

**Test:** `testing/contracts/programs/tier3-glibc-static.c`
```c
// Compile with gcc (glibc) -static
#include <stdio.h>
#include <pthread.h>

static int counter = 0;
pthread_mutex_t mu = PTHREAD_MUTEX_INITIALIZER;

void *thread_func(void *arg) {
    pthread_mutex_lock(&mu);
    counter++;
    pthread_mutex_unlock(&mu);
    return NULL;
}

int main() {
    pthread_t tid;
    pthread_create(&tid, NULL, thread_func, NULL);
    pthread_join(tid, NULL);

    if (counter != 1) {
        printf("ERROR: counter=%d\n", counter);
        return 1;
    }

    printf("glibc pthreads ok\n");
    return 0;
}
```

**Why:** glibc is the default C library on virtually every Linux distro. Kevlar needs glibc compatibility for M10.

### Tier 4: Common system utilities (dynamic glibc)

Goal: Run programs that are pre-compiled on the host distro (Ubuntu/Fedora) against Kevlar.

Programs:
- `/bin/ls` (from host)
- `/bin/ps` (requires /proc/[pid] parsing)
- `/usr/bin/strace` (requires ptrace)
- `/usr/bin/vim` (text editor, minimal curses)
- `/usr/bin/python3` (interpreter + stdlib)

**Key contracts exercised:**
- /proc/[pid]/stat, /proc/[pid]/maps (Phase 5)
- /proc/cpuinfo (Phase 5)
- Signal delivery and handling (Phase 4)
- Dynamic linker (ld.so) compatibility
- ELF symbol versioning
- Various ioctl stubs (TIOCGWINSZ, etc.)

**Harness test:** `testing/contracts/programs/tier4-system-utils.sh`
```bash
#!/bin/sh
# Test: /bin/ls should list files
ls /proc
echo "ps output:"
ps aux
```

### Tier 5: Compiled interpreters and language runtimes

Goal: Run full language runtimes that stress-test every subsystem.

Programs:
- Python 3 (with stdlib)
  - `python3 -c "import os; print(os.cpu_count())"` — tests /proc/cpuinfo
  - `python3 -c "import subprocess; subprocess.run(['ls'])"` — tests fork+exec
  - `python3 -c "import threading; ..."` — tests pthreads
- Node.js (libuv, epoll, signals)
- GCC (compiles C programs — tests file I/O heavily)

**Key contracts exercised:**
- /proc/cpuinfo (os.cpu_count)
- fork() + execve() pipeline (subprocess)
- epoll (Node.js event loop)
- File I/O at scale (GCC)

**Test:** `testing/contracts/programs/tier5-interpreters.sh`
```bash
#!/bin/sh
# Python test
python3 -c "
import os
import threading

# Test os.cpu_count()
cpus = os.cpu_count()
print('cpu_count:', cpus)

# Test threading
results = []
def worker():
    results.append(1)

threads = [threading.Thread(target=worker) for _ in range(4)]
for t in threads: t.start()
for t in threads: t.join()
print('threads:', len(results))
"
```

### Tier 6: Compiled programs with network access

Goal: Run programs that use networking (TCP/UDP sockets).

Programs:
- `curl https://example.com` (HTTPS — tests TLS, TCP, DNS)
- `wget` (HTTP — tests TCP)
- Python `requests` library
- Simple HTTP server (`python3 -m http.server`)

**Key contracts exercised:**
- TCP socket contracts (connect, accept, send, recv)
- DNS resolution (/etc/resolv.conf, getaddrinfo)
- TLS handshake (libssl)
- epoll for I/O multiplexing

### Tier 7: GPU driver prerequisites

Goal: Run programs that establish the prerequisites for GPU drivers.

Programs:
- `vulkaninfo` (requires /dev/dri/renderD128, DRM ioctls)
- `glxinfo` (requires X11 or Wayland, DRM, Mesa)
- `glmark2` benchmark (OpenGL workload)
- `nvidia-smi` stub (requires /dev/nvidia* devices)

**Key contracts exercised:**
- /dev/dri device nodes (Phase 5)
- DRM ioctl dispatch
- Mesa shared library loading
- PCI device enumeration (/sys/bus/pci/devices)
- Memory-mapped I/O (mmap with device fd)

**Status:** Tier 7 is expected to fail on M6.5. The goal is to capture what's missing, not fix it (that's M10).

## Failure Taxonomy

When a program fails, classify the failure:

| Category | Example | Action |
|----------|---------|--------|
| Missing syscall | `rseq` returns EFAULT | Stub with ENOSYS |
| Wrong syscall semantics | `futex` returns wrong errno | Fix per Phase 1-5 contracts |
| Wrong /proc format | `ps` can't parse /proc/[pid]/stat | Fix per Phase 5 contracts |
| Missing /dev node | /dev/dri/card0 not found | Stub in M8/M10 |
| Missing /sys attribute | /sys/class/net/eth0 not found | Document for M8 |
| Signal bug | program deadlocks under signals | Fix per Phase 4 contracts |
| VM bug | program crashes on mmap | Fix per Phase 2 contracts |
| Unknown | need more investigation | Add to "unknown failures" list |

All failures are logged in `build/contract-results.json` with category and fix status.

## Fix Pipeline

For each failure:
1. Identify the category (use taxonomy above)
2. Link to the relevant contract from Phases 2-5
3. Implement the fix
4. Re-run the failing program
5. Check for regressions (M6 test suite)
6. Update contract documentation

**Target for M6.5:** Tiers 1-4 must pass. Tier 5 partial (Python 3 works). Tiers 6-7 failures are documented but not fixed (M7-M10 scope).

## Implementation Plan

1. **Set up test environment:**
   - Prepare initramfs with glibc, musl, common utilities
   - Configure QEMU to mount host directory (via virtio-9p) for easy binary injection

2. **Run Tier 1-2** (1 day): Confirm no regressions, validate musl dynamic works

3. **Run Tier 3** (1 day): glibc static binaries. Fix futex ops, rseq stub.

4. **Run Tier 4** (1 day): System utilities (ls, ps, strace). Fix /proc format divergences.

5. **Run Tier 5** (1 day): Python 3, Node.js. Fix interpreter-specific issues.

6. **Document Tier 6-7 failures** (0.5 day): Capture what's missing for M7-M10.

## Makefile Targets

```makefile
.PHONY: test-tier1 test-tier2 test-tier3 test-tier4 test-tier5 test-compat

test-tier1:
	python3 tools/compare-contracts.py --suite tier1 --output build/tier1.json

test-tier2:
	python3 tools/compare-contracts.py --suite tier2 --output build/tier2.json

test-tier3:
	python3 tools/compare-contracts.py --suite tier3 --output build/tier3.json

test-tier4:
	python3 tools/compare-contracts.py --suite tier4 --output build/tier4.json

test-tier5:
	python3 tools/compare-contracts.py --suite tier5 --output build/tier5.json

test-compat: test-tier1 test-tier2 test-tier3 test-tier4 test-tier5
	python3 tools/summarize-compat.py build/tier*.json
```

## Success Criteria

- [ ] Tier 1 (BusyBox, musl): 100% PASS
- [ ] Tier 2 (dynamic musl): 100% PASS
- [ ] Tier 3 (glibc static): >90% PASS (futex ops, rseq, clone3 stubs)
- [ ] Tier 4 (system utilities): >80% PASS (ps, ls, vim)
- [ ] Tier 5 (interpreters): Python 3 basic PASS; Node.js partial
- [ ] Tier 6-7 failures documented in contract-results.json
- [ ] No M6 regressions

## Output Artifacts

By the end of Phase 6:

1. `build/contract-results.json` — full test results per tier
2. `docs/contracts.md` — consolidated contract documentation from Phases 2-5
3. `docs/known-divergences.md` — list of known Kevlar vs Linux divergences (for M7-M10 authors)
4. `testing/contracts/**` — all contract test binaries
5. `tools/compare-contracts.py` — test harness

These become the foundation for M7-M10 development:
- M7 authors check contracts before implementing /proc
- M8 authors check contracts before implementing cgroups
- M10 authors have a clear list of what needs to be fixed for GPU drivers

## Lessons from glibc Investigation (pre-M6.5)

During M6 we investigated glibc compatibility briefly before pivoting to M6.5. What we found:

| Issue | Status | Expected Fix Phase |
|-------|--------|--------------------|
| FUTEX_CMP_REQUEUE (op 4) | Not implemented | Phase 3 (Scheduling) |
| FUTEX_WAKE_OP (op 5) | Not implemented | Phase 3 (Scheduling) |
| rseq (syscall 334) | Not stubbed | Phase 6 Tier 3 |
| MAP_STACK / MAP_GROWSDOWN | Now fixed (M6 Phase 4) | Done |
| AT_HWCAP / AT_CLKTCK | Now fixed (M6 Phase 4) | Done |
| clone3 (syscall 435) | Returns ENOSYS, glibc falls back | Tier 3 validates |

The systematic approach of M6.5 will surface the remaining issues and ensure they're all fixed before moving to M7.
