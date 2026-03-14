# Phase 2: Global /proc Files

**Duration:** ~1 day
**Prerequisite:** Phase 1
**Goal:** Implement /proc/cpuinfo, /proc/version, /proc/meminfo, /proc/mounts.

## Scope

These are global (non-per-process) /proc files.  They're read-only,
world-readable, and relatively simple to implement.

## Files

### 1. /proc/cpuinfo

Format (must match Linux exactly — tools parse this):
```
processor	: 0
vendor_id	: GenuineIntel
cpu family	: 6
model		: 106
model name	: Intel(R) Xeon(R) ...
stepping	: 0
cpu MHz		: 2400.000
cache size	: 0 KB
physical id	: 0
siblings	: 1
core id		: 0
cpu cores	: 1
flags		: fpu de pse tsc msr pae mce cx8 ...
```

Fields separated by tab + colon + space.  One block per CPU, separated
by blank line.  Implement using `arch::cpuid()` on x86_64.

**Why critical:** glibc, `lscpu`, Python's `platform.processor()`, and
GPU drivers all parse /proc/cpuinfo.

### 2. /proc/version

Single line:
```
Kevlar version 0.1.0 (gcc) #1 SMP PREEMPT Fri Mar 14 2026
```

Simple static string.  Some tools (uname -a) read this.

### 3. /proc/meminfo

Format:
```
MemTotal:       1048576 kB
MemFree:         524288 kB
MemAvailable:    524288 kB
Buffers:              0 kB
Cached:               0 kB
SwapTotal:            0 kB
SwapFree:             0 kB
```

Read from `page_allocator::stats()` (free pages * 4).  MemTotal from
boot info.  Other fields can be 0 for now.

**Why critical:** `free`, `top`, `htop`, systemd's memory pressure
monitoring all read this.

### 4. /proc/mounts

Format:
```
rootfs / tmpfs rw 0 0
devfs /dev devfs rw 0 0
proc /proc proc rw 0 0
```

Iterate mounted filesystems.  Initially can be hardcoded; dynamic
mount tracking comes later.

## Implementation notes

- All files are read-only, world-readable (mode 0444)
- No allocations in read paths — format directly into UserBufWriter
- /proc/cpuinfo is the most complex: needs CPUID instruction results
- On ARM64, /proc/cpuinfo has a different format (no vendor_id)

## Testing

Contract test: `testing/contracts/subsystems/proc_global.c`
```c
// Read /proc/cpuinfo, verify "processor" field exists
// Read /proc/version, verify "Kevlar" appears
// Read /proc/meminfo, verify MemTotal > 0
// Read /proc/mounts, verify at least one entry
```

## Success criteria

- [ ] `cat /proc/cpuinfo` shows CPU info per online CPU
- [ ] `cat /proc/version` shows Kevlar version
- [ ] `cat /proc/meminfo` shows MemTotal and MemFree
- [ ] `cat /proc/mounts` shows mounted filesystems
