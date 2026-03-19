# M9.9: vDSO Syscall Acceleration — Beat Linux on Identity Syscalls

**Goal:** Extend Kevlar's vDSO to serve getpid, gettid, getuid, getpriority,
and uname entirely in userspace, achieving 5-8x speedup over Linux on these
syscalls. These are optimizations Linux cannot replicate due to its shared
vDSO page model and PID namespace complexity.

## Why this matters

Identity syscalls (getpid, gettid, getuid) are called millions of times in
production workloads — logging frameworks stamp every line, musl calls getpid
on every `fork()` return check, and `uname` is called during every process
startup by glibc/musl for feature detection.

Linux serves these as real syscalls (~86-162ns each). Kevlar's vDSO
infrastructure (already proven with clock_gettime at 10ns vs Linux's 19ns)
can serve them in ~10ns by reading from a per-process data page mapped
into userspace.

## Why Linux can't do this

Linux's vDSO page is a single shared mapping across all processes. This makes
per-process data (PID, TID, UID) impossible to serve from the vDSO without
a separate per-process "vvar" data page. Linux does have vvar pages for
clock data, but extending them to identity syscalls is blocked by:

1. **PID namespaces**: `getpid()` must return the namespace-local PID, but the
   vDSO page is shared across namespace boundaries. Linux would need per-namespace
   vvar mappings — a major kernel infrastructure change nobody has shipped.

2. **setuid/setgid semantics**: UID can change mid-process via `setuid()`. Linux
   would need to update the vvar page atomically from the kernel side while
   userspace reads it — tricky without a seqlock in the vDSO.

3. **Thread semantics**: `gettid()` returns a per-thread value, requiring
   per-thread vvar mappings or TLS-based storage. Linux vDSO has no per-thread
   data model.

Kevlar maps a fresh vDSO page per-process and can write per-process data
directly into it. Combined with our ownership-proof model, this is safe and
simple.

## Current vDSO state

- Single 4KB page at `VDSO_VADDR = 0x1000_0000_0000`
- One function: `__vdso_clock_gettime` (CLOCK_MONOTONIC via rdtsc)
- Data area at offset 0xF00: `tsc_origin` (8 bytes) + `ns_mult` (8 bytes)
- musl discovers the function via DT_HASH + DT_SYMTAB + DT_STRTAB
- Result: clock_gettime 10ns (Kevlar) vs 19ns (Linux) = 0.53x

## Current benchmark standings

| Syscall | Linux KVM | Kevlar KVM | Ratio | After vDSO (est.) |
|---------|-----------|------------|-------|--------------------|
| getpid | 86ns | 77ns | 0.90x | ~10ns (0.12x) |
| gettid | 90ns | 80ns | 0.89x | ~10ns (0.11x) |
| getuid | 84ns | 76ns | 0.90x | ~10ns (0.12x) |
| getpriority | 86ns | 80ns | 0.93x | ~10ns (0.12x) |
| uname | 162ns | 145ns | 0.90x | ~40ns (0.25x) |

## Phase plan

| Phase | Scope | Target |
|-------|-------|--------|
| 1 | [vDSO data page + getpid/gettid](phase1-vdso-getpid-gettid.md) | getpid/gettid ~10ns |
| 2 | [vDSO getuid/getpriority](phase2-vdso-getuid-getpriority.md) | getuid/getpriority ~10ns |
| 3 | [vDSO uname](phase3-vdso-uname.md) | uname ~40ns |
| 4 | [Validation + benchmark](phase4-validation.md) | All 5 syscalls verified |

## Success criteria

- getpid, gettid, getuid, getpriority: < 15ns (>5x faster than Linux)
- uname: < 50ns (>3x faster than Linux)
- All contract tests still pass (105 PASS, 0 FAIL)
- No regressions on any other benchmark
- musl compatibility verified (musl's `__vdsosym()` finds all new symbols)

## Files to modify

| File | Changes |
|------|---------|
| `platform/x64/vdso.rs` | Expand data area, add 5 new vDSO functions, update ELF metadata |
| `kernel/syscalls/mod.rs` | Short-circuit getpid/gettid/getuid to update vDSO data on change |
| `kernel/process/process.rs` | Write PID/TID/UID to vDSO data page on fork/clone/setuid |
| `kernel/mm/vm.rs` | Ensure vDSO page is per-process (not shared across fork) |
| `benchmarks/bench.c` | Verify benchmarks call raw syscall (not libc cache) |

## Architecture decision: per-process vs per-thread vDSO data

**Decision: per-process vDSO data page + TID via TLS.**

- PID, UID, nice value, uname data: written to the vDSO data page at offset 0xF10+
- TID: each thread has a unique TID but shares the vDSO page. Two options:
  a. Separate vDSO page per thread (memory cost: 4KB/thread)
  b. Store TID in TLS (FS segment base), read via `mov rax, fs:[offset]`

  Option (b) is better — zero memory cost, ~3ns read. musl already sets up
  TLS with `set_tid_address`. The vDSO `__vdso_gettid` function reads from
  a known TLS offset. The kernel writes the TID to that offset during
  `clone()` / `set_tid_address()`.

  **Fallback**: If TLS is not set up (static binaries without pthread), the
  vDSO function falls back to `syscall(SYS_gettid)`.
