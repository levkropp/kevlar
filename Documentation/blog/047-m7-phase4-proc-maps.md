# M7 Phase 4: /proc/[pid]/maps

Phase 4 enriches the existing /proc/[pid]/maps implementation with a
synthetic vDSO entry and adds a contract test verifying format
correctness.

## What already existed

The maps file was implemented during M5 and already iterated VMAs with
correct start-end addresses, rwxp permissions, and file offsets.
Anonymous VMAs at index 0 and 1 were labeled `[stack]` and `[heap]`
respectively, matching the internal Vm layout where `vm_areas[0]` is
always the stack and `vm_areas[1]` is always the heap.

## vDSO synthetic entry

The vDSO is mapped directly into the page table at
`VDSO_VADDR = 0x1000_0000_0000` during `setup_userspace()` without
creating a VMA.  This means it was invisible in /proc/[pid]/maps.

The fix adds a synthetic entry after the VMA loop:

```
100000000000-100000001000 r-xp 00000000 00:00 0          [vdso]
```

This is gated behind `#[cfg(target_arch = "x86_64")]` since ARM64
doesn't currently have a vDSO.  Tools like `ldd`, glibc's dynamic
linker, and GDB look for `[vdso]` when resolving clock_gettime.

## Contract test

The new `proc_maps.c` test:

- mmaps an anonymous page, then reads /proc/self/maps
- Verifies `[stack]` annotation exists
- Verifies `[heap]` annotation exists
- Validates the `XXXXXXXX-XXXXXXXX rwxp` line format
- Confirms the mmap'd address appears in the output

## Results

23/23 contract tests pass (6/6 subsystem tests including the new
proc_maps).

## What's next

Phase 5 handles /proc/[pid]/fd/ directory and symlinks — the interface
that `ls -l /proc/self/fd/` and `lsof` use to enumerate open file
descriptors.
