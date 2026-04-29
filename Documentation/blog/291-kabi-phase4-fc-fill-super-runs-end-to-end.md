# 291 — Phase 4: fc_fill_super runs end-to-end through erofs's superblock validation

The Phase 4 iterative bring-up started this session.  Three
specific bugs surfaced and got fixed; together they take erofs's
`fc_fill_super` from "HVF-unclassifiable abort partway through"
to "runs end-to-end and returns a clean errno" — even if the
final return is still `-EINVAL` from a downstream stub.

The pattern from previous phases held: each fix moved the failure
forward by a measurable amount, and the next blocker was specific
and named.  Two were ABI compat issues (struct offset, calling
convention); one was a stub gap.

## Bug 1: SCS hand-off on every dispatch

Linux 7.0 modules built with `CONFIG_SHADOW_CALL_STACK=y` use
`x18` as the per-task SCS pointer.  Every function prologue
executes `str x30, [x18], #8`; epilogues do
`ldr x30, [x18, #-8]!`.

Kevlar's existing `call_module_init_with_scs` allocated a 1 KiB
SCS for `init_module` only.  But the deeper fs dispatch paths
(`init_fs_context`, `ops->get_tree`, `fc_fill_super`) called the
module via raw `transmute(ptr)`:

```rust
let init_fc_fn: InitFsContextFn = unsafe { core::mem::transmute(...) };
let rc = unsafe { init_fc_fn(fc) };  // x18 = whatever Rust left it
```

`x18` was whatever Rust kernel code happened to have it as.
Sometimes the SCS write landed on writable memory and worked
(silently corrupting it); sometimes it didn't — manifesting as
an HVF assertion (`Assertion failed: (isv)`) deep inside
fc_fill_super at an instruction with no obvious cause.

Fix: extract `call_with_scs_1(f, arg0)` /
`call_with_scs_2(f, arg0, arg1)` helpers and route every
dispatch into the .ko through them.

```rust
let rc = super::loader::call_with_scs_2(
    fill_super as *const (), sb as usize, fc as usize,
) as i32;
```

The asm:

```rust
core::arch::asm!(
    "str x18, [sp, #-16]!",   // save Rust's x18 on the stack
    "mov x18, {scs}",          // give the .ko a real SCS
    "blr {fp}",                // call into the .ko
    "ldr x18, [sp], #16",      // restore
    scs = in(reg) scs_ptr, fp = in(reg) f,
    in("x0") arg0, in("x1") arg1,
    lateout("x0") result,
    clobber_abi("C"),
);
```

The first attempt used `mov x9, x18` to save x18 in a register.
`x9` is caller-saved per AAPCS64, so the .ko's deep call chain
clobbered it and the restore at the end of the asm wrote
garbage into x18.  The stack-save form is robust against any
register clobber the .ko's chain does.

## Bug 2: `sb->s_fs_info` offset (320 → 912)

`struct_layouts.rs` had `SB_S_FS_INFO_OFF = 320` marked **GUESS**.
Verified via `erofs_read_superblock` disasm at offset `0x43ec`:

```asm
ldr x20, [x22, #912]   ; x22 = sb arg → x20 = sb->s_fs_info
```

The wrong offset meant we propagated `fc->s_fs_info → sb[+320]`
(some unused stretch of the 4 KiB sb buffer).  Erofs read
`sb[+912]` and got the zero-init value; then
`strb [0, #223]` wrote to user-VA `0xDF` (which silently
succeeded because TBI1=1 ignores the top byte and TTBR0 happens
to be set up); then later `ldrb [0, #223]` read `0` back as the
filesystem block-size-bits.  Crc32c got called with
`len = (1 << 0) - 0x400 - 8 = 0xFFFFFFF9` — runs ~52 MiB past the
end of our 4 KiB data buffer → synchronous external abort at
paddr `0x80000000` (well past the 1 GiB of RAM).

Fix: one line in `struct_layouts.rs`.

The `crc32c(crc=..., data=0xffff..., len=4294967289)` log line —
added briefly for diagnosis — was the smoking gun.  In retrospect,
guessed offsets in a layered Linux struct are the easiest source
of these "everything looks OK but the next-next-next thing is
wrong" bugs.

## Bug 3: `_erofs_printk` stub

While debugging Bug 2 we wired `_erofs_printk(sb, fmt, ...)` to
`log::warn!` so erofs's own diagnostic output would show up.  The
shim strips the leading `\x01N` priority byte that Linux's
`KERN_ERR`-class macros prepend, then routes through the same
`format_into` machinery as the existing `printk` shim.

Result: erofs printk fires through to the boot log:

```
[mod] 3erofs (device ): 0xffff00004279daf8
```

(The format string is `"%s (device %s): %pV\n"`.  We don't
expand `%pV` yet — the actual error message lives behind that
pointer.  Adding `%pV` is a tiny extension to `printk_fmt`.)

## Bug 4 (deferred): stub null-returns from inode/dentry allocators

`iget5_locked`, `new_inode`, `d_make_root` were returning null.
Fixed with real impls that `kmalloc` + zero-fill the struct,
populate the few fields we know of (`i_sb`, `i_state=I_NEW`,
`d_flags=DCACHE_DIRECTORY_TYPE`, `d_parent=self`, `d_inode`),
and run any callback (e.g. `iget5_locked`'s `set` arg, which
erofs uses to stash the inode number).

But — these aren't the path erofs takes today.  fc_fill_super
returns `-EINVAL` before reaching `erofs_iget`.  The remaining
`-EINVAL` comes from one of several earlier `mov w4, #-22`
paths in fc_fill_super.

## End state

Boot log (with `kabi-load-erofs=1 kabi-fill-super=1`):

```
kabi: dispatching erofs ops->get_tree(fc=0xffff00007fc34810)
kabi: filp_open_synth("/lib/test.erofs")
kabi: get_tree_nodev_synth: sb=0xffff00007fc40010, calling fill_super
kabi: super_setup_bdi(sb=...)
kabi: read_cache_folio: fake_page=...c1f36000 data_va=...7cd80000
[mod] 3erofs (device ): 0xffff...
kabi: get_tree_nodev_synth: fill_super returned -22 — bailing
panic: ec=0x0, esr=0x2000000, pc=0x0, far=0x0   ← post-return
```

Compare to last session:

```
kabi: read_cache_folio: ...
Assertion failed: (isv), function hvf_handle_exception
```

We:
  * cleared the HVF abort (was a translation/external abort
    derailed by struct-offset and SCS bugs).
  * see erofs's own printk fire (proving fc_fill_super reached
    its diagnostic path).
  * get a clean `-EINVAL` from fill_super.

The post-fill_super `PC=0` panic is the new top-of-stack issue
— most likely fc_fill_super's failure path corrupted some
return address or the SCS area.  Phase 4f will address it
alongside fixing the remaining sbi field offsets so erofs's
mount path doesn't hit `-EINVAL` in the first place.

## Status

| Bug | Resolution |
|---|---|
| SCS x18 hand-off for fs ops | ✅ stack-save form |
| `sb->s_fs_info` offset 320→912 | ✅ verified via disasm |
| `_erofs_printk` shim | ✅ routes to `log::warn!` |
| `iget5_locked`/`new_inode`/`d_make_root` real | ✅ |
| Remaining `-EINVAL` source | ⏳ likely `sbi[+32]`/other guess |
| Post-return `PC=0` panic | ⏳ |
| LXDE 8/8 default boot | ✅ no regression |

Three commits this session:

  * **`Phase 4 prep: real crc32c + auto-generate 4KB-block test fixture`**
    — `kernel/kabi/fs_stubs.rs::crc32c` real impl,
    `tools/build-initramfs.py` inline mkfs.erofs.
  * **`Phase 4a-4b: SCS hand-off for fs ops + correct
    sb->s_fs_info offset (912)`** — three layered ABI fixes.
  * **`Phase 4c-4e + SCS x18 stack-save: real
    iget5_locked/new_inode/d_make_root`** — alloc the inode
    and dentry that erofs would call once mount progresses
    past current blocker.

The pattern that's working: each fault has ONE specific cause
(a wrong constant, a wrong offset, a wrong asm clobber list)
and the disasm + log instrumentation together pinpoint it
within an hour or two.  The bringup loop is methodical, just
slow.
