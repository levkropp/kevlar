# M8 Phase 2: Namespaces — UTS, PID, and Mount

Phase 2 adds Linux namespace support with UTS (hostname isolation),
PID (process ID isolation), and mount namespace infrastructure.

## Architecture

A new `kernel/namespace/` module provides:

- **NamespaceSet** — per-process bundle of Arc pointers to UTS, PID,
  and mount namespace objects.  Processes sharing the same Arc see the
  same namespace.
- **UtsNamespace** — hostname and domainname with SpinLock-protected
  buffers.  `sethostname()` writes to the calling process's UTS
  namespace.  `uname()` reads from it.
- **PidNamespace** — local/global PID translation maps.  Non-root
  namespaces allocate sequential PIDs starting at 1.  `getpid()`
  returns the namespace-local PID.
- **MountNamespace** — placeholder for Phase 3 (pivot_root).

## Syscalls added

| Syscall | Behavior |
|---------|----------|
| unshare(2) | Create new namespace(s) for calling process |
| sethostname(2) | Set hostname in UTS namespace |
| setdomainname(2) | Set domainname in UTS namespace |

## clone(2) namespace flags

clone() now handles CLONE_NEWUTS, CLONE_NEWPID, CLONE_NEWNS.
CLONE_NEWNET returns EINVAL (not implemented). When CLONE_NEWPID is
set, the child gets a namespace-local PID (typically 1) and getpid()
returns it.

## uname(2) enrichment

`uname()` now returns:
- hostname and domainname from the calling process's UTS namespace
- machine field (`x86_64` or `aarch64`)

Previously these were empty/zeroed.

## Results

- 27/28 PASS, 1 XFAIL (ns_uts: unshare needs CAP_SYS_ADMIN on Linux)
- 14/14 musl threading tests pass (no regression)
