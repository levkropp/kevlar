# M10 Phase 7: Multi-User + Security

**Goal:** Real UID/GID enforcement, file permissions, su/sudo, PAM basics.

## Why This Matters

Everything up to Phase 6 runs as root with no permission checks. A real
server needs:
- Non-root users (www-data, nobody, sshd)
- File permission enforcement (rwxrwxrwx bits, owner/group)
- setuid binaries (su, sudo, ping)
- Process credential tracking (real/effective/saved UID/GID)

## Scope

### File permission enforcement

Currently our VFS ignores permission bits. Need:
- Check `mode` bits against calling process's UID/GID on open/exec/stat
- Respect `S_ISUID` / `S_ISGID` bits on execve (set effective UID/GID)
- `access()` / `faccessat()` check real UID, not effective
- Root (UID 0) bypasses all checks (CAP_DAC_OVERRIDE)

### Real chown/chmod

Currently stubs. Need:
- Store owner UID/GID in inode metadata
- `chown(path, uid, gid)` updates inode
- `chmod(path, mode)` updates mode bits
- Propagate to stat() results

### Process credentials

Currently all processes run as UID 0. Need:
- `setuid(uid)` / `setgid(gid)` — change real + effective UID/GID
- `seteuid(uid)` / `setegid(gid)` — change effective only
- `setreuid()` / `setresuid()` — set real/effective/saved
- `setgroups()` — supplementary groups
- `getgroups()` — read supplementary groups
- Fork inherits parent credentials

### PAM (Pluggable Authentication Modules)

Ubuntu uses PAM for login/su/sudo. Alpine uses a simpler shadow-based
auth. For M10, support:
- `/etc/passwd` + `/etc/shadow` reading (BusyBox login already does this)
- `crypt()` password hashing (musl provides this)
- PAM stubs that always succeed (for Ubuntu compatibility later)

### su / sudo

- `su` — BusyBox or coreutils. Needs setuid bit, reads shadow, calls
  setuid/setgid, execs shell.
- `sudo` — more complex (reads /etc/sudoers). Can defer to Phase 8.

## Verification

```
# Create non-root user
adduser -D testuser
su - testuser
whoami  # "testuser"
cat /etc/shadow  # "Permission denied" (not root)
touch /tmp/myfile
ls -la /tmp/myfile  # owned by testuser
exit
sudo -u testuser id  # uid=1000(testuser) gid=1000(testuser)
```
