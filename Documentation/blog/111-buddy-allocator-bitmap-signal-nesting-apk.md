# Blog 111: Buddy allocator bitmap guard, signal nesting, and apk installs packages

**Date:** 2026-03-23
**Milestone:** M10 Alpine Linux

## Summary

Three critical kernel bugs fixed, Alpine's `apk` package manager now installs
packages live over HTTP, and the BusyBox test suite passes 100/100 via the
`sh -c` vfork path (previously crashed).

## Bug 1: Buddy Allocator Returning Already-Allocated Pages

**Symptom:** BusyBox test suite crashed with SIGSEGV (RBP=0, vaddr=0x2b8)
after ~70 fork+exec cycles when run via `sh -c`. The kernel stack of sleeping
processes was silently zeroed, corrupting saved register state.

**Root cause:** The buddy allocator's `free_coalesce` merged freed blocks with
"buddy" blocks that were NOT genuinely free. Pages removed from the buddy's
intrusive free lists (e.g., sitting in the page-allocator's `PAGE_CACHE`) were
invisible to the free-list walk, so `remove_from_free_list` returned false ---
but the coalescing logic had no second opinion. Meanwhile,
`refill_prezeroed_pages` (called from the idle thread) allocated single pages
from buddy and zeroed them. If those pages were part of an active kernel
stack, the sleeping process's stack frame was destroyed.

**Fix:** Added a global allocation bitmap (32 KB static, 1 bit per 4 KB page).
`alloc_order` marks pages as allocated; `free_coalesce` marks them free. Before
coalescing with a buddy, `free_coalesce` now checks that ALL the buddy's bitmap
bits are clear --- preventing merges with pages in PAGE_CACHE or any other
non-buddy tracking structure.

**Files:** `libs/kevlar_utils/buddy_alloc.rs`

## Bug 2: Signal Handler Re-Entrancy Corrupting Registers

**Symptom:** `apk update` crashed with SIGSEGV at address 0x2b8 (null struct
pointer + field offset). RBP=0 after returning from a signal handler. Multiple
SIGCHLD signals during HTTP fetches caused nested handler invocations.

**Root cause:** Kevlar stored the interrupted register context in a single
kernel-side slot (`signaled_frame`). When a second signal arrived during the
first handler (e.g., SIGALRM interrupting SIGCHLD handler), it overwrote the
slot. On `rt_sigreturn`, the outer handler restored the wrong context.

**Fix:** Two changes:

1. **User-stack signal context:** `setup_signal_stack` now writes the complete
   interrupted register state (19 fields: all GPRs + RIP + RSP + RFLAGS +
   signal mask = 152 bytes) to the user stack in the reserved 832-byte signal
   frame area. `rt_sigreturn` reads them back. Each nested signal gets its own
   independent save on the user stack.

2. **Signaled frame stack:** Changed `signaled_frame` from a single
   `AtomicCell<Option<PtRegs>>` to a `SpinLock<ArrayVec<PtRegs, 4>>` --- a
   small stack supporting up to 4 levels of nesting.

3. **sa_mask parsing:** `rt_sigaction` now reads and stores the `sa_mask` field
   from userspace sigaction structs.

**Files:** `platform/x64/task.rs`, `kernel/process/process.rs`,
`kernel/process/signal.rs`, `kernel/syscalls/rt_sigaction.rs`

## Bug 3: brk Heap VMA Overlapping Shared Library Text

**Symptom:** `apk update` crashed with SIGSEGV at address 0x2b8. The process
had 3924 VMAs (!) and two VMAs overlapped: a read-write heap VMA and a
read-execute musl text VMA.

**Root cause:** In `Vm::expand_heap_to`, when the heap grew via `brk()` and the
range wasn't free, the code called `extend_by(grow)` on an existing anonymous
VMA without checking if the extension would overlap OTHER VMAs. The heap VMA
grew into musl's `.text` segment, causing code execution to read heap data
instead of instructions.

**Fix:** Before extending a VMA, verify the extension range `[area_end,
area_end + grow)` doesn't overlap any other VMA (excluding the one being
extended).

**Files:** `kernel/mm/vm.rs`

## Other Fixes

- **Device node rdev:** `/dev/null`, `/dev/zero`, `/dev/urandom` now report
  correct major:minor numbers in `stat()` (was 0:0, now 1:3, 1:5, 1:9).
  Required by OpenSSL to validate `/dev/urandom`.

- **Alpine image build:** Added `/etc/ld-musl-x86_64.path` with `/lib` and
  `/usr/lib` search paths. Symlinked all `/usr/lib/*.so*` into `/lib/` so
  musl's dynamic linker finds them. Copies `apk.static` from initramfs into
  the Alpine rootfs at boot for reliable package management.

- **Test harness:** New `test-alpine-apk` target and C test binary that boots
  Alpine with OpenRC, runs `apk update` + `apk add curl`, verifies curl runs.
  Uses a disk image copy so tests don't corrupt the interactive image.

## Status

| Feature | Status |
|---------|--------|
| OpenRC boot | **Zero crashes** |
| BusyBox 100/100 | **Via sh -c (vfork)** |
| apk update | **25,397 packages** |
| apk add curl | **8 deps installed** |
| curl runs | **Version prints** |
| Signal nesting | **User-stack save** |
| Buddy allocator | **Bitmap-guarded** |
| Alpine shell | **Interactive `/ #`** |
