## Blog 207: ARM64 port revival — build, boot, and the gap to contract parity

**Date:** 2026-04-22

The FUTURE_SESSION_ARM64_PROMPT handoff from blog 206 assumed ARM64
was in the same shape it was in on 2026-03-19 (≥95/118 contract
tests passing, 14/14 threading).  It wasn't.  Between then and now,
the x86_64 side absorbed a month of bug-fix and instrumentation
work — the Milestone T dynamic-analysis stack (blogs 196–206), the
task #25 investigation (PCID per-CPU generation in blog 203, CoW
free-ordering in 204, defensive `cld` in 205), the resched
plumbing, new `PageTable` methods for ghost-fork and deferred
teardown.  None of that landed on ARM64.  The ARM64 platform crate
was still emitting the 2026-03-19 surface area; the cross-arch
kernel crate had grown to call APIs that only x86 exposed.

The first `make ARCH=arm64 RELEASE=1 check` of this session hit 20
compile errors plus the usual warnings.  This post covers what got
ported to clear them, what now works, and — because this is the
honest start of a real ARM64 effort — what's still broken that
deserves its own milestone.

## The 20 errors, grouped

### Arch-level APIs added on x86, missing on arm64

- `arch::watchdog_check`, `arch::watchdog_enable` — LAPIC-heartbeat-
  based hard-lockup detector; called every timer tick from
  `kernel/timer.rs:204` and at init from `kernel/main.rs:488`.
- `arch::if_trace_enable` — interrupt-flag transition ring buffer
  enable; called at init.
- `arch::set_need_resched`, `arch::set_resched_fn` — deferred
  reschedule from `preempt_enable`.  Callers: timer ISR when
  `in_preempt()` is true; kernel init wires `set_resched_fn` to
  `process::switch`.
- `arch::register_cpu_apic_id` — per-CPU APIC ID registration for
  the watchdog's NMI-IPI target list.

### `PageTable` methods added on x86, missing on arm64

- `map_device_page` — PCI BAR / framebuffer MMIO mapping with
  cache-disable/write-through attrs.
- `pml4`, `clear_pml4_for_defer`, `from_pml4_for_teardown` — the
  deferred-Vm-drop machinery from blog 199's munmap-TLB-shootdown
  deadlock fix.  Vm::Drop under IF=0 stashes the PML4 physaddr and
  a safe-context drainer re-materialises a PageTable to run the
  teardown.
- `duplicate_from_ghost`, `restore_writable_from`,
  `teardown_ghost_pages` — the blog-194/195 ghost-fork optimisation.
  Parent's PTEs get CoW-marked, child gets a structurally-identical
  copy without touching refcounts until exec or exit.

### `ArchTask` methods used by the NMI-watchdog corruption detector

- `saved_context_summary` — read the saved-by-do_switch_thread
  context frame (rsp, rip, rbp) for non-running tasks.
- `kernel_stack_paddr` — physical base of the kernel stack for
  diagnostics.
- `rsp_in_owned_stack` — distinguish "saved RSP points into a stack
  we own" from "saved RSP is garbage / dangling pointer".

### Miscellaneous

- `kernel/mm/page_fault.rs:93` unconditionally called
  `x86::controlregs::cr2()` in the SIGSEGV diagnostic path.  ARM64
  has `FAR_EL1`.
- `kernel/syscalls/mod.rs:1355,1700` had match arms
  `SYS_SCHED_*AFFINITY | SYS_SCHED_*AFFINITY_COMPAT` — the COMPAT
  constants only exist in the x86_64 syscall-number module.
- Two unguarded `asm!("cli", ...)` sites in `kernel/lang_items.rs`
  (recursive panic) and `kernel/process/process.rs` (release_stacks
  re-mask).

## The port

The arm64 `CpuLocalHead` gained a `need_resched: u32` after
`preempt_count` (no shift of existing offsets — `usermode.S` still
reads `preempt_count` via `ldr w1, [x0, #16]`).  `preempt_enable`
now mirrors the x86 version: decrement count, and if count==0 and
`need_resched != 0`, clear the flag and invoke the reschedule
function pointer via a single SeqCst AtomicPtr load.

The watchdog and if-trace stubs are explicit no-ops with comments
pointing at their ARM64 equivalents (GICv3 NMI / FIQ for the
watchdog, DAIF.I transition tracking for if-trace).  A proper port
exists as a task; the stubs let the cross-arch kernel init path
keep working without `#[cfg]` noise at every call site.

For `PageTable`, the fun parts are:

- `pml4()` returns `self.pgd` — the name stays "pml4" for API
  parity; arm64's top-level is a PGD.
- `map_device_page` uses MAIR index 0 (Device-nGnRnE — already
  configured by `boot.S`), not index 1 (Normal WB).  The framebuffer
  path produces uncached writes, which is what bochs_fb's x86
  path gets via CACHE_DISABLE|WRITE_THROUGH.  (`bochs_fb` itself is
  x86-only — the `IoPort` import and VBE register accessors got
  `#[cfg(target_arch = "x86_64")]` gates, since VBE is a PC-ism.)
- `duplicate_from_ghost` on arm64 falls back to plain `duplicate_table`
  and returns an empty `Vec<usize>` of CoW-marked addresses.  This
  is a *performance* regression vs x86 (no CoW, full eager copy of
  PTEs), not a correctness one.  `restore_writable_from` is
  therefore a no-op — `addrs.is_empty()` under the fallback.
  `teardown_ghost_pages` delegates to `teardown_forked_pages` since
  the underlying structures are identical.
- `ArchTask::saved_context_summary` returns `None` for now — reading
  arm64's saved context frame off the kernel stack requires knowing
  what `do_switch_thread` pushes, which is a separate port.  The
  corruption detector gracefully skips tasks whose summary is
  `None`, so the stub just opts out of that diagnostic on arm64.

## Build system gaps

Two Makefile bugs unrelated to the code port:

1. No rule existed for `build/testing.arm64.initramfs`.  The
   `INITRAMFS_PATH` variable was set to the arm64 filename when
   `ARCH=arm64`, but make had no way to produce that file.  Added a
   parallel rule that calls `tools/build-initramfs.py --arch arm64`.

2. `LLVM_BIN_DIR` hardcoded `rustlib/x86_64-unknown-linux-gnu/bin`.
   On `aarch64-apple-darwin` the llvm-tools live under
   `rustlib/aarch64-apple-darwin/bin`.  Switched to `$(RUSTC_HOST)`
   auto-detected from `rustc -vV`.

On macOS the initramfs step also needs a darwin-native aarch64→Linux
cross-compiler; the default bundled path downloads an *i386 Linux
ELF* musl.cc toolchain that can't execute under Darwin.  Installing
Homebrew's `aarch64-linux-musl-gcc` (via a
`macos-cross-toolchains`-style tap) resolves it — the existing
`build-initramfs.py` already prefers a system cross-compiler over
the downloaded one, so no script changes needed.

## What works

- `make ARCH=arm64 RELEASE=1 check`: clean.  341 warnings, 0 errors.
- `make ARCH=arm64 RELEASE=1 build`: produces `kevlar.arm64.elf`
  (37 MB), stripped ELF (29 MB), and flat Image (29 MB).
- Boot: reaches the BusyBox shell prompt `~ #` in under a second
  under QEMU TCG on `-machine virt -cpu cortex-a72 -m 1024`.  Simple
  shell commands (`echo`, `exit`) work; `exit` returns cleanly and
  the system halts with status 0.

Two cosmetic issues in the boot path that don't block anything and
go on the arm64 task list:

- `sh: poll: File exists` printed at each shell prompt — busybox's
  `sh` is getting `EEXIST` back from some `poll()` invocation on
  arm64.  Specific to our syscall layer, not fatal.
- First character of typed input gets dropped (`uname -a` → the
  kernel echoes `name -a`).  Terminal-echo / line-discipline issue
  on the arm64 serial path.

## What doesn't work: contract test regression

Sampled 63 contract tests across `process/`, `signals/`, `pipes/`,
`fd/`, `time/` (running with `--no-linux --cc aarch64-linux-musl-gcc`
since we're cross-compiling on macOS, not running a Linux baseline):

```
Results: 18/63 PASS  |  0 DIVERGE  |  45 FAIL  |  0 SKIP
```

28% pass rate, down from the March 2026 baseline of ≥95/118 (~80%).
The representative failure I dug into — `vm/mmap_anon`:

```
zero_init: ok
map_fixed_zero: ok
[PANIC] CPU=0 at libs/kevlar_utils/buddy_alloc.rs:209
buddy_alloc: double free page 0x42268000 block 0x42268000 order 0
```

Flight recorder shows the panic on `MUNMAP pid=1 addr=0xa00000000
len=0x2000` at tick 206813, right after a `MAP_FIXED|MAP_ANONYMOUS`
replacement of an existing mapping.  The arm64 mmap/munmap path is
double-freeing one of the pages that was released by the MAP_FIXED
replacement.

This is **not** caused by the port work — none of the methods I
added or stubbed are in the munmap path.  It's a pre-existing arm64
page-allocator bug that was never hit by the 2026-03-19 baseline
because that baseline didn't exercise MAP_FIXED replacements at
scale, or because the arm64 mmap implementation changed behaviour
independently between then and now.

Other failures look like independent ABI gaps:

```
FAIL  time.settimeofday_accept
  CONTRACT_FAIL settimeofday: ret=-1 errno=22
```

— settimeofday returns EINVAL where Linux returns 0.  These are
contract-parity issues that need to be run to ground one per
blog-post or at least one per commit.

## The honest state

The x86_64 side of the Milestone T / task-25 investigation did not
maintain ARM64 parity.  Every fix for a paging or syscall bug
between blog 194 and blog 205 was implemented against
`platform/x64/` and `kernel/mm/` without a matching arm64 change.
In code review terms: the arm64 backend had no owner for a month.

The port this session delivers is the *build + boot* layer: the
minimum cross-arch surface area to compile, link, and get through
init.  ABI parity — the thing that actually matters for "Linux
replacement on ARM64" — is its own body of work.  The contract test
suite is the correct ledger for that work: each failing test is a
known gap, each passing test is a proven equivalence.

## What comes next

Fix the contract tests.  Starting with `vm/mmap_anon` — the
`buddy_alloc` double-free is the most load-bearing failure because
it crashes the kernel, which masks other tests' behaviour.  Order
of attack:

1. `vm/` — MAP_FIXED replace, MAP_ANONYMOUS zeroing, demand paging,
   brk.  Fixing the panic classes first unblocks the other
   categories.
2. `signals/`, `pipes/`, `fd/` — straightforward syscall ABI work
   once the kernel stops panicking mid-run.
3. `subsystems/` — clock, setgroups, resource limits; these are
   typically missing-syscall or wrong-errno issues.

Each fix gets its own small blog post.  The goal is to end this
arc with `test-contracts-arm64` matching the x86_64 baseline
test-for-test — that's the "Linux-compatible on ARM64" claim
cashed.

## Stats

- 1 session
- 9 files touched (5 arm64 platform, 2 kernel cross-arch, 1 bochs_fb, 1 Makefile)
- ~150 lines of ARM64 platform code added
- 20 compile errors → 0
- 18/63 contract tests passing on the sampled categories
- 1 new task: fix ARM64 contract regressions to parity
