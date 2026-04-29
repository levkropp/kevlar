# 299 — Phase 7: `mount("/lib/test.erofs", "/mnt/erofs", "erofs", MS_RDONLY, NULL)` from C

Phase 7 closes the kABI mount arc.  A static-musl C program
running as PID 1 calls `mount(2)` for `"erofs"`, then `opendir`/
`readdir`/`open`/`read` against `/mnt/erofs`, and gets back the
exact bytes that `mkfs.erofs` packed into the test image.
End-to-end, through the standard libc syscall surface, against
erofs's compiled `.ko` mount machinery.

Final test output:

```
TEST_START kabi_mount_erofs
PASS mount_erofs
PASS opendir_mnt_erofs
PASS readdir_hello.txt
PASS readdir_info.txt
PASS open_hello.txt
PASS read_hello.txt_size
PASS read_hello.txt_bytes
PASS read_hello.txt_offset_6
RESULTS: 8 passed, 0 failed
TEST_PASS kabi_mount_erofs
```

Run via `make ARCH=arm64 test-kabi-mount-erofs`.

## Two Phase-7-only fixes

The kernel-side stack worked end-to-end at the end of Phase 6.
Bringing up the userspace boundary surfaced two new bugs that
the in-kernel boot probe had been silently masking.

### 1. NULL `inode->i_sb` in the synth backing file

`erofs_fc_fill_super` at offset `+0x25c` in `.ko` text:

```asm
ldr   x0, [x20, #16]      ; x0 = sbi->dif0.file
ldr   x0, [x0, #32]       ; x0 = file->f_inode
ldr   x1, [x0, #40]       ; x1 = inode->i_sb
ldr   x2, [x1, #48]       ; x2 = i_sb->s_op  ← read at NULL+0x30
```

Erofs is checking `sbi->dif0.file->f_inode->i_sb->s_op == &shmem_ops`
to detect shmem-backed mounts.  Our `filp_open_synth` allocates
a fresh inode for the backing file and only sets `i_mode`,
`i_size`, `i_mapping` — NOT `i_sb`.  So `i_sb = NULL` and the
deref reads VA `0x30`.

In the **boot-probe context** (PID 0, no user process), TTBR0 is
loaded with the boot page tables which happen to map low VAs
identity-style — the read at `0x30` returns garbage but
*doesn't fault*.  The garbage compared not-equal to the shmem
constant, so erofs took the "not shmem" path and continued.

In **userspace process context** (PID 1, real process), TTBR0 is
the user process's page tables.  Like every Linux process, the
first page is unmapped (NULL pointer protection).  The read at
`0x30` triggers a translation fault → kernel page fault →
panic.

Fix in `kernel/kabi/fs_synth.rs::filp_open_synth`: kmalloc a
zero-filled "host_sb" buffer and set
`inode->i_sb = host_sb`.  Erofs reads
`i_sb->s_op = host_sb[+48] = 0`, compares not-equal to the
shmem constant, takes the not-shmem path.  No real backing
super_block needed — just a non-NULL placeholder so the deref
doesn't fault.

The lesson: **boot-probe success doesn't imply userspace
success**.  Different page-table contexts mask different bugs.
Userspace tests are the real bar.

### 2. `mkdir("/mnt/erofs")` returns `EROFS` at runtime

The C test originally tried to create the mount point at
runtime:

```c
mkdir("/mnt", 0755);
mkdir("/mnt/erofs", 0755);
mount("/lib/test.erofs", "/mnt/erofs", "erofs", MS_RDONLY, NULL);
```

But Kevlar's initramfs is mounted read-only.  `mkdir` returned
`-EROFS (-30)`, and `mount` then tried to create
`/mnt/erofs` itself via the parent directory's `create_dir`,
which also returned `EROFS` — surfacing as `mount` failing with
`errno=30`.

Fix in `tools/build-initramfs.py`: pre-create `/mnt/erofs` in
the rootfs at build time.  At runtime the directory already
exists, so `mount` finds it via `lookup_dir` and proceeds.

Side note for future filesystems: when we add Phase-N support
for a writable VFS layer (tmpfs already supports it), this
becomes a non-issue.

## What Phase 7 actually contains

Three files, no kernel-side changes (other than the
backing-inode `i_sb` fix and a probe gate):

  * **NEW** `testing/test-kabi-mount-erofs.c` — 8-check libc
    test program.
  * **EDIT** `tools/build-initramfs.py` — compile entry +
    pre-created `/mnt/erofs`.
  * **EDIT** `Makefile` — `make ARCH=arm64 test-kabi-mount-erofs`
    target.
  * **EDIT** `kernel/main.rs` — gate the in-kernel mount probe
    behind `kabi-mount-probe=1` so it doesn't consume the
    single-mount slot before userspace runs.
  * **EDIT** `kernel/kabi/fs_synth.rs::filp_open_synth` — set
    `inode->i_sb` to a kmalloc'd host_sb buffer.

Cmdline for the test:

```
kabi-load-erofs=1 kabi-fill-super=1 init=/bin/test-kabi-mount-erofs
```

  * `kabi-load-erofs=1` triggers loading erofs.ko at boot.
  * `kabi-fill-super=1` enables fill_super dispatch.
  * `init=...` runs the test as PID 1.
  * `kabi-mount-probe=1` is OMITTED so the in-kernel probe
    skips, leaving the single-mount slot for userspace.

## What this completes

The kABI Linux .ko mount stack is now functional end-to-end at
the userspace boundary:

```
userspace test-kabi-mount-erofs
  ↓ libc mount(2)                      ← Phase 7
  ↓ kernel/syscalls/mount.rs           ← K33 routing
  ↓ kabi::fs_adapter::kabi_mount_filesystem  ← Phase 5+
  ↓ erofs.ko init_fs_context           ← K33
  ↓ erofs.ko fc_get_tree → filp_open_synth   ← K34, this blog Phase 7 fix
  ↓ get_tree_nodev_synth → fc_fill_super     ← Phase 4
  ↓ KabiFileSystem(erofs)              ← Phase 5 v1
  ↓ root_fs.mount_readonly("/mnt/erofs", fs) ← VFS namespace
userspace opendir + readdir          ← KabiDirectory  ← Phase 5 v3
userspace open + read                ← KabiFile      ← Phase 6
```

Every layer above proven by a `PASS` line.

## Status

| Phase | Status |
|---|---|
| 1 — sp_el0 task_struct mock | ✅ |
| 2 — VA aliasing | ✅ |
| 3b — VMEMMAP shadow + folio_address | ✅ |
| 4 — fc_fill_super reaches d_make_root | ✅ |
| 5 v1-v4 — KabiDirectory + lookup | ✅ |
| 6 — KabiFile::read | ✅ |
| 7 — userspace mount(2) integration | ✅ |

**The seven-phase plan is done.**  A test program written
against Linux's libc API mounts and reads files from a
filesystem implemented entirely by Linux's compiled `.ko`,
loaded into a Rust microkernel.

## Next

The natural follow-on is **task #99**: build `ext4.ko` +
`jbd2.ko` + `mbcache.ko` in Docker against the same Linux 7.0
tree, drop them in `/lib/modules/`, and route the existing
`mount -t ext4` syscall path through `kabi_mount_filesystem`.
The compat layer should be 80%+ ready — `ext4.ko` uses the
same VFS surface erofs does, and we have stubs for the journal
infrastructure in place from K33 Phase 2.

Beyond that: writable filesystems, multi-mount support, and
eventually GPU/input/network drivers — the same playbook
extended to the rest of `drivers/`.
