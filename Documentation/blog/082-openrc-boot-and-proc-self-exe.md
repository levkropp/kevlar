# 082: OpenRC Boot — /proc/self/exe Shebang Bug and Fork OOM Hardening

## Context

Running `make run` with BusyBox init + Alpine OpenRC produced an immediate
kernel panic: `failed to allocate kernel stack: PageAllocError` inside
`fork()`.  The flight recorder showed PIDs climbing past 5000 — a fork storm
was exhausting all physical memory before the kernel could even reach a login
prompt.

Three bugs conspired to produce the crash:

1. **`alloc_kernel_stack` panicked** instead of returning ENOMEM, so any OOM
   during fork killed the entire kernel rather than just the calling process.
2. **`/proc/self/environ` returned empty**, causing OpenRC's `init.sh` to
   believe procfs was stale ("cruft") and attempt to remount it on every boot
   iteration.
3. **`/proc/self/exe` pointed to the script, not the interpreter**, for
   shebang-executed scripts.  This was the root cause of the fork storm.

## Fix 1: Fork returns ENOMEM instead of panicking

`alloc_kernel_stack()` in `platform/stack_cache.rs` called `.expect()` on the
buddy allocator result.  A single failed fork under memory pressure took down
the entire kernel.

Changed `alloc_kernel_stack` to return `Result<OwnedPages, PageAllocError>`.
Propagated the error through `ArchTask::fork()` → `Process::fork()` →
`sys_fork()`, which now returns `ENOMEM` to userspace.  Boot-time allocations
(`new_kthread`, `new_idle_thread`, `new_user_thread`) keep their `.expect()`
since those are fatal anyway.

The same change was applied to both x86_64 and ARM64 `ArchTask::fork()` and
`ArchTask::new_thread()`.

## Fix 2: /proc/self/environ returns per-process content

OpenRC's `init.sh` checks whether `/proc` is real by comparing:

```sh
[ "$(VAR=a md5sum /proc/self/environ)" = "$(VAR=b md5sum /proc/self/environ)" ]
```

On Linux, each `md5sum` child process sees a different `/proc/self/environ`
(because `VAR=a` vs `VAR=b` is part of the initial environment).  Our stub
returned empty bytes for every process, so both md5sums matched and OpenRC
concluded `/proc` was fake.

Fixed `ProcPidEnviron` to return `KEVLAR_PID=<pid>\0` — a synthetic
per-process string.  This is enough to make the md5sum comparison differ
between the two child processes, so OpenRC correctly detects that `/proc` is
already mounted and sets `mountproc=false`.

## Fix 3: /proc/self/exe for shebang scripts (root cause)

### Symptom

Exec tracing showed the full call chain:

```
E#5  pid=7  ppid=5  /usr/libexec/rc/sh/init.sh        ← openrc runs init.sh
E#12 pid=17 ppid=7  grep -Eq [[:space:]]+xenfs$ ...    ← last cmd in init.sh
E#13 pid=19 ppid=17 eval_ecolors                       ← init.sh re-starts!
E#14 pid=22 ppid=17 einfo /proc is already mounted
E#19 pid=27 ppid=17 grep -Eq ...                       ← last cmd again
E#20 pid=29 ppid=27 eval_ecolors                       ← re-starts AGAIN
```

PID 17 was supposed to be `grep`, but it re-executed init.sh from the top.
PID 27 did the same.  Each iteration spawned ~10 child processes, producing
~5000 PIDs before the page allocator was exhausted.

### Root cause

BusyBox ash with `CONFIG_FEATURE_SH_STANDALONE=y` runs applets by doing:

```c
execve("/proc/self/exe", ["grep", "-Eq", ...], envp);
```

This re-execs the BusyBox binary (which is `/bin/busybox`) with `argv[0]` set
to the applet name.  BusyBox then dispatches to the `grep` applet.

But Kevlar's `Process::execve()` set `exe_path` to the **original path passed
to execve** — before shebang resolution.  For PID 7 (init.sh), the sequence
was:

1. `execve("/usr/libexec/rc/sh/init.sh", ...)`
2. Kernel detects `#!/bin/sh` shebang, loads `/bin/sh` (= BusyBox) as interpreter
3. But `exe_path` was already set to `/usr/libexec/rc/sh/init.sh`

So `/proc/self/exe` → `/usr/libexec/rc/sh/init.sh` (the script), not
`/bin/sh` (the interpreter).  When ash's child did
`execve("/proc/self/exe", ["grep", ...])`, it got init.sh back — which the
kernel re-interpreted via shebang as `/bin/sh init.sh`, re-running the entire
script instead of grep.

### Fix

In `do_script_binfmt()`, after resolving the shebang interpreter path, update
`exe_path` to the interpreter (e.g., `/bin/sh`):

```rust
let resolved = shebang_path.resolve_absolute_path();
let mut ep = current.exe_path.lock_no_irq();
ep.clear();
let _ = ep.try_push_str(resolved.as_str());
```

Linux's `/proc/self/exe` always points to the loaded ELF binary, not a script
file.  This matches that behavior.

## Supporting fixes

- **`/etc/group`**: Added standard Unix groups (`uucp`, `tty`, `wheel`, etc.)
  so OpenRC's `checkpath -o root:uucp /run/lock` succeeds.
- **`/etc/runlevels/`**: Created `sysinit`, `boot`, `default`, `shutdown`,
  `nonetwork` directories so OpenRC can determine runlevel state.

## Result

OpenRC boots cleanly to a login prompt:

```
   OpenRC 0.55.1 is starting up Linux 6.19.8 (x86_64)

 * /proc is already mounted
 * /run/openrc: creating directory
 * /run/lock: creating directory
 * Caching service dependencies ... [ ok ]

Kevlar (Alpine) kevlar /dev/ttyS0

kevlar login:
```

Fork under memory pressure now returns ENOMEM instead of crashing the kernel.
