# M10 Phase 9: BusyBox Tests, Benchmarks, and Three Kernel Bugs

We set out to make `test-busybox` pass and `bench-busybox` produce
comparable numbers to Linux on KVM. Along the way we found three kernel
bugs, removed Docker from the Linux build, and made KVM the default for
all test targets.

## Bug 1: usercopy3 label misalignment

The most impactful bug. Every read from `/dev/zero` into a large buffer
crashed the kernel with a page fault panic.

The usercopy assembly in `platform/x64/usercopy.S` has labeled
instructions that the page fault handler recognizes as "safe" — if a
fault occurs at one of these labels, it's a user-space demand page fault
during a kernel usercopy, not a real kernel bug. The handler checks
`frame.rip == usercopy3` to decide.

`memset_user` fills a user buffer with a byte value. It's used by
`/dev/zero`'s `read()` to fill the user's buffer with zeros:

```asm
memset_user:
    mov rcx, rdx
    cld
usercopy3:          ; <-- label HERE
    mov al, sil     ; <-- but THIS instruction doesn't fault
    rep stosb       ; <-- THIS one does (writes to user memory)
    ret
```

The label pointed at `mov al, sil` (a register-to-register move that
never faults), but the actual user-space memory access is `rep stosb`
two bytes later. When `rep stosb` triggered a demand page fault, the
RIP was at `usercopy3 + 2`, the handler didn't match it, and the
kernel panicked.

The fix: move the label to the faulting instruction.

```asm
memset_user:
    mov rcx, rdx
    cld
    mov al, sil
usercopy3:          ; <-- label now at the faulting instruction
    rep stosb
    ret
```

This bug existed since the usercopy optimization pass (M6.6 Phase D)
but was invisible because `/dev/zero` reads only fault when the user
buffer straddles an unmapped page — which BusyBox `dd` does via
`malloc` (backed by `mmap` for large allocations) but the raw syscall
test doesn't (it uses stack buffers or pre-faulted heap).

## Bug 2: kernel heap OOM on tmpfs writes

After fixing the usercopy crash, `dd` still panicked when writing 1MB
to tmpfs:

```
[PANIC] CPU=0 at platform/global_allocator.rs:24
tried to allocate too large object in the kernel heap (requested 2097152 bytes)
```

Tmpfs stores file data in a `Vec<u8>` on the kernel heap. Vec's growth
strategy doubles capacity: writing 4KB chunks to build a 1MB file
produces a Vec that goes 4K → 8K → 16K → ... → 512K → **1024K**. At
1024K, Vec doubles to 2MB for the next resize — exceeding the 1MB heap
chunk limit.

Two fixes applied:
1. Increased `KERNEL_HEAP_CHUNK_SIZE` from 1MB to 4MB
2. Tmpfs `write()` now uses `reserve_exact` instead of letting Vec double:

```rust
let cap = data.capacity();
if new_len > cap {
    data.reserve_exact(new_len - cap);
}
data.resize(new_len, 0);
```

This keeps tmpfs allocations tight to the actual file size. A 1MB file
uses ~1MB of heap, not 2MB.

## Bug 3: Docker caching failures

Docker's build context hashing invalidated the entire multi-stage build
whenever any file in `testing/` changed. A one-line edit to
`busybox_suite.c` triggered a full rebuild of BusyBox, curl, dropbear,
bash, and systemd from source — minutes of wasted time.

Replaced the Docker pipeline with `tools/build-initramfs.py`, a native
Python builder that:
- Compiles test binaries directly with `musl-gcc`/`gcc` (parallel)
- Downloads and builds external packages once, cached in
  `build/native-cache/ext-bin/`
- Downloads Alpine packages directly from the CDN
- Assembles the rootfs and creates the CPIO archive

Incremental rebuild times: **1.5 seconds** when a `.c` file changes,
**65ms** when nothing changed. Docker fallback preserved via
`USE_DOCKER=1`.

## KVM by default

All test and benchmark targets now use `--kvm` unconditionally. Tests
that previously ran on TCG (software emulation, ~100x slower than KVM)
now run at hardware speed. No more `KVM=1` flag needed.

## Results

**BusyBox test suite:** 101/101 pass (unchanged)

**BusyBox benchmarks** (Kevlar KVM vs Linux KVM, lower = faster):

| Benchmark | Kevlar | Linux | Ratio |
|-----------|-------:|------:|------:|
| bb_exec_true | 340µs | 1.78ms | **0.19x** |
| bb_shell_noop | 610µs | 3.66ms | **0.17x** |
| bb_echo | 335µs | 1.88ms | **0.18x** |
| bb_cp_small | 526µs | 2.97ms | **0.18x** |
| bb_dd | 6.15ms | 4.89ms | 1.26x |
| bb_find_tree | 600µs | 3.14ms | **0.19x** |
| bb_gzip | 1.27ms | 3.96ms | **0.32x** |
| bb_tar_extract | 1.64ms | 6.44ms | **0.25x** |

Kevlar is 2-6x faster than Linux across most BusyBox workloads. The
one exception is `bb_dd` (1.26x slower) which is dominated by tmpfs
`Vec::resize` allocations — a known area for future optimization with
page-backed storage.

**Micro-benchmarks** (42 syscalls, Kevlar KVM vs Linux KVM):

- 19 faster, 14 at parity, 5 marginally slower, 4 regressions
- Key wins: `brk` 450x, `mmap_munmap` 5x, `signal_delivery` 2x,
  `mprotect` 1.6x, `stat` 1.4x
- Regressions in workload benchmarks (`exec_true` 2.6x, `shell_noop`
  5.4x, `pipe_grep` 15x, `sed_pipeline` 21x) — these are fork+exec
  heavy and will be addressed in M9.6

## Source fixes

Four test files had compilation errors masked by Docker's older musl:
- `benchmarks/fork_micro.c`: missing `#include <sys/stat.h>`
- `testing/mini_storage.c`: `struct statx` guarded with
  `#ifndef STATX_BASIC_STATS` for newer musl
- `testing/busybox_suite.c`: function name `do_dd_diag` used as
  lvalue, fixed to use `dd_diag_mode` variable
- `testing/contracts/scheduling/futex_requeue.c`: missing
  `#include <time.h>`

## What's next

The micro-benchmark regressions in fork+exec workloads point to
overhead in the process creation and pipe paths. M9.6 will be a
focused optimization pass to bring these back to Linux parity.
The Alpine integration test (layers 3-7) depends on chroot + dynamic
linking from ext2, which is the next area of investigation.
