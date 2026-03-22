# Blog 106: GCC compiles C on Alpine/Kevlar — two ELF loader bugs squashed

**Date:** 2026-03-21
**Milestone:** M10 Alpine Linux

## The Milestone

GCC 14.2.0 runs on Kevlar. `gcc -o hello hello.c` compiles and links
successfully:

```
/ # gcc --version
gcc (Alpine 14.2.0) 14.2.0

/ # gcc -o /root/hello /root/hello.c
/ # echo $?
0
```

Two bugs prevented this — one in ELF loading, one in process management.

---

## Bug 1: AT_PHDR wrong for non-PIE (ET_EXEC) binaries

**Symptom:** `gcc --version` crashed with SIGSEGV at address `0xa001e8950`
(first attempt) then `0x40` (after partial fix). Every non-PIE dynamically-
linked binary crashed.

**Root cause:** The kernel passed `AT_PHDR` pointing to a stack-mapped copy
of the ELF header instead of the program headers in the loaded image.
musl's dynamic linker computes `load_bias = AT_PHDR - phdr[0].p_vaddr`,
so the wrong AT_PHDR produced a wildly incorrect load bias. For gcc
(base 0x400000, e_phoff=0x40), AT_PHDR was 0x40 instead of 0x400040.

**Fix:** `AT_PHDR = main_lo + main_base_offset + e_phoff`
- PIE (ET_DYN): `main_lo=0`, `offset=base` → `base + e_phoff` (unchanged)
- Non-PIE (ET_EXEC): `offset=0`, `main_lo=0x400000` → `0x400040` (now correct)

This was a one-line fix in `kernel/process/process.rs` but affects every
non-PIE binary on the system. All PIE binaries (curl, make, busybox,
openrc) were unaffected because the PIE path already set AT_PHDR correctly.

## Bug 2: clone() didn't add child to parent's children list

**Symptom:** `gcc` compiled but reported "failed to get exit status: No
child process" — wait4() returned ECHILD.

**Root cause:** `Process::clone_process()` added the child to the process
table and scheduler but forgot `parent.children().push(child)`. The
`fork()` path had this line; `clone()` didn't. Since musl's `posix_spawn`
uses `clone(CLONE_VM|CLONE_VFORK|SIGCHLD, ...)`, gcc's cc1/as/ld
subprocesses were invisible to wait4().

**Fix:** Added `parent.children().push(child.clone())` to the clone path,
matching fork.

---

## Alpine Image: build-base pre-installed

The Alpine ext4 image now includes `build-base` (gcc, binutils, make,
musl-dev) pre-installed from Docker, with the disk increased to 512MB
to accommodate the 245MB toolchain. This avoids the slow ~200MB download
over emulated networking.

---

## Known Issue: ext4 directory entry visibility

GCC-compiled binaries can't be executed immediately after compilation:
```
/ # gcc -o /root/hello /root/hello.c   # exit 0
/ # /root/hello                         # not found!
```

Freshly created files aren't visible to subsequent path lookups via a
different VFS traversal. The ext4 `create_file` writes the directory
entry to disk via `write_block`, but a new `Ext2Dir` instance reading
the same directory doesn't find the entry. Under investigation — likely
a block I/O coherence issue in the virtio-blk path.

---

## Results

| Feature | Before | After |
|---------|--------|-------|
| `gcc --version` | SIGSEGV | **gcc (Alpine 14.2.0) 14.2.0** |
| `gcc -o hello hello.c` | SIGSEGV | **exit 0** |
| `make --version` | worked | GNU Make 4.4.1 |
| Non-PIE ELF binaries | all crash | **all work** |
| clone() + wait4() | ECHILD | **correct** |
