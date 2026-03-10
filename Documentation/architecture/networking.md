# Networking

## TCP/IP: smoltcp

Kevlar uses [smoltcp 0.12](https://github.com/smoltcp-rs/smoltcp) for the TCP/IP
stack. smoltcp is a no-std, event-driven network stack that runs entirely inside the
kernel without its own thread.

The network stack is accessed through the `NetworkStackService` trait
(`kernel/net/service.rs`). The `SmoltcpNetworkStack` struct implements this trait
and is registered in the `ServiceRegistry` at boot.

### Interface

smoltcp drives the network interface via the `VirtioNet` driver
(`exts/virtio_net/`), which communicates with the QEMU virtio-net device over MMIO
or PCI. The driver provides a `Device` implementation that smoltcp polls for
incoming packets and fills with outgoing packets.

The network processing loop runs in the kernel's main event loop (timer interrupt
context), calling `iface.poll()` to advance TCP state machines and process queued
packets.

### Configuration

Network configuration at boot:
- **DHCP** (`ip=dhcp` kernel parameter) — smoltcp's built-in DHCP client
- **Static** (`ip=192.168.1.10/24` kernel parameter) — fixed IP/mask

The default gateway and DNS are set via kernel parameters or DHCP.

## Unix Domain Sockets

Unix domain sockets (`AF_UNIX`) are fully implemented, including:

- **SOCK_STREAM** — connection-oriented byte streams
- **SOCK_DGRAM** — connectionless datagrams
- **SOCK_SEQPACKET** — connection-oriented message streams
- `socketpair(AF_UNIX, ...)` — creates a connected socket pair
- **SCM_RIGHTS** — file descriptor passing via `sendmsg` / `recvmsg`
- **SCM_CREDENTIALS** — process credentials passing (UID/GID/PID)

Unix socket addresses are filesystem paths or anonymous (autobind). Named sockets
appear as `S_IFSOCK` nodes in the filesystem.

## Socket API

All socket types (`AF_INET`, `AF_INET6`, `AF_UNIX`) implement `FileLike` and share
the same fd-based interface:

| Syscall | Support |
|---|---|
| `socket` | AF_INET (TCP/UDP), AF_UNIX |
| `bind` | IP address + port, Unix path |
| `connect` | TCP three-way handshake (via smoltcp) |
| `listen` / `accept` | TCP server sockets |
| `send` / `recv` | Basic send/receive |
| `sendto` / `recvfrom` | UDP datagrams |
| `sendmsg` / `recvmsg` | With SCM_RIGHTS / SCM_CREDENTIALS |
| `setsockopt` / `getsockopt` | SO_REUSEADDR, SO_KEEPALIVE, TCP_NODELAY, etc. |
| `shutdown` | TCP half-close |
| `getsockname` / `getpeername` | Local and remote address |
| `socketpair` | AF_UNIX pairs |
| `poll` / `epoll` | Readiness monitoring on sockets |

## epoll

`epoll_create1`, `epoll_ctl`, `epoll_wait` are fully implemented. An `EpollInstance`
maintains a set of watched `FileLike` objects. `epoll_wait` sleeps on a `WaitQueue`
and is woken when any watched fd becomes ready.

Edge-triggered (`EPOLLET`) and level-triggered modes are supported.
