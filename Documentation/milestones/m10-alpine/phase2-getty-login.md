# M10 Phase 2: getty + login

**Goal:** Interactive shell via serial console — type commands, see output.

## getty Requirements

BusyBox getty (`/sbin/getty -L 115200 ttyS0 vt100`):

1. `open("/dev/ttyS0", O_RDWR | O_NONBLOCK)` — open serial device
2. `fcntl(fd, F_SETFL, 0)` — clear O_NONBLOCK
3. `dup2(fd, 0); dup2(fd, 1); dup2(fd, 2)` — redirect stdio
4. `fchown(0, 0, 0)` — set tty ownership
5. `fchmod(0, 0620)` — set tty permissions
6. `ioctl(0, TIOCSCTTY, 0)` — set controlling terminal
7. `tcsetpgrp(0, getpid())` — set foreground process group
8. `tcgetattr(0, &t)` / `tcsetattr(0, &t)` — configure terminal
9. Write `/etc/issue` banner, read username
10. `exec("/bin/login", username)`

## Kernel Changes

- **TIOCSCTTY (0x540E)**: Set controlling terminal for session leader.
  Store the controlling tty in the process's session. Verify our TTY
  layer supports this — it may already work via the pty code.

- **TIOCSPGRP (0x5410)**: Set foreground process group of terminal.
  Used by `tcsetpgrp()`. May already be implemented in pty.rs.

- **fchown**: Implement properly (currently stub returning Ok(0)).
  Getty calls `fchown(0, 0, 0)` to set tty ownership. Since we don't
  enforce Unix permissions, the stub may be sufficient.

- **/dev/ttyS0**: Create a serial device node that acts as an alias
  for the kernel serial console. Read from serial input, write to
  serial output. Our existing SERIAL_TTY provides this; we just need
  to expose it at the right path.

## login Requirements

BusyBox login reads `/etc/passwd` and `/etc/shadow`. Alpine's default
root has no password (empty field in shadow). Login sequence:

1. `getpwnam("root")` — reads `/etc/passwd` (libc does this, not a syscall)
2. `getspnam("root")` — reads `/etc/shadow`
3. If no password → skip prompt, proceed
4. `setgid(0)`, `setuid(0)` — set credentials
5. `chdir("/root")` — change to home
6. `exec("/bin/sh")` — launch login shell

## Verification

```
# Automated test: send "root\n" via serial, check for "# " prompt
make test-m10-phase2
```

Success: typing `root` at "alpine login:" produces a `# ` shell prompt.
Interactive commands (`ls`, `cat /proc/version`, `uname -a`) work.
