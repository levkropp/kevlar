# Blog 134: The getdents64 bug — how 3 lost directory entries broke Python3

**Date:** 2026-03-30
**Milestone:** M10 Alpine Linux — Phase 6 (Python3 Compatibility)

## Summary

A one-line position-tracking bug in `getdents64` caused directory listings
to silently drop entries at buffer boundaries. This broke Python3's import
system — the `collections` stdlib module was invisible to `os.listdir()`
even though `stat()` and `open()` found it fine. The fix: peek before
advancing the directory position.

## The symptom

After installing Python3 via `apk add python3` on Kevlar:

```python
>>> import json
ModuleNotFoundError: No module named 'collections'
```

But `collections` existed on disk:
```python
>>> os.path.isdir('/usr/lib/python3.12/collections')
True
>>> os.path.getsize('/usr/lib/python3.12/collections/__init__.py')
52378
```

Yet it was invisible to directory listing:
```python
>>> 'collections' in os.listdir('/usr/lib/python3.12')
False
```

The directory had 200 entries across 3 blocks (12KB), but `os.listdir()`
only returned 197. The 3 missing entries included `collections`.

## The investigation

### Red herring: ext4 multi-block directory parsing

Initial hypothesis: the ext4 `read_dir_entries()` function wasn't reading
beyond the first 4KB block. Testing showed this was wrong — the raw
directory data was 12KB with valid entries in all 3 blocks. Block 2's
first 8 bytes were non-zero (verified by kernel logging).

### Red herring: ext4 rec_len chain

Second hypothesis: the `rec_len` chain in block 1 didn't continue to block
2. Also wrong — the chain correctly summed to 4096 per block, and entries
at offset 4096 were valid.

### The real bug: getdents64 position tracking

The raw `syscall(SYS_getdents64, fd, buf, 65536)` with a 64KB buffer
returned all 200 entries correctly. But musl libc's `opendir`/`readdir`
uses a smaller buffer (~4KB), calling `getdents64` multiple times.

The kernel's `sys_getdents64` loop:
```rust
while let Some(entry) = dir.readdir()? {  // ← advances position
    if writer.pos() + reclen > len {
        break;  // ← entry already consumed, LOST
    }
    // write entry to userspace buffer
}
```

`dir.readdir()` called `self.pos.fetch_add(1)` unconditionally — even for
the entry that triggered the buffer-full `break`. When musl called
`getdents64` again, the kernel started from `pos+1`, skipping the entry
that didn't fit.

With ~150 entries per 4KB buffer, this lost 1 entry per call. Over 2-3
calls to enumerate 200 entries, 3 entries were lost.

## The fix

Split `readdir()` into `readdir_peek()` + `readdir_advance()`:

```rust
// Before: position advanced before buffer check
fn readdir(&self) -> Result<Option<DirEntry>> {
    let entry = self.as_dir()?.readdir(self.pos())?;
    self.pos.fetch_add(1);  // ← always advances
    Ok(entry)
}

// After: peek without advancing, advance only on successful write
loop {
    let entry = dir.readdir_peek()?;  // ← no position change
    if entry fits in buffer {
        dir.readdir_advance();        // ← advance only if written
        write entry to buffer;
    } else {
        break;  // entry preserved for next call
    }
}
```

Also fixed: `readdir()` no longer advances `pos` when returning `None`
(end of directory), preventing spurious position drift.

## Impact

| Test | Before | After |
|------|--------|-------|
| tmpfs 300-entry dir | 298 entries | **302** (300 + `.` + `..`) |
| ext4 /usr/lib/python3.12 | 197 entries | **200** (all entries) |
| Python3 `import collections` | ModuleNotFoundError | **PASS** |
| Python3 `import json` | ModuleNotFoundError | **PASS** |
| Python3 `import signal` | ModuleNotFoundError | **PASS** |
| Python3 test suite | 7/10 | **10/11** |

The only remaining Python3 failure is `subprocess` (fork+exec+pipe within
Python), which is a separate issue from directory enumeration.

## Lessons

1. **Small buffer sizes expose bugs that large buffers hide.** The raw
   `getdents64` syscall with a 64KB buffer worked perfectly. The bug only
   manifested when musl used a 4KB buffer, requiring multiple calls.

2. **Position-before-check is a classic off-by-one pattern.** The Linux
   kernel's `iterate_dir()` uses a callback that writes entries one at a
   time, returning an error if the buffer is full. The position is only
   advanced on successful write. Our implementation pre-advanced.

3. **`stat()` working but `listdir()` failing is a strong signal** for a
   readdir bug. The VFS lookup path (used by stat/open) traverses directory
   blocks independently, while readdir depends on sequential enumeration.

## Files changed

- `kernel/syscalls/getdents64.rs` — peek/advance pattern for buffer overflow
- `kernel/fs/opened_file.rs` — `readdir_peek()`, `readdir_advance()`, fix `readdir()` None case
- `testing/test_python3.c` — clean Python3 test suite (11 tests)
- `testing/test_readdir_debug.c` — directory enumeration diagnostic tool
