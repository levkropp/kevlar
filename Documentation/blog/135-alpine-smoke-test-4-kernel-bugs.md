# Blog 135: Alpine smoke test suite — 67 tests, 4 kernel bugs found and fixed

**Date:** 2026-03-31
**Milestone:** M10 Alpine Linux — Drop-in Validation

## Summary

Built a comprehensive 67-test smoke test suite that validates Kevlar as a
drop-in Linux replacement across 8 categories: boot, filesystem, shell
utilities, process management, system info, networking, package management,
and stress testing. The suite runs in 23 seconds under KVM and uncovered
4 kernel bugs — all fixed.

Final result: **67/67 PASS, 159/159 contract tests still green.**

## The smoke test

The existing Alpine test coverage was layered (7-layer integration test) but
narrow — it tested "can apk update work?" but not "does the system behave
like Linux?" The smoke test (`testing/smoke_alpine.c`) boots Alpine 3.21
on an ext2 disk and exercises real-world operations end-to-end:

| Phase | Tests | What it covers |
|-------|-------|----------------|
| P1: Boot | 5 | ext2 mount, Alpine filesystem layout, musl loader |
| P2: Filesystem | 9 | write/read, symlinks, hardlinks, chmod, 1MB file, deep dirs, readdir 20 files |
| P3: Shell | 20 | pipes, redirects, loops, grep/sed/awk/sort/find/tar/xargs/cut/tr, heredocs |
| P4: Processes | 9 | fork, signals (TERM/KILL/INT/TSTP/CONT), process groups, background jobs |
| P5: System info | 10 | uname, /proc/{version,meminfo,cpuinfo,self/maps,self/status,uptime,mounts,self/fd} |
| P6: Networking | 4 | DNS resolution, HTTP GET, unix socketpair, TCP loopback via 127.0.0.1 |
| P7: Packages | 4 | apk update, apk add less, run installed binary, apk info |
| P8: Stress | 6 | rapid fork x50, 5 concurrent pipe chains, 100 files create/unlink, 1MB pipe, signal storm x100, dd 4MB |

Run it with `make test-smoke-alpine`. Uses KVM with the Alpine ext2 disk
image (built by `make alpine-disk`).

## Bug 1: SIGKILL ignored when signals are blocked

**Symptom:** The smoke test hung at `p4_sigkill`. A child process blocked all
signals with `sigfillset` + `sigprocmask(SIG_BLOCK)` then called `pause()`.
The parent sent `SIGKILL` — the child never died.

**Root cause:** `has_pending_signals()` checked `pending & !blocked` but
didn't unmask SIGKILL/SIGSTOP from the blocked set. POSIX requires these
two signals to be unblockable. The delivery path (`pop_pending_unblocked`)
correctly unmasked them, but the sleep wakeup path (`has_pending_signals`)
didn't — so `sleep_signalable_until` never returned EINTR.

**Fix:** One line in `process.rs`:

```rust
pub fn has_pending_signals(&self) -> bool {
    let pending = self.signal_pending.load(Ordering::Relaxed);
    let mut blocked = self.sigset_load().bits() as u32;
    // SIGKILL (9) and SIGSTOP (19) can NEVER be blocked (POSIX).
    blocked &= !((1 << (SIGKILL - 1)) | (1 << (SIGSTOP - 1)));
    (pending & !blocked) != 0
}
```

This is a critical correctness fix. Without it, any process that blocked
all signals became unkillable — not even SIGKILL could stop it.

## Bug 2: setpgid(0, 0) created process group 0

**Symptom:** `p4_pgid_kill` failed. The test forked a child that called
`setpgid(0, 0)` to become its own process group leader, then the parent
sent `kill(-child_pid, SIGKILL)`. The kill returned ESRCH — no such
process group.

**Root cause:** POSIX says `setpgid(0, 0)` means "set my process group ID
to my own PID." Kevlar passed the raw `pgid=0` to `find_or_create_by_pgid`,
which created a process group with pgid=0 instead of pgid=caller's PID.

**Fix:** In `setpgid.rs`, resolve pgid=0 to the target's PID before creating
the group:

```rust
let effective_pgid = if pgid.as_i32() == 0 {
    PgId::new(target.pid().as_i32())
} else {
    pgid
};
```

## Bug 3: ProcessGroup::signal() panicked on dead processes

**Symptom:** Race condition — not triggered by the smoke test, but identified
during code review. `ProcessGroup::signal()` called `.unwrap()` on
`Weak<Process>::upgrade()`. If any process in the group had exited but
hadn't been cleaned from the group's member list, this panicked.

**Root cause:** Process group members are stored as `Weak<Process>` refs.
When a process exits, its `Arc<Process>` may be dropped before the weak ref
is removed from the group. A concurrent signal delivery (e.g., SIGINT from
Ctrl+C) could hit the dropped ref.

**Fix:** Replace `unwrap()` with `retain()` that filters dead refs:

```rust
pub fn signal(&mut self, signal: Signal) {
    self.processes.retain(|proc| {
        if let Some(p) = proc.upgrade() {
            p.send_signal(signal);
            true
        } else {
            false
        }
    });
}
```

## Bug 4: Kernel GPF during shutdown

**Symptom:** After the smoke test completed and PID 1 exited, a General
Protection Fault (vec=13) occurred in `UserCStr::new` → `sys_execve`.

**Root cause:** When PID 1 exits, it calls `halt()` which writes to QEMU's
debug exit port. But there's a tiny window between the port write and the
VM actually stopping — orphaned child processes (left over from fork tests)
could still be scheduled and try to access freed page tables.

**Fix:** Send SIGKILL to all remaining processes before halting:

```rust
if current.pid == PId::new(1) {
    // ... debug dumps ...
    let all_pids: Vec<PId> = PROCESSES.lock().keys().cloned().collect();
    for pid in all_pids {
        if pid != PId::new(1) {
            if let Some(proc) = PROCESSES.lock().get(&pid).cloned() {
                proc.send_signal(SIGKILL);
            }
        }
    }
    kevlar_platform::arch::halt();
}
```

## Other improvements

**TCP ephemeral port allocation:** `bind()` with port 0 now assigns a port
from the dynamic range (49152-65535), matching the UDP socket behavior.
Previously, port 0 was stored literally, causing `listen()` to fail when
smoltcp tried to listen on port 0.

**TCP loopback verified:** Loopback routing (127.0.0.1) was already
implemented in the TX path — `OurTxToken::consume()` detects packets for
127.0.0.0/8 or the interface's own IP and reinjects them into the RX queue.
Added a proper TCP loopback test (server + client via 127.0.0.1) to the
smoke suite to validate this.

## What the smoke test proves

With 67 tests passing across 8 phases, Kevlar demonstrates:

- **POSIX process semantics:** fork, exec, wait, signals (including
  SIGKILL/SIGSTOP unblockability), process groups, job control
- **Shell compatibility:** BusyBox sh runs pipes, redirects, loops,
  command substitution, and 15+ standard Unix utilities correctly
- **Filesystem correctness:** ext2 read-write with symlinks, hardlinks,
  permissions, large files, deep directories, readdir consistency
- **Network stack:** DNS, TCP (both external HTTP and local loopback),
  Unix sockets all work
- **Package management:** `apk update` + `apk add` installs and runs
  real Alpine packages
- **Stress resilience:** rapid fork/exit, concurrent pipe chains, 100-file
  create/unlink, 1MB pipe throughput, signal storms

This is the validation gate for moving to graphical mode — the text-mode
Alpine experience is now systematically verified as Linux-equivalent.
