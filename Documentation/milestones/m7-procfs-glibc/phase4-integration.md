# Phase 4: /proc/[pid]/maps

**Duration:** ~1 day
**Prerequisite:** Phase 1
**Goal:** Implement /proc/[pid]/maps — the memory map file.

## Why a separate phase

`/proc/[pid]/maps` is the most complex /proc file.  It requires:
- Iterating all VMAs with the VM lock held
- Formatting addresses, permissions, offsets, device numbers, inodes
- Resolving VMA-to-file paths (for file-backed mappings)
- Matching Linux's exact format (debuggers, glibc malloc, strace parse it)

## Format

```
00400000-00408000 r-xp 00000000 00:00 12345    /bin/hello
00607000-00608000 rw-p 00007000 00:00 12345    /bin/hello
00608000-00629000 rw-p 00000000 00:00 0        [heap]
7f8000000000-7f8000001000 rw-p 00000000 00:00 0
7ffffffde000-7ffffffff000 rw-p 00000000 00:00 0 [stack]
```

Fields (hyphen/space delimited):
1. `start-end` — hex addresses, no leading zeros (lowercase)
2. `perms` — 4 chars: r/-, w/-, x/-, p(private)/s(shared)
3. `offset` — hex file offset for file-backed mappings, 0 for anon
4. `dev` — `major:minor` of device (00:00 for anonymous/tmpfs)
5. `inode` — inode number (0 for anonymous)
6. `pathname` — file path, `[heap]`, `[stack]`, `[vdso]`, or blank

## Implementation

```rust
fn read_maps(pid: PId, buf: &mut UserBufWriter) -> Result<usize> {
    let process = find_process(pid)?;
    let vm_ref = process.vm();
    let vm = vm_ref.as_ref().unwrap().lock_no_irq();

    for vma in vm.vm_areas() {
        let start = vma.start().value();
        let end = vma.end().value();
        let prot = vma.prot();
        let r = if prot.contains(MMapProt::PROT_READ) { 'r' } else { '-' };
        let w = if prot.contains(MMapProt::PROT_WRITE) { 'w' } else { '-' };
        let x = if prot.contains(MMapProt::PROT_EXEC) { 'x' } else { '-' };
        let p = 'p'; // always private for now (MAP_SHARED not tracked)

        // Format: start-end perms offset dev inode pathname
        write!(buf, "{:08x}-{:08x} {}{}{}{} 00000000 00:00 0",
               start, end, r, w, x, p)?;

        // Pathname annotation
        match vma.area_type() {
            VmAreaType::File { .. } => { /* write file path */ }
            VmAreaType::Anonymous => {
                if is_heap(vma) { write!(buf, " [heap]")?; }
                else if is_stack(vma) { write!(buf, " [stack]")?; }
            }
        }
        write!(buf, "\n")?;
    }
    Ok(buf.written())
}
```

## Key decisions

- **heap detection:** VMA at index 1 (heap_vma) → annotate as `[heap]`
- **stack detection:** VMA at index 0 (stack_vma) → annotate as `[stack]`
- **vDSO:** VMA containing VDSO_VADDR → annotate as `[vdso]`
- **file-backed:** use inode number and file path from VmAreaType::File
- **MAP_SHARED:** currently not tracked in VMAs, always show 'p'

## Vm struct additions

Need `pub fn vm_areas(&self) -> &[VmArea]` accessor on Vm.

## Testing

Contract test: `testing/contracts/subsystems/proc_maps.c`
```c
// mmap anonymous page, read /proc/self/maps
// Verify the mapped address appears with correct permissions
// Verify [stack] and [heap] annotations exist
```

## Success criteria

- [ ] `cat /proc/self/maps` shows all VMAs with correct format
- [ ] Permissions match actual mmap/mprotect settings
- [ ] [heap] and [stack] annotations present
- [ ] glibc malloc can read /proc/self/maps without crashing
