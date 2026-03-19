# Blog 088: Heap VMA index corruption — the apk infinite fault loop

**Date:** 2026-03-19
**Milestone:** M10 Alpine Linux

## The bug

After fixing three bugs in blog 087 (lseek on directories, debug= cmdline
concatenation, and CLOCK_REALTIME wall-clock), we re-ran `apk update` expecting
it to progress past the userspace spin loop. It did — apk now exited with code 1
instead of hanging forever — but ktrace still showed PID 6 stuck for 30 seconds
with no syscalls after its last `mmap` call. The wall-clock fix helped (apk no
longer spun *forever*), but something else was keeping it from reaching the
network phase.

## Adding PAGE_FAULT events to ktrace

ktrace only traced syscalls, context switches, wait queues, and network events.
Page faults were invisible. We added a `PAGE_FAULT` event type to ktrace (gated
by `ktrace-mm`), recording the faulting address, RIP, and x86 error code bits.

The result was dramatic: **45.8 million events** in 30 seconds, with the ring
buffer completely saturated by page faults. Every single one was identical:

```
addr=0x420000  rip=0x420000  reason=PRESENT|USER|INST_FETCH
```

This is a **NX fault loop**: the CPU tries to execute code at 0x420000, the page
*is* present (PRESENT=1), but the No-Execute bit is set. The page fault handler
"fixes" the flags and returns, but NX persists on the next access. ~1.5 million
faults per second, burning 100% CPU.

## Why was NX set on a code page?

Address 0x420000 falls squarely in apk.static's `.text` segment (LOAD 1:
0x401000–0x73F6D3, flags R+E). The VMA should have `PROT_READ|PROT_EXEC` (5),
and the page fault handler correctly clears NX when PROT_EXEC is present.

We added a diagnostic that dumped the VMA's `prot_flags` during the fault:

```
prot_flags=1
```

Just `PROT_READ`. No execute permission. But the ELF loader's `elf_flags_to_prot`
correctly converts `PF_R|PF_X` → `PROT_READ|PROT_EXEC`. Where was PROT_EXEC
getting lost?

## The VMA dump reveals overlapping VMAs

We added a VMA dump to the diagnostic:

```
VMA[1]: [0x400000-0x89328c) prot=1 file off=0x0 fsz=0x28c  ← WRONG
VMA[2]: [0x401000-0x73f6d3) prot=5 file off=0x1000 fsz=0x33e6d3  ← correct
```

VMA[1] is a *giant* file-backed VMA spanning nearly 5 MB, with just `PROT_READ`.
It completely overlaps VMA[2] (the actual code segment). Since `find_vma_cached`
does a linear search and VMA[1] comes first, every page fault in the code range
gets `prot=1` → NX set.

But VMA[1] should be the **heap VMA** (anonymous, start=0x890000, len=0). How did
it become a file-backed VMA at 0x400000?

## Root cause: mmap(MAP_FIXED) destroys heap VMA index

The smoking gun was musl's malloc initialization sequence:

```
brk(0)        → 0x890000       # query current break
brk(0x892000) → 0x892000       # extend heap by 8KB
mmap(0x890000, 0x1000, MAP_FIXED) → 0x890000   # remap first heap page
```

musl uses `brk()` to extend the heap, then `mmap(MAP_FIXED)` to remap specific
pages within it. This is valid on Linux where the brk area is tracked by
`mm_struct->brk` and `mm_struct->start_brk`, independent of VMA indices.

In Kevlar, the heap was tracked by **hardcoded index**: `heap_vma_mut()` returned
`&mut vm_areas[1]`. When `mmap(MAP_FIXED)` at 0x890000 called `remove_vma_range`,
the heap VMA was removed from index 1. The `Vec::remove()` shifted all subsequent
elements down: the ELF LOAD 0 segment (prot=R, starting at 0x400000) moved to
index 1.

Later, `brk(0x893000)` called `expand_heap_to`, which accessed `vm_areas[1]` —
now LOAD 0 instead of the heap. It extended LOAD 0's length:

```
new_len = 0x28C + align_up(0x893000 - 0x40028C) = 0x49328C
```

This created a 5 MB read-only file-backed VMA overlapping the entire ELF image,
including the code segment. The code segment VMA was still present at index 2, but
the linear VMA search found the bloated LOAD 0 first.

## The fix

Replaced index-based heap tracking with explicit fields in the `Vm` struct:

```rust
pub struct Vm {
    // ... existing fields ...
    heap_bottom: UserVAddr,
    heap_end: UserVAddr,
}
```

`expand_heap_to()` now creates new anonymous VMAs for expanded heap regions
instead of mutating a VMA at a fixed index. The `heap_bottom`/`heap_end` fields
are the source of truth for `brk()`, immune to VMA reordering by munmap/mmap.

## After the fix: apk reaches the network

With the heap fix, apk progresses through database parsing and reaches the
network phase:

```
fetch http://dl-cdn.alpinelinux.org/alpine/v3.21/main/x86_64/APKINDEX.tar.gz
DHCP: got a IPv4 address: 10.0.2.15/24
```

ktrace shows healthy activity: 482 syscalls, 579 page faults (normal demand
paging), 10 network events. apk creates a UDP socket, sends DNS queries, and
enters `poll()` waiting for the response.

The next blocker is DNS resolution: the response packet arrives (RX 64 bytes) but
`poll()` never detects data on the UDP socket — a smoltcp/socket wake integration
issue to investigate next.

## Bug #5: UDP source IP 0.0.0.0

After the heap fix, apk reached DNS resolution but `poll()` blocked forever.
ktrace showed the DNS response arriving but the UDP socket never reported data
ready.

Packet logging revealed the root cause: the DNS query went out with **source IP
0.0.0.0** despite DHCP having configured 10.0.2.15. smoltcp uses the socket's
bound address as the source — and the socket was bound to `0.0.0.0:50000`
(INADDR_ANY). The DNS response came back addressed to 0.0.0.0, but smoltcp's
interface filter (`has_ip_addr`) rejected it since the interface IP is now
10.0.2.15.

**Fix:** In `UdpSocket::sendto()`, rebind the socket from 0.0.0.0 to the
interface's actual IP before sending. Same fix in `TcpSocket::connect()` for the
local endpoint.

## Bug #6: recvmsg on UDP returns EBADF

After DNS worked, apk entered a tight `poll()` + `recvmsg()` busyloop. The
`recvmsg` handler called `file.read()`, but `UdpSocket` doesn't implement
`read()` — only `recvfrom()`. The default `FileLike::read()` returns EBADF.

**Fix:** Changed `recvmsg` handler to call `file.recvfrom()` instead of
`file.read()`, since `recvfrom` is implemented by all socket types.

## Current state

With all 6 bugs fixed, apk successfully:
1. Parses the local package database (15 installed packages)
2. Resolves `dl-cdn.alpinelinux.org` via DNS
3. Attempts TCP connection to the CDN

The next blocker is the TCP/HTTP fetch — apk exits with code 1 without an error
message. Investigation of the TCP connection is needed.

## Bugs fixed this session (cumulative with blog 087)

| # | Bug | Symptom | Root cause |
|---|-----|---------|------------|
| 1 | lseek on directories | ESPIPE instead of 0 | `Directory(_) => false` in seekable check |
| 2 | debug= cmdline concat | ktrace filter not activated | Missing comma separator between args |
| 3 | CLOCK_REALTIME | Near-zero timestamps | vDSO only handled MONOTONIC |
| 4 | **Heap VMA corruption** | **Infinite NX page fault loop** | **Hardcoded `vm_areas[1]` for heap** |
| 5 | **UDP source IP 0.0.0.0** | **DNS response dropped** | **smoltcp uses socket bind addr as source** |
| 6 | **recvmsg on UDP** | **EBADF busyloop** | **recvmsg called file.read(), not recvfrom** |

## Test results

- BusyBox: 100/100 PASS
- Contract tests: 103 PASS, 9 XFAIL, 0 FAIL
- SMP threads: 14/14 PASS
