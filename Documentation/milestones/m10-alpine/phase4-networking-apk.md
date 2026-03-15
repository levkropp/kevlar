# M10 Phase 4: Networking + apk

**Goal:** Network connectivity and package installation from Alpine repos.

## Network Configuration

Alpine uses `/etc/network/interfaces` for static or DHCP configuration.
Our virtio-net driver + smoltcp stack handles TCP/UDP. DHCP is already
implemented for the kernel's own stack. For userspace:

- BusyBox `udhcpc` → needs raw socket (AF_PACKET) or uses the kernel's
  DHCP result if available
- Static IP: `ifconfig eth0 10.0.2.15 netmask 255.255.255.0 up`
  → needs SIOCSIFADDR, SIOCSIFNETMASK, SIOCSIFFLAGS ioctls

## apk Requirements

`apk add <package>` does:
1. DNS resolution (reads `/etc/resolv.conf`, connects to DNS server)
2. HTTPS fetch from `dl-cdn.alpinelinux.org` (needs TLS — musl uses
   `libssl` or `mbedtls`)
3. Tar extraction + package database update
4. Writable `/var/cache/apk/`, `/lib/apk/db/`

For QEMU with user-mode networking (`-netdev user`), the guest can
reach the internet via the host's network stack.

## SSH Server

We already have Dropbear compiled as a static musl binary. Alpine also
packages Dropbear. With networking working:

```
apk add dropbear
rc-service dropbear start
```

Or use our pre-built Dropbear binary.

## Kernel Changes

- **AF_PACKET raw socket**: BusyBox udhcpc may need it for DHCP.
  Alternative: pre-configure static IP or use kernel DHCP.
- **Network ioctls**: SIOCSIFADDR (0x8916), SIOCSIFNETMASK (0x891c),
  SIOCSIFFLAGS (0x8914), SIOCGIFADDR (0x8915), SIOCGIFFLAGS (0x8913).
  These are used by BusyBox ifconfig and ip commands.
- **DNS resolution**: musl's resolver reads `/etc/resolv.conf` and
  uses UDP socket to DNS server. Should work with our UDP stack.

## Verification

```
make test-m10-phase4
# Boot Alpine, configure network, install a package
# Expect: "apk add curl" succeeds, "curl --version" works
```

Success: `apk add` installs packages from Alpine repos, SSH is reachable.

## M10 Complete

With Phase 4 done, Alpine Linux boots to a fully functional text-mode
system with:
- Interactive login on serial console
- OpenRC service management
- Network connectivity
- Package installation
- SSH access
