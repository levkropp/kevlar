# 286 — K35: VA aliasing alone isn't enough; sp_el0 ABI also blocks

K34 ended with a clean characterization: erofs.ko's
`fc_fill_super` faults on `0xffff_8010_0000_0400`, an
address in Linux's `PAGE_OFFSET` region we don't have
mapped.  K35 was scoped as "set up alias mappings to put
our paddrs at Linux-compat VAs" — the natural first
unblock.

Within a few hours that turned out to be one of *two*
layered Linux-ABI obstacles, not one.  Aliasing alone
moves the trap class but doesn't fix it; the deeper issue
is `sp_el0` — Linux 7.0 arm64 uses it as the per-CPU
current-task pointer, and Kevlar uses it for the user
stack pointer.  This post is the writeup of what the
investigation surfaced.

## The PGD alias attempt

Kevlar's arm64 boot maps `PGD[0]` of TTBR1 to a PUD that
covers physical 0-4GB at kernel VA `0xffff_0000_0000_0000`.
Linux on the test image (Ubuntu's `linux-modules-7.0.0-14`,
config `ARM64_VA_BITS=52` with VA_BITS_MIN fallback to 48)
expects `PAGE_OFFSET = 0xffff_8000_0000_0000`.

Adding a single PGD entry alias at `PGD[256]` (= bits[47:39]
of `0xffff_8000_0000_0000`'s lower 48) pointing at the same
PUD as `PGD[0]`:

```asm
adrp    x0, __kernel_pgd_ttbr1
add     x0, x0, :lo12:__kernel_pgd_ttbr1
add     x0, x0, #(256 * 8)
str     x1, [x0]               // same PUD as PGD[0]
```

Result: trap class shifts from `ESR=0x96000004` (translation
fault L0 — PGD entry missing) to `ESR=0x96000005` (translation
fault L1 — PGD valid, PUD entry missing).  At least the
PGD walk worked.

But the **specific VA** that erofs computes is
`0xffff_8010_0000_0400` — decoded:

  * PGD index = bits[47:39] of `0x_8010_0000_0400` = 256 ✓
  * **PUD index = bits[38:30] = 64**
  * PMD index = 0
  * PT index = 0
  * Page offset = `0x400`

Our PUD has 4 entries (indices 0-3) covering 0-4GB physical.
PUD[64] is 64GB into the alias region — we don't have RAM
there at all.

So the alias didn't unblock anything.  It's also not just
a matter of growing the PUD: the question is *why is erofs
generating a VA at PAGE_OFFSET + 64GB?*

## The real culprit: sp_el0

Disasm of `erofs_read_superblock` (which `fc_fill_super`
calls right after `super_setup_bdi`):

```
00000000000043a8 <erofs_read_superblock>:
    43a8: nop
    43ac: nop
    43b0: paciasp
    ...
    43d8: mrs   x0, sp_el0           ← read current task pointer
    43dc: ldr   x1, [x0, #1912]      ← read task->stack_canary
    43e0: str   x1, [sp, #40]        ← stash for stack-protect
    ...
```

Linux 7.0 arm64 stores `current` (the running task's
`task_struct *`) in `sp_el0`.  Functions read it via
`mrs x0, sp_el0`, then dereference fields at fixed offsets:
`current->stack_canary` (at +1912), `current->pid`,
`current->mm`, etc.

Kevlar's arm64, by contrast, **uses sp_el0 to hold the user
stack pointer** — saved on EL0→EL1 entry, restored on
EL1→EL0 exit.  When kernel code is running, sp_el0 holds
whatever was there when we entered the kernel: typically
the user stack, or zero, or stale data.

So when erofs reads `sp_el0`, it gets a value that's not a
`task_struct *`.  `[sp_el0 + 1912]` lands in random memory.
The FAR `0xffff_8010_0000_0400` decodes as
`sp_el0 + 0x778` for some `sp_el0` near
`0xffff_800f_ffff_fc88`.

## The two-layer compat requirement

To make Linux fs `.ko` mounts work, Kevlar needs both:

  **Layer A (K35): VA aliasing.**  Kernel direct map must
  be accessible at Linux's `PAGE_OFFSET` (`0xffff_8000_...`
  with VA_BITS=48 fallback) so erofs's `kmap_local_page`
  inline expansions produce valid VAs.

  **Layer B (K36): `sp_el0` task-struct mock.**  When
  kernel code is running, `sp_el0` must hold a pointer to
  a Kevlar-managed `task_struct` mock that lays out fields
  at Linux 7.0's expected offsets — at minimum
  `stack_canary`, `pid`, `mm`, `tgid`, the few `current->X`
  fields fs code reads.  Kevlar's existing
  user-SP-via-sp_el0 contract has to coexist: write the
  task pointer on EL1 entry, write the user SP back on
  EL0 exit.

Each is multi-week kernel mm/sched work.  Together they're
the FreeBSD-LinuxKPI playbook for matching the host
kernel's runtime ABI to Linux's compile-time assumptions.

## What ships in K35 (commit `8f221b9`)

  * Struct offset corrections verified against
    disasm + matching kernel source:
      - `fs_context.s_fs_info` at +128
      - `fs_context.sb_flags` at +136
      - `fs_context.purpose` at +148
      - `super_block.s_blocksize_bits` at +20
      - `super_block.s_blocksize` at +24
      - `super_block.s_op` at +48
      - earlier guesses at these offsets were wrong by 4-12 bytes.
  * `get_tree_nodev_synth` propagates `fc->s_fs_info` into
    `sb->s_fs_info` (Linux's `vfs_get_super` does this; we
    weren't).
  * `fs_adapter.rs` consolidates magic offsets into a single
    `struct_layouts` module so K36+ can verify each at one
    place.
  * Documented the `sp_el0` finding in `boot.S` comments
    (PGD alias attempt reverted but the investigation
    record stays).

## Status

| Milestone | Status |
|---|---|
| K30-K33: kABI ext4 control flow | ✅ |
| K34: synth struct file + get_tree_nodev | ✅ (gated) |
| K35: PGD alias attempt, struct-offset fixes, sp_el0 finding | ✅ |
| K36: sp_el0 task-struct ABI mock | ⏳ |
| K37: full PUD aliasing + erofs mount working | ⏳ |
| K38+: dentry walk, KabiDirectory, ls /mnt/erofs | ⏳ |

## Honest accounting

The K34 blog estimated "1-2 weeks of careful page table work"
to get from "control flow runs" to "ls /mnt/erofs".  That
estimate covered Layer A (VA aliasing).  Layer B (sp_el0
task-struct mock) is a separate K36 effort — call it
another 1-2 weeks.  And then there's the iterative
struct-field bring-up the original K34 plan covered, which
becomes exercisable once both layers are through.

Realistic total time-to-mount: 4-8 weeks of focused work,
not 1.  This matches what FreeBSD took to get LinuxKPI
reliable for GPU drivers.

The good news: each layer surfaces a clean error before we
go further, and the kABI playbook keeps track of the
control flow well.  Default boot continues to pass 8/8,
nothing's regressed, and we've now scoped K36-K38 with
real evidence rather than guesses.
