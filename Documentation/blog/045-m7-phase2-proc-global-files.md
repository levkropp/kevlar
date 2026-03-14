# M7 Phase 2: Global /proc File Validation

Phase 2 validates the 10 global /proc files that were implemented during
M5 and enriches /proc/cpuinfo with CPUID-derived fields that userspace
tools expect.

## What already existed

All 10 system-wide /proc files were implemented during M5 Phase 4:
cpuinfo, version, meminfo, mounts, stat, uptime, loadavg, filesystems,
cmdline, and metrics.  They serve live data from the page allocator,
process table, mount table, and TSC clock.  Phase 2's job was to verify
format correctness and fill gaps.

## cpuinfo enrichment

The existing /proc/cpuinfo had processor, vendor_id, model name, MHz,
cache size, flags, and bogomips — but was missing `cpu family`, `model`,
and `stepping`.  These three fields are parsed by `lscpu`, Python's
`platform.processor()`, and glibc's CPU feature detection.

The fix adds a `cpuid_family_model_stepping()` function to the platform
crate that reads CPUID leaf 1 via the `raw-cpuid` crate:

```rust
pub fn cpuid_family_model_stepping() -> (u32, u32, u32) {
    let info = CpuId::new().get_feature_info().unwrap();
    (info.family_id() as u32, info.model_id() as u32, info.stepping_id() as u32)
}
```

The `raw-cpuid` crate handles the Intel/AMD extended family/model
encoding automatically — `family_id()` combines base and extended
family for families >= 15, and `model_id()` combines base and extended
model for families 6 and 15.

## Contract test

The new `proc_global.c` contract test verifies all six key global files:

- `/proc/cpuinfo` — contains `processor` field
- `/proc/version` — contains kernel name substring
- `/proc/meminfo` — `MemTotal:` with value > 0
- `/proc/mounts` — at least one mount entry
- `/proc/uptime` — two parseable floats > 0
- `/proc/loadavg` — five parseable fields (three averages + running/total)

## Results

21/21 contract tests pass, including the new `proc_global` test.

## What's next

Phase 3 enriches the per-process /proc files.  The existing
/proc/[pid]/stat outputs 52 fields but many are hardcoded zeros.  Phase
3 adds real values for utime/stime (CPU accounting), num_threads,
starttime, vsize, and rss — the fields that `ps`, `top`, and `htop`
actually parse.
