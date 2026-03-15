# M10 Phase 6: Networking

**Goal:** Full userspace networking — DHCP, DNS, SSH, package downloads.

## Current State

Kernel has TCP/UDP via smoltcp + virtio-net. Works for kernel-internal
networking (DHCP discovery at boot, TCP connections). But userspace
networking needs additional interfaces.

## Scope

### Network ioctls

BusyBox `ifconfig` and `ip` use socket ioctls:

- `SIOCGIFFLAGS` (0x8913) — get interface flags (UP, RUNNING, etc.)
- `SIOCSIFFLAGS` (0x8914) — set interface flags (bring UP/DOWN)
- `SIOCGIFADDR` (0x8915) — get IP address
- `SIOCSIFADDR` (0x8916) — set IP address
- `SIOCGIFNETMASK` (0x891b) — get netmask
- `SIOCSIFNETMASK` (0x891c) — set netmask
- `SIOCGIFHWADDR` (0x8927) — get MAC address
- `SIOCGIFINDEX` (0x8933) — get interface index

These operate on our smoltcp network interface.

### DHCP client

BusyBox `udhcpc` needs:
- Raw socket (`AF_PACKET`, `SOCK_RAW`) for DHCP broadcast
- Or: use our kernel's existing DHCP and expose the result to userspace
  via `/var/run/udhcpc.leases` or similar

Simplest path: pre-configure static IP from kernel DHCP, let userspace
read the configuration. Or implement AF_PACKET minimally.

### DNS resolution

musl's resolver reads `/etc/resolv.conf` and sends UDP queries.
This should work with our existing UDP socket implementation.
Pre-configure `/etc/resolv.conf` with QEMU's default DNS (10.0.2.3).

### iptables / nftables

Full netfilter is a large subsystem. For M10, stub it:
- `setsockopt(SOL_IP, IP_ADD_MEMBERSHIP)` — multicast stubs
- iptables binary can return "not supported" gracefully
- nftables similarly

Real packet filtering can be M11 or later.

### AF_NETLINK

Many network tools use netlink for configuration. This is a significant
implementation (NETLINK_ROUTE for interface/address/route management).

Options:
1. Implement minimal NETLINK_ROUTE (ADD/DEL address, link up/down)
2. Stub it and rely on ioctl-based tools (ifconfig)
3. Implement it properly (large effort)

Start with option 2, add NETLINK_ROUTE incrementally.

## Verification

```
# In Alpine guest:
ifconfig eth0 10.0.2.15 netmask 255.255.255.0 up
route add default gw 10.0.2.2
echo "nameserver 10.0.2.3" > /etc/resolv.conf
ping 10.0.2.2  # gateway
wget http://dl-cdn.alpinelinux.org/alpine/MIRRORS.txt  # HTTP works
apk update && apk add curl  # package installation
ssh root@localhost  # SSH server reachable
```
