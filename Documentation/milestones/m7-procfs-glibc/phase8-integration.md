# Phase 8: Integration Testing

**Duration:** ~1.5 days
**Prerequisite:** Phases 1-7
**Goal:** Verify /proc + glibc work together with real programs.

## Test matrix

### Tier 1: glibc hello world

Compile with glibc:
```bash
gcc -static -o hello-glibc hello.c
```

Run in Kevlar.  Must print and exit cleanly.  This exercises:
- glibc init (rseq stub, robust list, futex wait/wake)
- printf (buffered I/O)
- exit (atexit handlers, stdio flush)

### Tier 2: glibc pthreads

Compile existing `testing/mini_threads.c` with glibc:
```bash
gcc -static -O2 -pthread -o mini-threads-glibc testing/mini_threads.c
```

Run under `-smp 4`.  All 14 tests must pass:
1. thread_create_join
2. thread_arg_return
3. mutex_basic
4. mutex_recursive
5. condvar_signal
6. condvar_broadcast
7. rwlock_basic
8. barrier_basic
9. tls_basic
10. detach_basic
11. multiple_threads
12. thread_stack_size
13. pipe_pingpong
14. thread_storm

This is the critical gate: if glibc pthreads work on 4 CPUs, we have
real glibc compatibility.

### Tier 3: ps aux

Build BusyBox with glibc support (or use host `ps` binary).
`ps aux` must:
- List at least PID 1 (init)
- Show process state (S/R)
- Show command name

This exercises /proc/[pid]/stat, /proc/[pid]/status, and /proc/[pid]/cmdline.

### Tier 4: Regression

- `make test-threads-smp` — 14/14 musl tests (no regressions)
- `make test-regression-smp` — 15/15 M4 mini_systemd tests
- `make test-contracts` — 18/19 M6.5 contracts
- `make bench-kvm` — no >10% regressions from M6.6 baseline

## Build system additions

```makefile
.PHONY: build-glibc-tests
build-glibc-tests:
	gcc -static -O2 -pthread -o testing/mini-threads-glibc testing/mini_threads.c
	gcc -static -O2 -o testing/hello-glibc testing/hello.c

.PHONY: test-glibc-threads
test-glibc-threads: build-glibc-tests
	$(MAKE) build INIT_SCRIPT="/bin/mini-threads-glibc"
	timeout 180 $(PYTHON3) tools/run-qemu.py \
		--arch $(ARCH) $(kernel_qemu_arg) -- -smp 4 2>&1 \
		| tee /tmp/kevlar-test-glibc-threads.log
	@grep -E '^(TEST_PASS|TEST_FAIL|TEST_END)' \
		/tmp/kevlar-test-glibc-threads.log

.PHONY: test-m7
test-m7: test-glibc-threads test-threads-smp test-regression-smp test-contracts
	@echo "M7 integration suite complete."
```

## Known challenges

- **glibc version sensitivity:** glibc 2.35+ uses rseq; 2.38+ may use
  clone3 instead of clone.  Test with the glibc version in Ubuntu 20.04's
  Docker image.
- **Stack size:** glibc default thread stack is 8 MB (musl is 128 KB).
  May need to increase Kevlar's address space limits.
- **malloc /proc/self/maps:** glibc's malloc may read /proc/self/maps
  on every `mmap`.  Ensure this path is fast.
- **ld.so dynamic linker:** glibc static binaries include the dynamic
  linker statically, which reads /proc/self/exe and /proc/self/maps.

## Success criteria

- [ ] glibc hello world prints and exits cleanly
- [ ] 14/14 glibc pthreads tests pass on -smp 4
- [ ] 14/14 musl pthreads tests still pass
- [ ] 15/15 M4 regression tests still pass
- [ ] 18/19 M6.5 contract tests still pass
- [ ] ps aux shows at least PID 1
- [ ] No >10% benchmark regression from M6.6 baseline
