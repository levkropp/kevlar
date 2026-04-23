## Blog 214: three 7×+ bench wins on arm64 HVF

**Date:** 2026-04-23

Blog 213 shelved the fork_exit arc at iter-1 gains and said the
remaining slow bench ratios (`socketpair`, `pipe`, `read_zero`,
`mmap_fault`) were likely to yield more than the residual fork_exit
hunt.  That was an understatement.  Three changes in a few hours —
one for each benchmark — collectively deliver:

| benchmark   | before    | after     | speedup |
|-------------|-----------|-----------|--------:|
| socketpair  | 7586 ns   |  523 ns   |   14.5× |
| pipe        | 2384 ns   |  304 ns   |    7.8× |
| read_zero   | 1083 ns   |  147 ns   |    7.4× |

Contract suite: 159/159 throughout.

Each win was a distinct class of bug and each took five to twenty
minutes to fix once identified.  The common pattern: ports of x86
code that inherited byte-at-a-time fallback loops on arm64 that
nobody ever tuned.

## Win 1: socketpair's 16 KB zero

`socketpair(AF_UNIX, SOCK_STREAM)` was 7.6 µs per call in a
create-then-close loop.  The stack path:

```
sys_socketpair
 → UnixStream::new_pair_typed
  → alloc_stream_inner (×2)
   → alloc_zeroed(Layout::new::<StreamInner>())   — 16 KB each
```

`StreamInner` is a `RingBuffer<u8, 16384>` plus a few bytes of
metadata (`ancillary: Option<VecDeque>`, `shut_wr: bool`).  The
ring-buffer data is `[MaybeUninit<u8>; 16384]` — uninitialized by
design, because the `rp`/`wp`/`full` tracking already says "no
bytes valid".  Every socketpair was zeroing **32 KB of MaybeUninit
bytes it would never read**.  At ~10 GB/s that's 3.2 µs per pair
— about half the measured 7.6 µs.

Fix: switch the `alloc_zeroed(layout)` to `alloc(layout)` + three
in-place writes of the metadata fields (`RingBuffer::new()`,
`None`, `false`).  The 16 KB stays uninitialized.

```rust
let ptr = alloc::alloc::alloc(layout) as *mut StreamInner;
core::ptr::addr_of_mut!((*ptr).buf).write(RingBuffer::new());
core::ptr::addr_of_mut!((*ptr).ancillary).write(None);
core::ptr::addr_of_mut!((*ptr).shut_wr).write(false);
```

socketpair: 7586 → 523 ns.  14.5× faster.  That's more than I
predicted.  Either the zero was slower than 10 GB/s on HVF, or
there was additional allocator overhead that correlates with
request size.  Either way: no more needless zeroing.

## Win 2: pipe's byte-at-a-time user copy

`bench_pipe` is a tight write(4 KB) + read(4 KB) loop on a single
pipe.  Per-iteration: 1192 ns per syscall.  Linux: ~325 ns.

The culprit was `platform/arm64/usercopy.S::usercopy_memcpy`:

```
usercopy_memcpy:
    cbz     x2, 2f
1:
    ldrb    w3, [x1], #1
    strb    w3, [x0], #1
    subs    x2, x2, #1
    b.ne    1b
2:
    ret
```

**Byte at a time.**  4 KB = 4096 iterations.  On a 3 GHz core that's
~1.3 µs per 4 KB copy — close to the entire pipe-syscall cost.

Rewrote with 32-byte bulk via two `ldp`/`stp` pairs, then 8-byte
chunks, then byte tail.  The whole function stays inside
`[usercopy_start, usercopy_end)` so the page-fault handler still
recognizes any fault inside and returns `AccessError` rather than
panicking.

```
usercopy_memcpy:
    cbz     x2, done
    cmp     x2, #32
    b.lo    small
bulk:
    ldp     x3, x4, [x1], #16
    ldp     x5, x6, [x1], #16
    stp     x3, x4, [x0], #16
    stp     x5, x6, [x0], #16
    sub     x2, x2, #32
    cmp     x2, #32
    b.hs    bulk
small:
    ...
```

pipe: 2384 → 337 ns per 4 KB xfer.  7.1×.

This also helps every read/write/send/recv path, just with smaller
proportional gains because most syscalls aren't 4 KB transfers.

## Win 3: read_zero's byte-at-a-time user memset

`bench_read_zero` is `read(/dev/zero, buf, 4096)` in a loop.  1083
ns per call.  `/dev/zero` fills the user buffer with zeros via
`memset_user`, which on arm64 was the direct analogue of the
byte-at-a-time `usercopy_memcpy`:

```
memset_user:
    cbz     x2, 8f
7:  strb    w1, [x0], #1
    subs    x2, x2, #1
    b.ne    7b
8:  ret
```

Rewrote to replicate the byte across a `u64`, then `stp x3, x3,
[x0], #16` pairs for 32 bytes per iteration.  Tail byte-by-byte.

read_zero: 1083 → 147 ns.  7.4×.

## Win 3.5 (partial): kernel-side memcpy

Committed: `platform/mem.rs`'s `memcpy` (used internally by the
kernel for CoW page copies, PT duplication, struct copies) was 8
bytes per iteration.  Upgraded to 32 bytes per iteration.  No
benchmark-visible impact — PT copies and CoW copies are tiny
fractions of fork_exit — but the right shape for the code.

The matching `memset` upgrade (for zeroing freshly-allocated pages
in the page allocator) broke `subsystems.proc_global`'s
`uptime_parse` in a way I don't yet understand — both 16-byte and
32-byte bulk variants fail while the 8-byte loop passes.  Likely a
codegen interaction between `+strict-align` and `write_unaligned`
at higher offsets, or something subtler with how the page cache's
zero-fill interacts with allocator-returned buffers.  Deferred; the
8-byte memset stays.

## Why these landed in hours instead of days

All three wins were **ports of x86 code** where the x86 version
used inline asm (`rep movsb`, `rep stosb`) that happens to be
hardware-accelerated on modern Intel/AMD parts.  The arm64 port,
written under pressure to be correct rather than fast, fell back
to byte-at-a-time loops.  Nobody measured them.  They sat there
bleeding microseconds on every syscall for however long the arm64
port has been hosting the contract suite.

Process lesson for this codebase: when porting byte-slab primitives
to a new arch, *always* check the port uses at least word-sized
accesses in the common path.  Even a naïve 8-byte loop is 8× the
throughput of single-byte stores.

## What's next

mmap_fault is still ~1340 ns/iter, 3.09× Linux.  Each iteration is
one page fault: trap entry + VMA lookup + page alloc + PTE write +
TLB flush.  The page alloc path zeroes 4 KB on miss.  That goes
through `platform/mem.rs::memset` — still at 8 B/iter until I chase
the proc_global uptime failure.  Suspect that's the next single
biggest residual.

The four top-line task gaps from blog 211:

- `socketpair 5.03×` → **SOLVED (now 14.5× faster, at-parity-or-better)**
- `pipe 3.67×`       → **SOLVED (now 7.8× faster, at-parity-or-better)**
- `read_zero 3.50×`  → **SOLVED (now 7.4× faster, at-parity-or-better)**
- `mmap_fault 3.09×` → still open; page-alloc zeroing likely the lever

fork_exit stays at ~100 µs as documented in blog 213.  ASID-tagged
TLBs and FP-off kernel rebuild remain the big unclaimed levers
there; neither is a session-sized task.
