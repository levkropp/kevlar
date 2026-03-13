# M6.5 Phase 5: Subsystem Contracts

Phase 5 validates kernel subsystem interfaces: device nodes and
/proc filesystem.

---

## /dev/zero implementation

Kevlar's devfs had `/dev/null` but was missing `/dev/zero`.  Added
`kernel/fs/devfs/zero.rs` — a simple character device that returns
infinite zeros on read and absorbs all writes.  The implementation
uses `UserBufWriter::write_with()` to fill the user buffer with
`slice.fill(0)`.

## Tests

**dev_null_zero** — Validates `/dev/null` (write succeeds, read returns
EOF) and `/dev/zero` (read returns all zeros).

**proc_self** — Validates `/proc/self/exe` (readlink returns executable
path) and `/proc/self/stat` (contains `pid (comm) state` in the expected
Linux format with a valid state character).

## Known gaps

- `/proc/cpuinfo` format validation: not tested yet (needed for M7)
- `/proc/[pid]/maps` format: not tested yet
- `/sys` hierarchy: not implemented
- DRM devices: not implemented (M10 scope)
- `/dev/urandom`: not implemented (getrandom syscall works instead)

## Results

Full suite: **17/18 PASS** (only sa_restart TIMEOUT remains).
