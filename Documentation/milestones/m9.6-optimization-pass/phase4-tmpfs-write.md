# M9.6 Phase 4: tmpfs Write Path (bb_dd)

**Regression:** `bb_dd` (1.4x)
**Target:** Within 1.1x of Linux KVM

## The problem

`bb_dd` runs `dd if=/dev/zero of=/tmp/dd bs=4096 count=256` — writing
1MB to tmpfs.  Linux KVM: 4.9ms.  Kevlar: 6.7ms — 1.4x slower.

The raw syscall dd (no fork/exec) does 1MB in ~5ms on Kevlar, so the
overhead is partly fork+exec (~177µs) and partly the tmpfs write path
itself.

Tmpfs stores file data in `Vec<u8>`.  Each `write()` call:
1. Acquires `lock_no_irq()` (disables IRQs)
2. Checks if `reserve_exact` is needed
3. Calls `data.resize(new_len, 0)` — may reallocate + memcpy
4. Calls `reader.read_bytes()` — usercopy from user buffer
5. Releases lock

For 256 sequential 4KB writes growing a file from 0 to 1MB:
- ~9 reallocations (Vec grows: 4K → 8K → 16K → ... → 1MB with
  reserve_exact keeping it tight)
- Each reallocation memcpys the entire existing data
- Total memcpy overhead: 4K + 8K + 16K + ... + 512K ≈ 1MB of copying
- Plus 256 × usercopy of 4KB = 1MB of usercopy
- Total data movement: ~2MB

Linux's tmpfs uses page-backed storage — writes go directly to pages,
no reallocation or memcpy ever needed.

## Potential fixes

### Fix A: page-backed tmpfs (long-term)

Replace `Vec<u8>` with a page list.  Each write allocates pages as
needed and copies user data directly into them.  No reallocation, no
memcpy of existing data.  This is what Linux does.

This is a significant refactor of the tmpfs File struct and affects
read, write, truncate, and stat.

### Fix B: pre-size on truncate/fallocate (medium-term)

BusyBox dd doesn't call ftruncate before writing, but other tools do.
If we receive a truncate/fallocate hint, pre-allocate the Vec to the
target size.  Doesn't help dd but helps other workloads.

### Fix C: append-detect fast path (short-term)

If offset == data.len() (pure append), we can use `extend_from_slice`
which avoids the zeroing in `resize`.  The usercopy still happens but
we skip the redundant zero-fill.

## Success criteria

- `bb_dd` < 5.4ms (within 1.1x of Linux's 4.9ms)
