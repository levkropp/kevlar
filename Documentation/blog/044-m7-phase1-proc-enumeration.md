# M7 Phase 1: /proc Root Directory PID Enumeration

The /proc filesystem has existed in Kevlar since M5, but it had a blind
spot: `readdir("/proc/")` only returned the 10 static files (cpuinfo,
meminfo, mounts, etc.).  It never enumerated live PIDs or showed the
`self` symlink.  Any program that iterates /proc to discover processes —
`ps`, `top`, `htop`, systemd's process tracker — would see an empty
process list.

Phase 1 closes this gap.  `ls /proc` now shows `self`, every live PID
directory, and all static files.

## The gap

ProcRootDir already handled *lookups* correctly: `open("/proc/42/stat")`
worked because `lookup()` parsed numeric names and constructed
ProcPidDir on the fly.  But `readdir()` — the function behind
`getdents64(2)` — only delegated to the underlying tmpfs, which knew
about the 10 static files and nothing else.

The fix has two parts: a way to enumerate PIDs, and a readdir that
stitches static entries, `self`, and PIDs together.

## list_pids()

The process table is a `SpinLock<BTreeMap<PId, Arc<Process>>>`.  We
already had `process_count()` that locks and returns `.len()`.  The new
`list_pids()` follows the same pattern:

```rust
pub fn list_pids() -> Vec<PId> {
    PROCESSES.lock().keys().cloned().collect()
}
```

BTreeMap iteration yields keys in sorted order, so the PID list comes
out naturally sorted — `ls /proc` shows `1 2 3 ...` without extra work.

## Stitched readdir

The readdir protocol is index-based: the VFS calls `readdir(0)`,
`readdir(1)`, `readdir(2)`, etc., until it gets `None`.  Our readdir
partitions the index space into three regions:

1. **Static entries** (indices 0..N): delegated to the tmpfs directory
   (metrics, mounts, cpuinfo, meminfo, stat, version, etc.)
2. **"self" symlink** (index N): a DirEntry with `FileType::Link`
3. **PID directories** (indices N+1..): one DirEntry per live process
   with `FileType::Directory` and the PID as the name

When the static directory exhausts its entries, we count how many it had
and use the remainder as an offset into the dynamic entries.

## Contract test

The new `proc_mount.c` contract test verifies four things:

- `readdir("/proc/")` contains a `self` entry
- `readdir("/proc/")` contains at least one numeric PID entry
- `readlink("/proc/self")` resolves to the current process's PID
- `/proc/1/stat` is readable

This runs on both Linux and Kevlar through the contract comparison
framework.  Both produce identical output: `proc_readdir_self: ok`,
`proc_readdir_pid: ok`, `proc_self_readlink: ok`, `proc_1_stat: ok`,
`CONTRACT_PASS`.

## Results

20/20 contract tests pass, including the new `proc_mount` test.  No
regressions in existing tests.

## What's next

Phase 2 enriches the global /proc files.  Most of these already exist
(/proc/cpuinfo, /proc/version, /proc/meminfo, /proc/mounts) but need
verification against glibc's expectations and multi-CPU accuracy for
/proc/cpuinfo.  The goal is that every file `ls /proc` shows is actually
readable with correct content.
