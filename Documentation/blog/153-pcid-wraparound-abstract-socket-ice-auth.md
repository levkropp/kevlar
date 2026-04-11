# Blog 153: PCID Wraparound Fix, Abstract Socket getsockname, ICE Auth Resolved

**Date:** 2026-04-11

## PCID Wraparound: Root Cause of SMP Page Corruption

The intermittent SIGSEGV in XFCE processes (Blog 152) was caused by
**PCID wraparound**. Each address space got a sequential 12-bit PCID
(Process Context IDentifier). Context switches loaded CR3 with bit 63=1
(no-invalidate), preserving TLB entries tagged with all PCIDs.

After 4095 process creations (XFCE startup creates many: dbus-daemon,
xfconfd, xfce4-session, xfwm4, xfsettingsd, etc.), the PCID counter
wrapped. A new process got the same PCID as a long-dead process. Stale
TLB entries from the dead process — still cached on remote CPUs — now
matched the new process's PCID. The CPU loaded code/data from the dead
process's physical pages instead of the new process's.

**Fix:** Disable PCID (alloc_pcid returns 0). Every CR3 write now
fully flushes the TLB. Safe but slower (~5-10% for context switches).
The proper fix is Linux-style PCID tracking with invpcid on reuse.

**Result:** xfwm4 now survives XFCE startup. The session manager
progresses to launch Thunar (Client3), confirming all prior clients
registered successfully.

## Abstract Unix Socket getsockname: ICE Authentication Fix

The ICE session manager authentication failure ("MIT-MAGIC-COOKIE-1
authentication rejected") was caused by a missing NUL byte in
`getsockname()` for abstract Unix sockets.

**Background:** Abstract sockets use `sun_path[0] = NUL` as a
namespace marker. Kevlar stored abstract paths internally without the
NUL prefix. `getsockname()` returned the path without NUL — making it
look like a filesystem path to userspace.

**Impact:** ICE uses `getsockname()` to construct the network ID for
auth cookie lookup. Without the NUL prefix, the network ID was
`unix/kevlar:/tmp/.ICE-unix/18` instead of `local/kevlar:@/tmp/...`.
The ID didn't match the `.ICEauthority` entries. xfwm4 found no
matching cookie and sent no credentials. The session manager rejected
the connection.

**Fix:** Prefix abstract paths with `@` in internal storage (bind,
connect). `getsockname()`/`getpeername()` detect `@` and write NUL
byte as `sun_path[0]`.

## Additional Fixes

- **SO_PEERCRED:** Returns actual peer pid/uid/gid from UnixStream
  (was returning caller's own credentials)
- **xfwm4 compositing:** Disabled via xfwm4.xml config (avoids
  RenderBadPicture crash when swrast_dri.so is missing)
- **BSP LAPIC timer:** Replaces PIT killed by Xorg's iopl(3) port writes
- **Timer starvation:** resume_boosted + need_resched for scheduler fairness
- **IF=1 forced on SYSRET/IRET:** Prevents permanent interrupt disable from iopl cli

## Test Results

| Suite | Result |
|-------|--------|
| Threading SMP (4 CPUs) | 14/14 PASS |
| XFCE Desktop (SMP 2) | 3/4 consistent |

XFCE 3/4: mount_rootfs + xfwm4 + xfce4_session pass.
Panel (4th test) needs all prior failsafe clients to start — blocked
by intermittent timer starvation from iopl(3) cli/sti.

## Files Changed

- `platform/x64/paging.rs`: Disable PCID (alloc_pcid returns 0)
- `kernel/net/unix_socket.rs`: Abstract socket `@` prefix, peer credentials
- `kernel/syscalls/getsockopt.rs`: SO_PEERCRED returns peer creds
- `kernel/syscalls/mod.rs`: iopl(3) implementation
- `platform/x64/usermode.S`: Force IF=1 on SYSRET
- `platform/x64/trap.S`: Force IF=1 on IRET
- `kernel/main.rs`: BSP LAPIC timer, AP processes full timer handler
- `testing/test_xfce.c`: Compositing disabled, ICE dir, clean auth, 15s wait
