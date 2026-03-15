# M9 Phase 3.3: Service Lifecycle and Target Reach

## Status After Phase 3.2

systemd v245 boots on Kevlar under KVM:
- Loads 12+ glibc shared libraries via dynamic linker
- Detects KVM virtualization, x86-64 architecture
- Loads `default.target` and `kevlar-getty.service` from `/etc/systemd/system/`
- Forks child process (PID 2), logs "Started Kevlar Console Shell."
- Enters sd-event main loop

Remaining issues:
- "connect() failed: Invalid argument" — AF_UNIX bind/connect not working
- `/sys/fs/cgroup/system.slice/` hierarchy doesn't exist
- No "Reached target" logged
- Child process (/bin/sh) may not have working TTY

## Phase 3.3 Goals

1. **AF_UNIX bind/connect** — systemd's sd_notify socket needs bind() at
   `/run/systemd/notify`. Connect from child processes needs to work.

2. **Cgroup hierarchy** — systemd creates `system.slice/kevlar-getty.service/`
   under `/sys/fs/cgroup/`. The cgroupfs mkdir works but the full path
   traversal through mount points may fail.

3. **Missing /proc files** — `/proc/sys/kernel/pid_max` (systemd reads
   to determine PID space), `/proc/2/stat` must work for child monitoring.

4. **Service output** — kevlar-getty.service runs `/bin/sh` with
   StandardInput=tty. The child needs a working /dev/console or /dev/tty.

5. **"Reached target"** — systemd should log when default.target's
   dependencies are satisfied.

6. **Clean shutdown** — reboot(POWER_OFF) from systemd should halt QEMU.

## Verification

```
make test-systemd  # boots systemd, checks for "Started Kevlar Console Shell"
```

Success criteria: "Reached target Kevlar Default Target" appears in output.
