# Blog 112: ext4 mmap writeback, comprehensive test suite, and OpenSSL SIGSEGV root cause

**Date:** 2026-03-23
**Milestone:** M10 Alpine Linux

## Summary

Five kernel bugs fixed, a comprehensive ext4 + dynamic linking test suite
built (19/22 pass), and the root cause of Alpine's dynamic binary failures
identified: SIGSEGV inside OpenSSL's `RAND_status()` during DRBG initialization.

## Bug Fixes

### 1. Buddy Allocator Bitmap Guard

The buddy allocator's `free_coalesce` merged freed blocks with pages that
were in the PAGE_CACHE (not in the buddy's free lists). Added a global
allocation bitmap (32KB static, 1 bit per 4KB page) that prevents coalescing
with pages whose bitmap bit is set (allocated). Fixes kernel stack corruption
under heavy fork/exit workloads that caused the BusyBox test suite crash
via `sh -c` (vfork path).

### 2. Signal Nesting on User Stack

Nested signal delivery (e.g., SIGALRM during SIGCHLD handler) overwrote
the single kernel-side `signaled_frame` slot. Changed to:
- Save full register context (19 fields, 152 bytes) on the USER STACK
- Changed `signaled_frame` to `ArrayVec<PtRegs, 4>` (nesting stack)
- Parse and store `sa_mask` from `rt_sigaction`
- Each nested signal gets independent save/restore

### 3. brk Heap VMA Overlap

`expand_heap_to` called `extend_by(grow)` on existing anonymous VMAs
without checking if the extension overlapped OTHER VMAs. The heap grew
into musl's `.text` segment, creating overlapping RW+RX VMAs (3924 VMAs!).
Fix: verify extension range against all other VMAs before extending.

### 4. MAP_SHARED Writeback on munmap

`munmap` did not write back dirty MAP_SHARED pages to files. When
`apk.static` installed packages via `mmap(MAP_SHARED) + memcpy + munmap`,
file data was lost — installed binaries were 0-byte empty files.
Fix: before freeing pages from shared file VMAs, write page data back
to the file via the inode's write method.

### 5. Device Node rdev Numbers

`/dev/null`, `/dev/zero`, `/dev/urandom` reported `major:minor = 0:0`.
Fixed to `1:3`, `1:5`, `1:9` respectively. Required by OpenSSL to
validate `/dev/urandom` as a random device.

## Test Suite (19/22 pass)

Built `test_ext4_comprehensive.c` — a statically-linked musl diagnostic
binary that tests every ext4 I/O mechanism and dynamic binary execution:

| Category | Tests | Status |
|----------|-------|--------|
| **File I/O** | write, writev, pwrite/pread, append, ftruncate, mmap_shared, mmap_unaligned, sendfile | 8/8 PASS |
| **Directory** | mkdir/readdir, rename, unlink, symlink | 4/4 PASS |
| **Permissions** | chmod | 0/1 FAIL (not persisted on ext4) |
| **Dynamic** | busybox, openrc, file | 3/3 PASS |
| **Dynamic** | curl --version, apk --version | 0/2 FAIL (SIGSEGV in OpenSSL) |
| **Integrity** | curl binary checksum | 1/1 PASS (byte-identical to package) |
| **Library** | LD_PRELOAD all 7 curl deps | 7/7 PASS (constructors work) |

**Benchmarks:** Write 485 KB/s, Read 2.8 GB/s, Create/delete 13ms/op.

## Investigation: Why curl/apk/gcc Fail

### The Symptom

Every Alpine program linking `libcrypto.so.3` (curl, apk, gcc) silently
exits with code 1 and produces zero output. BusyBox, OpenRC, and `file`
(which don't link libcrypto) work fine.

### The Hunt

1. **mmap writeback?** No — files are byte-identical (checksum verified)
2. **ELF corruption?** No — valid headers, correct NEEDED entries
3. **Library constructors?** No — all 7 pass via LD_PRELOAD
4. **Missing syscalls?** No — full trace shows zero errors
5. **VMA overlaps?** No — addresses are sequential

### The Breakthrough: Debug Curl

Built a custom `curl-debug` binary in Alpine Docker that wraps
`curl_global_init()` with debug prints:

```
DBG: step1 - before curl_version
DBG: step2 - curl_version='libcurl/8.14.1 OpenSSL/3.3.6 zlib/1.3.1...'
DBG: step3 - before curl_global_init
(exit=1)
```

`curl_version()` works, but `curl_global_init()` never returns!

### The Root Cause: SIGSEGV in RAND_status()

Built an `ssl-test` binary that calls OpenSSL functions one at a time:

```
1: getrandom=16 (OK)
2: /dev/urandom open=3, read=16 (OK)
3: OpenSSL_version='OpenSSL 3.3.6' (OK)
4: RAND_status -> SIGNAL: caught signal 11 (SIGSEGV!)
```

**`RAND_status()` crashes with SIGSEGV.** The DRBG code dereferences
a bad pointer during initialization. `getrandom()` and `/dev/urandom`
work fine — the crash is in OpenSSL's internal dispatch table, not
the entropy source.

### Hypothesis

The most likely cause is a **relocation issue**. Our kernel's
`prefault_writable_segments` eagerly maps the writable data segments
of the main executable and interpreter BEFORE the dynamic linker
applies RELR relocations. If the prefaulted pages have stale content
(unpatched function pointers in libcrypto's GOT), the DRBG dispatch
table points to wrong addresses.

Programs with few libraries (BusyBox, file) don't hit this because
their GOT is small. Programs with many libraries (curl, apk) have
large GOTs that need more relocation patches.

## Status

| Feature | Status |
|---------|--------|
| Alpine boot + OpenRC | **Working** |
| apk.static update/add | **25,397 packages** |
| BusyBox wget HTTP | **528 bytes from example.com** |
| BusyBox dynamic | **Working** (--help output) |
| file dynamic | **Working** (libmagic) |
| curl/apk/gcc dynamic | **SIGSEGV in RAND_status()** |
| ext4 write/mmap/sendfile | **All pass** |
| Test suite | **19/22 pass** |
