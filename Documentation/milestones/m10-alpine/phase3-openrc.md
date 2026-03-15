# M10 Phase 3: OpenRC

**Goal:** Alpine boots with OpenRC managing services, reaching the
default runlevel.

## OpenRC Boot Sequence

BusyBox init invokes OpenRC three times via inittab:

```
::sysinit:/sbin/openrc sysinit
::sysinit:/sbin/openrc boot
::wait:/sbin/openrc default
```

OpenRC is a transient process — it runs, starts services, and exits.
It is NOT a daemon like systemd.

### sysinit runlevel

- **devfs**: Mounts `/dev` as devtmpfs, creates seed device nodes
  with mknod, mounts `/dev/pts` and `/dev/shm`
- **dmesg**: Sets kernel log level via `klogctl()`

### boot runlevel

- **sysfs**: Mounts `/sys`
- **hostname**: Reads `/etc/hostname`, calls `sethostname()`
- **bootmisc**: Cleans `/tmp`, creates runtime directories
- **networking**: Reads `/etc/network/interfaces`, configures lo + eth0

### default runlevel

- User-defined services (sshd, crond, etc.)

## Kernel Changes

- **mknod**: Implement real device node creation in tmpfs/initramfs.
  OpenRC's devfs service calls `mknod /dev/console c 5 1` etc.
  Our devfs has these pre-created, but the tmpfs overlay needs the
  mknod syscall to actually create character/block device inodes.

- **klogctl (syslog syscall)**: OpenRC's dmesg service uses this
  to set the console log level. We have sys_syslog but it may need
  the SYSLOG_ACTION_CONSOLE_LEVEL command.

- **Writable /proc/sys/**: OpenRC writes to `/proc/sys/kernel/hostname`
  and other sysctl files. Our ProcSysStaticFile accepts writes but
  doesn't update the kernel state. Wire hostname writes to
  `sethostname()`.

- **/etc/network/interfaces**: BusyBox ifconfig/ip needs to work.
  AF_INET socket + ioctl(SIOCSIFADDR) for IP configuration.
  Our smoltcp-based stack may need DHCP or static IP setup.

## Initramfs Changes

Install OpenRC package into the Alpine rootfs:

```dockerfile
FROM alpine:3.21 AS alpine_rootfs
RUN apk add --no-cache openrc alpine-baselayout
RUN rc-update add devfs sysinit
RUN rc-update add hostname boot
RUN rc-update add networking boot
```

## Verification

```
# Check for OpenRC completion message
make test-m10-phase3
# Expect: "* Starting hostname ... [ ok ]"
# Expect: "alpine login:" after all services start
```

Success: OpenRC processes sysinit → boot → default runlevels without
errors, then getty displays "alpine login:".
