# 293 — Phase 4 done: the post-return panic was a search-and-replace bug

Phase 4 is complete.  Erofs.ko's mount machinery runs
end-to-end, `fc->root` is populated with a real dentry
pointing at a real inode, and the call returns through every
kABI shim cleanly — no panic, no corrupted state, no leaked
exceptions.  Default boot 8/8 LXDE still passes.

The post-return `PC=0` panic that blocked Phase 4 from being
truly done in blog 292 turned out to be a one-line difference
between two near-identical helper functions.  Diagnosis took
longer than the fix.

## The bug

Two SCS hand-off helpers in `kernel/kabi/loader.rs`:

  * `call_with_scs_1(f, arg0)` — for 1-arg .ko entry points
    (`init_fs_context`, `ops->get_tree`).
  * `call_with_scs_2(f, arg0, arg1)` — for 2-arg
    (`fill_super(sb, fc)`).

Earlier in the session we noticed the saved x18 in
`call_with_scs_1` was using a fragile pattern:

```asm
mov x9, x18         ; save x18 in x9
mov x18, scs_ptr
blr fp
mov x18, x9         ; restore from x9
```

`x9` is caller-saved per AAPCS64.  fc_fill_super's deep call
chain clobbers it freely, so the `mov x18, x9` at the end
restores garbage into x18.

Fix: save to the stack instead.

```asm
str x18, [sp, #-16]!
mov x18, scs_ptr
blr fp
ldr x18, [sp], #16
```

But the fix only landed in `call_with_scs_1`.  `call_with_scs_2`
kept the broken pattern — different function, different edit
window, easy to miss.

When erofs's `ops->get_tree` was wrapped in `call_with_scs_1`
(safe), and its body called `fill_super` via
`call_with_scs_2` (broken), the inner asm's bad x18 restore
left x18 pointing into `scs_ptr2`'s region instead of back
to `scs_ptr1 + 8`.

After the inner asm exited, Rust resumed (with `+reserve-x18`,
Rust doesn't touch x18, so the corrupt value persisted).
Eventually `erofs_fc_get_tree`'s epilogue ran
`ldr x30, [x18, #-8]!` to pop its signed LR from what it
thought was the outer SCS — but x18 pointed inside the inner
SCS area whose contents were now random.  Erofs read junk,
authenticated it, `ret`'d to a junk LR.  The junk happened to
have all-zero lower bits → `PC = 0` → undefined synchronous
exception.

## How disasm-and-dump cracked it

The "single useful diagnostic" tool: extending the `EL1 EC=0`
exception handler to log the saved register frame instead of
just `pc/far/esr`.

```rust
// platform/arm64/interrupt.rs
let lr = unsafe { (*frame).regs[30] };
let fp = unsafe { (*frame).regs[29] };
let sp_saved = unsafe { (*frame).sp };
let x18 = unsafe { (*frame).regs[18] };
log::warn!("EL1 EC=0 details: lr={:#x} fp={:#x} sp={:#x} x18={:#x}",
           lr, fp, sp_saved, x18);
```

Boot log:

```
EL1 EC=0 details: lr=0x0 fp=0xffff00004279fd30
                  sp=0xffff00004207d000 x18=0xffff00004210f178
```

`lr=0x0`.  The CPU was returning to PC=0 because `ret` jumped
to LR=0.  And `x18 = 0xffff00004210f178` — completely outside
our outer scs_ptr (`0xffff00007fc40000`).  x18 had been
corrupted to point into a different heap region.

That made it obvious: the OUTER scs unwind was correct (we
verified via per-call dump), but the INNER scs unwind was
trashing x18.  Then the search located the inconsistency:
two near-identical asm blocks, only one fixed.

## Two more places needed the same wrap

Once `call_with_scs_2` was fixed, default boot regressed:
the cirrus PCI probe hit the same `Assertion failed: (isv)`
HVF abort.  Linux modules' driver probe functions also use
`paciasp + str x30, [x18], #8` prologues, and we were
calling them via raw `transmute` + direct call — same x18
hazard.

Two one-line changes:

  * `kernel/kabi/pci.rs::walk_and_probe` — route through
    `call_with_scs_2` (probe takes `pdev + pci_device_id`).
  * `kernel/kabi/virtio.rs::walk_and_probe` — route through
    `call_with_scs_1` (probe takes single `vdev`).

Both paths now green:

```
[default boot]
kabi: PCI walk: 'cirrus-qemu' probe returned 0
kabi: PCI walk: 'bochs-drm' probe returned -19   ← expected; bochs ID mismatch
kabi: virtio walk: 'virtio_input' probe returned 0
running init script: "/bin/sh"

[kabi-load-erofs=1 kabi-fill-super=1]
kabi: read_cache_folio: ... index=0 ...
kabi: new_inode: inode=...
kabi: iget5_locked: inode=... data=...
kabi: read_cache_folio: ... index=0 ...    ← root inode block
kabi: d_make_root: dentry=...
kabi: fill_super dispatch returned rc=0
kabi: get_tree_nodev_synth: fc->root = ... — mount succeeded
kabi: erofs ops->get_tree returned 0 — fc->root populated
kabi: erofs mount route returned ENOSYS (Phase 3 v1 expected)
running init script: "/bin/sh"
```

The mount runs end-to-end, `fc->root` reaches our adapter,
the value chain unwinds cleanly back through every shim.

## Phase 4 status

| Item | Status |
|---|---|
| `__GFP_ZERO` honored | ✅ |
| `I_NEW = 1 << 0` | ✅ |
| `SB_S_ROOT_OFF = 104` | ✅ |
| `SB_S_FS_INFO_OFF = 912` | ✅ |
| Real `iget5_locked` / `new_inode` / `d_make_root` | ✅ |
| `_erofs_printk` shim | ✅ |
| `crc32c` real impl | ✅ |
| `+reserve-x18` defensive | ✅ |
| call_with_scs_1/_2 use stack save | ✅ |
| PCI/virtio probe via SCS wrappers | ✅ |
| **fc_fill_super returns 0** | ✅ |
| **fc->root populated, return chain clean** | ✅ |
| **No PC=0 panic** | ✅ |
| Default boot 8/8 LXDE | ✅ |
| **Phase 4 done** | ✅ |

## Up next: Phase 5

Phase 5 is `KabiDirectory` — an adapter that takes the
`fc->root` dentry we now have and exposes
`Directory::lookup(name)` and `Directory::readdir(idx)` to
Kevlar's VFS.  The hard part is calling
`inode->i_op->lookup(parent, child_dentry, flags)` for
arbitrary names — which means more SCS-wrapped indirect
calls into erofs's compiled inode op table, and
synthesising a child `struct dentry` for each lookup.

The SCS infrastructure is now solid; the inode/dentry
struct synthesis is well understood; what remains is the
adapter glue.  Phase 5 entry point: `kabi_mount_filesystem`
in fs_adapter.rs, currently returns `Err(ENOSYS)` even on
success — wrap fc->root in a `KabiFileSystem` instead.

## Lessons

The diagnostic that broke this open — dumping the saved
register frame in the EL1 EC=0 handler — paid for itself
the first time it ran.  Six lines of code, kept in tree.
Future kABI calls that corrupt a return chain will surface
the same way.

The pattern holds: every Phase ends with an explicit
"what's the next blocker" answer, and the next phase
starts from a blocker that's named, located, and bounded.

Three commits this session beyond Phase 4 internals:

  * **`Phase 4g: kmalloc shims now honor __GFP_ZERO`** —
    one-bit fix that unblocked erofs's sbi struct.
  * **`Phase 4h: I_NEW = 1<<0 + SB_S_ROOT_OFF = 104`** —
    two struct constants matched to Linux 7.0.
  * **`Phase 4 complete: SCS hand-off everywhere`** —
    call_with_scs_2 + PCI probe + virtio probe.

LXDE 8/8 default boot still passes.
