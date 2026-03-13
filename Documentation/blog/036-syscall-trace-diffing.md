# M6.5 Phase 1.5: Syscall Trace Diffing and Contract Fixes

Phase 1 of M6.5 delivered the contract test harness — a framework that
compiles C contract tests, runs them on both Linux and Kevlar, and compares
output.  Phase 1.5 adds the tooling that makes those failures *actionable*:
runtime syscall tracing, a trace diff tool, and several kernel fixes
discovered by using the tooling on real failures.

---

## The debugging problem

When a contract test prints `CONTRACT_FAIL sbrk_grow` on Kevlar but
`CONTRACT_PASS` on Linux, you know the *test* fails but not *why*.  The
investigation cycle was:

1. Read the C test to identify which syscall it tests
2. Read the kernel's syscall implementation
3. Add printk-style tracing, recompile, re-run
4. Repeat until the root cause is found

This scales poorly.  A single failing test could take an hour to
diagnose.  We needed two things:

- **Runtime tracing** without recompilation
- **Automated diffing** of Linux vs Kevlar syscall sequences

## Runtime debug= cmdline

Kevlar already had a complete syscall trace infrastructure: `SyscallEntry`
and `SyscallExit` debug events serialized as JSONL `DBG {...}` lines.
But enabling it required a compile-time env var (`KEVLAR_DEBUG=syscall`)
and a full kernel rebuild.

The fix was simple: parse `debug=syscall` from the kernel command line.
The `BootInfo` struct gained a `debug_filter: ArrayString<64>` field,
parsed in both x64 and arm64 bootinfo code.  In `boot_kernel()`:

```rust
let debug_str = if !bootinfo.debug_filter.is_empty() {
    Some(bootinfo.debug_filter.as_str())
} else {
    option_env!("KEVLAR_DEBUG")
};
debug::init(debug_str);
```

Now `make run CMDLINE="debug=syscall"` produces full JSONL traces with
zero recompilation.  The compile-time `KEVLAR_DEBUG` remains as a fallback
for builds that need tracing always-on.

## diff-syscall-traces.py

`tools/diff-syscall-traces.py` runs a contract test on both sides and
aligns the syscall sequences:

1. **Linux**: runs the test binary under `strace -f`, parses the output
2. **Kevlar**: boots QEMU with `debug=syscall`, parses JSONL from serial
3. **Alignment**: greedy forward scan with 4-position lookahead, skipping
   "boring" startup syscalls (mmap, arch_prctl, etc.)
4. **Diff**: reports the first divergence with context lines

```
$ python3 tools/diff-syscall-traces.py brk_basic --filter brk
  Aligned 6 syscall pairs.  Divergences: 5
  ROOT CAUSE CANDIDATE: brk()
    Linux  → 0x3c0af000
    Kevlar → (none)
```

The `--trace` flag was also added to `compare-contracts.py` so that
`make test-contracts-trace` automatically runs trace diffs on failures.

## Bug fix 1: brk() never returns an error

The contract test used `sbrk(8192)` which calls `brk(current + 8192)`.
Our `sys_brk` propagated errors from `expand_heap_to()` with `?`,
returning `-ENOMEM`.  But Linux's brk() *never* returns a negative
error — on failure it returns the unchanged break.  musl's sbrk detects
failure by comparing the return value to the requested address.

```rust
// Before (wrong):
vm.expand_heap_to(new_heap_end)?;

// After (Linux semantics):
let _ = vm.expand_heap_to(new_heap_end);
```

A second discovery: musl 1.2.x *deprecated* `sbrk()` for non-zero
arguments.  The compiled binary's sbrk(N) is a stub that always returns
`-ENOMEM` without even making a syscall.  The contract test was rewritten
to use `syscall(SYS_brk, addr)` directly.

## Bug fix 2: mprotect(PROT_NONE) kills instead of delivering SIGSEGV

The mprotect_basic test installs a SIGSEGV handler, calls
`mprotect(p, 4096, PROT_NONE)`, then reads from p.  On Linux this
delivers SIGSEGV to the handler; the handler longjmps to safety.

On Kevlar, the page fault handler detected the PROT_NONE VMA and called
`Process::exit_by_signal(SIGSEGV)` — killing the process immediately.
The signal handler never ran.

The fix: send the signal and return from the page fault handler.  The
interrupt return path (`x64_check_signal_on_irq_return`) already checks
for pending signals and redirects RIP to the user's signal handler
trampoline via `try_delivering_signal()`.

```rust
// Before:
Process::exit_by_signal(SIGSEGV);

// After:
current.send_signal(SIGSEGV);
return;
```

## Bug fix 3: getpriority/setpriority ENOSYS

The scheduling/getpriority contract test failed with ENOSYS.  Added
`sys_getpriority` and `sys_setpriority` implementations.  The Linux
kernel convention for getpriority is to return `20 - nice` (avoiding
negative return values in kernel space); the libc wrapper inverts it.

## Results

After Phase 1.5:

| Test | Before | After |
|------|--------|-------|
| vm.brk_basic | FAIL | PASS |
| vm.mprotect_basic | DIVG (no output) | PASS |
| scheduling.getpriority | FAIL (ENOSYS) | PASS |
| signals.sa_restart | TIMEOUT | TIMEOUT (needs setitimer) |
| All others | PASS | PASS |

**7/8 contract tests pass.**  The remaining `sa_restart` requires
`setitimer`/`SIGALRM` (Phase 4 scope).

## New Makefile targets

- `make trace-contract TEST=brk_basic` — trace a single test
- `make test-contracts-trace` — run all tests with auto-trace on failure
