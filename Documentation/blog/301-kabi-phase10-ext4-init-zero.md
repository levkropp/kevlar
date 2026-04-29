# 301 — Phase 10 (ext4 arc): `ext4 init_module returned 0` in two stubs

Phase 9 left ext4.ko fully linked but bailing out of `init_module`
with `-ENOMEM`.  Phase 10 closes that gap by converting two
specific bulk stubs from null-returning to non-null-returning.
Total edit: ~50 lines.

End-state log:

```
kabi: ext4 loaded; runtime exports = 109
kabi: all external symbols resolved for /lib/modules/ext4.ko
kabi: register_filesystem(ext4) — registered (count now 3)
kabi: ext4 init_module returned 0
```

ext4 is now registered in the kABI fs registry; the next
`mount -t ext4 ...` syscall will dispatch through to its
`init_fs_context` callback.

## The trace

`init_module` (offset 0x4bc in `.init.text`) calls sub-inits in a
fixed order:

```
es → pending → post_read_processing → pageio → system_zone → sysfs → mballoc
```

Each sub-init has the same body shape: allocate-something, `cbz
x0, error`, return 0 on success.  Our existing `slab.rs` shims
make `__kmem_cache_create_args` and friends return valid
non-null cache descriptors, which gets ext4 through `es`,
`pending`, `pageio`, and `system_zone`.

The first failure was at `ext4_init_post_read_processing+0x90`:

```asm
46c:  bl mempool_create_node_noprof  ; bulk stub returns NULL
470:  str x0, [x20]                   ; store result
474:  mov x1, x0
47c:  cbnz x1, 48c <ok>               ; null → fall through to error
480:  ldr x0, [x19, #8]
484:  bl kmem_cache_destroy
488:  b 444 <ext4_init_post_read_processing+0x68>  ; → -12
```

The auto-generated bulk stub `pub extern "C" fn
mempool_create_node_noprof(...) -> *mut c_void { null_mut() }`
was the culprit.

The second failure (which would have surfaced after fixing
mempool) was in `ext4_init_sysfs+0x28`:

```asm
6a4:  bl kobject_create_and_add  ; bulk stub returns NULL
6a8:  str x0, [x20]
6ac:  cbz x0, 72c <error>        ; null → -12
```

## The fix

Two surgical edits:

**1. `kobject_create_and_add` — promoted from bulk to `kabi/kobject.rs`**

```rust
#[unsafe(no_mangle)]
pub extern "C" fn kobject_create_and_add(
    name: *const c_char, _parent: *mut KobjectShim,
) -> *mut KobjectShim {
    let boxed = Box::new(KobjectShim { inner: core::ptr::null_mut() });
    let raw = Box::into_raw(boxed);
    let _ = ensure_inner(raw);
    if !name.is_null() {
        // best-effort name copy ...
    }
    raw
}
```

Returns a real heap-allocated `KobjectShim` with a populated
inner refcount — same pattern as the existing `kobject_init`.
The pointer satisfies ext4's `cbz x0` non-null check.  We don't
actually hook into a `/sys/` tree; that's deferred until
something needs sysfs visibility from userspace.

**2. `mempool_create_node_noprof` — `fake_alloc()` helper in bulk_stubs**

```rust
fn fake_alloc() -> *mut c_void {
    super::alloc::kmalloc(64, super::alloc::__GFP_ZERO)
}

pub extern "C" fn mempool_create_node_noprof(...) -> *mut c_void {
    fake_alloc()
}
```

64-byte zeroed kmalloc.  ext4 stashes the pointer for later use
in the bio-completion path; init_module never reads back from it.
Real Linux allocates a per-cpu mempool here, which we don't need
for an RO mount that doesn't issue async I/O.

## Why this was so cheap

Two reasons Phase 10 stayed under an hour:

  1. **The bulk-stub strategy from Phase 9 was exactly right.**
     261 stubs returning NULL meant ext4 link succeeded immediately,
     and all init failures presented as `-ENOMEM` at one of N
     well-defined call sites.  We just walked the disasm of
     init_module, found the first failure, fixed it, repeated.

  2. **ext4's init path has no business logic to satisfy.**  The
     sub-inits are pure registration/allocation setup.  Returning a
     non-null pointer (any non-null pointer) is enough to clear
     each gate.  No struct fields read, no dependencies cascade.

The mount path won't be this gentle — `fc_fill_super` actually
reads field offsets and does I/O.  But for `init_module`, we're
in cheap territory.

## Status

| Phase | Status |
|---|---|
| 8 — inter-module exports | ✅ |
| 9 — mbcache + jbd2 + ext4 link | ✅ |
| 10 — ext4 init_module = 0 | ✅ |
| 11 — block_device synth (`bdev_file_open_by_path`) | ⏳ |
| 12 — fc_fill_super for ext4 (in-kernel mount) | ⏳ |
| 13 — userspace `mount -t ext4` | ⏳ |

Default boot 8/8 LXDE clean; Phase 7 erofs test still 8/8 PASS.

The fs registry now has three entries (erofs from existing
boot probe, plus the two from this commit):

```
kabi: register_filesystem(ext4) — registered (count now 3)
```

`mount -t ext4 /dev/vda /mnt/ext4` will now enter our kABI
dispatch, just like erofs did at the end of Phase 4 — only to
crash on `bdev_file_open_by_path` returning `-ENODEV`.  That's
where Phase 11 begins.
