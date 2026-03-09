# Optimized Usercopy and Copy-Semantic Frames

**Date:** 2026-03-08

---

With the safety profile feature flags in place, we've now implemented the first mechanisms that actually *differ* between profiles: optimized usercopy assembly (Phase 3) and copy-semantic page frames (Phase 4).

## Phase 3: Alignment-aware usercopy

The original `copy_from_user` / `copy_to_user` assembly was a flat `rep movsb` — one byte at a time regardless of buffer size. That's correct, but leaves performance on the table for the bulk copies that dominate page fault handling and large read/write syscalls.

The new implementation in `platform/x64/usercopy.S`:

```asm
copy_from_user:
copy_to_user:
    cld
    cmp rdx, 8
    jb .Lbyte_copy          ; Small buffers: byte copy

    ; Align destination to 8-byte boundary
    mov rcx, rdi
    neg rcx
    and rcx, 7
    jz .Laligned
    sub rdx, rcx
usercopy1:
    rep movsb               ; Copy leading unaligned bytes

.Laligned:
    mov rcx, rdx
    shr rcx, 3
usercopy1b:
    rep movsq               ; Bulk copy as qwords (8 bytes/iter)

    mov rcx, rdx
    and rcx, 7
    jz .Ldone
usercopy1c:
    rep movsb               ; Copy trailing bytes
.Ldone:
    ret
```

Three labeled instructions (`usercopy1`, `usercopy1b`, `usercopy1c`) instead of one. The page fault handler in `interrupt.rs` checks all three labels to distinguish "user page fault during usercopy" from "kernel bug":

```rust
let occurred_in_user = reason.contains(PageFaultReason::CAUSED_BY_USER)
    || frame.rip == usercopy1 as *const u8 as u64
    || frame.rip == usercopy1b as *const u8 as u64
    || frame.rip == usercopy1c as *const u8 as u64
    || frame.rip == usercopy2 as *const u8 as u64
    || frame.rip == usercopy3 as *const u8 as u64;
```

This is the same technique Linux uses — `_ASM_EXTABLE` entries that map faulting instruction addresses to fixup handlers. Ours is simpler since we just check if RIP matches a known usercopy label.

## Phase 4: Copy-semantic page frames (Fortress)

The key insight: in a safe kernel, `page_as_slice_mut(paddr)` returning `&'static mut [u8]` is dangerous. That reference can outlive the page mapping, alias with DMA buffers, or leak across ring boundaries. Under the Fortress profile, we replace it entirely.

`PageFrame` in `platform/page_ops.rs`:

```rust
pub struct PageFrame {
    paddr: PAddr,
}

impl PageFrame {
    pub fn new(paddr: PAddr) -> Self { ... }

    pub fn read(&self, offset: usize, dst: &mut [u8]) {
        assert!(offset + dst.len() <= PAGE_SIZE);
        unsafe { ptr::copy_nonoverlapping(src, dst, len); }
    }

    pub fn write(&mut self, offset: usize, src: &[u8]) {
        assert!(offset + src.len() <= PAGE_SIZE);
        unsafe { ptr::copy_nonoverlapping(src, dst, len); }
    }
}
```

No `&mut [u8]` ever escapes. The unsafe pointer operations are confined to the platform crate — Ring 0. Kernel code (Ring 1) can only copy data in and out through owned buffers.

The page fault handler becomes profile-conditional:

```rust
// Fortress: read file into stack buffer, copy to frame
#[cfg(feature = "profile-fortress")]
{
    let mut tmp = [0u8; PAGE_SIZE];
    file.read(offset_in_file, (&mut tmp[..copy_len]).into(), ...)?;
    PageFrame::new(paddr).write(offset_in_page, &tmp[..copy_len]);
}

// Other profiles: zero-copy direct write into page
#[cfg(not(feature = "profile-fortress"))]
{
    let buf = page_as_slice_mut(paddr);
    file.read(offset_in_file, (&mut buf[range]).into(), ...)?;
}
```

The cost: one extra 4KiB memcpy per demand-paged file read. The benefit: physical memory never appears as a Rust reference outside Ring 0. This eliminates an entire class of use-after-unmap and aliasing bugs.

## What's next

Phases 0-4 are complete:

| Phase | What | Status |
|-------|------|--------|
| 0 | Feature flags and Makefile integration | Done |
| 1 | Performance profile (concrete service types) | Done |
| 2 | Ludicrous profile (skip access_ok) | Done |
| 3 | Optimized usercopy | Done |
| 4 | Copy-semantic frames (Fortress) | Done |
| 5 | `catch_unwind` at service boundaries | Next |
| 6 | Capability tokens | Planned |
| 7 | Benchmarks and CI matrix | Planned |

Phase 5 is the hard one: `catch_unwind` requires `panic = "unwind"`, which means a bare-metal unwinder and a separate target spec. If it proves too complex, we'll use fail-stop logging instead.
