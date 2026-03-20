# VFS Path Resolution Overhaul — tar_extract 1.12x → 1.09x

**Date:** 2026-03-20
**Benchmark impact:** tar_extract 1.12x→1.09x, open_close 0.83x→0.75x, file_tree 0.62x→0.54x

## Problem

`tar_extract` was the only benchmark showing a REGRESSION (1.12x vs Linux).
Profiling pointed to VFS path resolution: every `open(O_CREAT)`, `unlink`,
`mkdir`, and `symlink` call built a full `Arc<PathComponent>` chain with
heap String allocations for every path component — even when only the parent
directory inode was needed.

## Three optimizations

### 1. Fast parent-inode lookup (`lookup_parent_inode_at`)

Syscalls like `unlinkat`, `mkdirat`, `symlinkat`, `linkat`, and `renameat`
only need the parent directory's inode to perform their operation. Previously
they called `lookup_parent_path_at()` which built the FULL PathComponent
chain (N `Arc::new` + N `String::to_owned`) just to extract the parent
inode and discard the chain.

New method `lookup_parent_inode_at()` resolves the parent using the fast
`lookup_inode()` path — zero Arc/String allocations, zero PathComponent
chain construction.

Also added `lookup_parent_inode()` (no `_at`) for absolute/CWD-relative
paths that doesn't require the opened files table lock at all.

### 2. Flat PathComponent for open/openat

Instead of building an N-level `Arc<PathComponent>` chain with parent
pointers and per-component String names, we now build a single "flat"
PathComponent:

```rust
PathComponent {
    parent_dir: None,          // No chain
    name: "/full/absolute/path", // Full path in one String
    inode: resolved_inode,
}
```

`resolve_absolute_path()` was updated to recognize flat paths (name starts
with '/') and return them directly — no parent chain walk needed.

To make this work for relative paths, RootFs now caches the cwd's absolute
path as a String (`cwd_abs`), updated on `chdir`/`chroot`. Building the
flat path for a relative open is just `String::with_capacity` + two
`push_str` calls.

### 3. O_CREAT skip-re-resolution

The old `openat(O_CREAT)` flow resolved the path TWICE:
1. `create_file_at()`: resolve parent → create file → drop everything
2. `lookup_path()`: resolve FULL path again → build PathComponent for fd table

Now both happen under a single `root_fs` lock:
1. `lookup_parent_inode()`: resolve parent (fast, no chain)
2. `create_file()`: get the new inode
3. `make_flat_path_component()`: build flat PathComponent from the inode directly

For the EEXIST case (file already exists), we fall back to `lookup_inode` +
flat path. Either way, we never build the intermediate PathComponent chain.

## What didn't work: dentry cache

We tried a global `HashMap<(dir_ptr, name_hash), INode>` cache checked before
every `dir.lookup()`. For tar_extract's create-delete-per-iteration pattern,
the SpinLock + HashMap overhead on every component lookup exceeded the cache
hit savings. Removed.

## Results

| Benchmark  | Before | After | Change |
|------------|--------|-------|--------|
| tar_extract | 1.12x | 1.09x | REGRESSION → marginal |
| open_close  | 0.83x | 0.75x | faster |
| file_tree   | 0.62x | 0.54x | faster |

All 116/118 contract tests pass. No new regressions.

## Files changed

- `kernel/fs/mount.rs` — `lookup_parent_inode[_at]`, `make_flat_path_component`, `cwd_abs` cache
- `kernel/fs/opened_file.rs` — flat path support in `resolve_absolute_path`
- `kernel/syscalls/openat.rs` — combined O_CREAT + flat PathComponent
- `kernel/syscalls/open.rs` — same optimization
- `kernel/syscalls/unlinkat.rs`, `mkdirat.rs`, `symlinkat.rs`, `linkat.rs`, `renameat.rs` — fast parent lookup
