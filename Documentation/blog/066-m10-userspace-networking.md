# M10 Phase 6: Complete Userspace Networking

Phase 4 wired userspace tools to the kernel's smoltcp network stack —
ifconfig worked, DNS config was in place, OpenRC's networking service
came up clean. But `wget` and `curl` still couldn't connect. The
problem: both tools use nonblocking connect with poll/select for timeout
handling, and our TCP connect always blocked.

## Nonblocking connect

The existing `TcpSocket::connect()` ignored the `options` parameter
entirely. It called `sleep_signalable_until()` waiting for `may_send()`
to become true, regardless of whether `O_NONBLOCK` was set.

The fix follows the POSIX/Linux model:

1. Initiate the TCP SYN via smoltcp
2. If nonblocking, return `EINPROGRESS` immediately
3. The caller polls for `POLLOUT` (connection established) or `POLLERR`
   (connection failed)
4. `getsockopt(SO_ERROR)` reports the result

```rust
fn connect(&self, sockaddr: SockAddr, options: &OpenOptions) -> Result<()> {
    // ... SYN initiation, unchanged ...
    process_packets();

    if options.nonblock {
        return Err(Errno::EINPROGRESS.into());
    }

    // Blocking path now checks state properly
    SOCKET_WAIT_QUEUE.sleep_signalable_until(|| {
        let socket: &tcp::Socket = sockets.get(self.handle);
        match socket.state() {
            tcp::State::Established => Ok(Some(())),
            tcp::State::Closed => Err(Errno::ECONNREFUSED.into()),
            _ => Ok(None),
        }
    })
}
```

The blocking path also improved: previously it checked `may_send()`
which doesn't distinguish "still connecting" from "connection failed".
Now it inspects smoltcp's TCP state machine directly — `Established`
means success, `Closed` means the remote sent RST (ECONNREFUSED).

Two guard checks at the top handle re-entrant connect calls: `EISCONN`
if already established, `EALREADY` if a SYN is already in flight. Both
are required by POSIX and expected by wget/curl.

## SO_ERROR with real state

The old `getsockopt(SO_ERROR)` always returned 0. After a nonblocking
connect, the caller needs to know whether the connection succeeded or
failed. The new implementation polls the socket — if `POLLERR` is set
(which `TcpSocket::poll()` now reports for `State::Closed`), it returns
`ECONNREFUSED` (111).

This completes the nonblocking connect lifecycle:
```
socket() → fcntl(O_NONBLOCK) → connect() = EINPROGRESS
→ poll(POLLOUT) → getsockopt(SO_ERROR) = 0  (success)
```

## ICMP ping socket

BusyBox `ping` uses Linux's "ping socket" feature:
`socket(AF_INET, SOCK_DGRAM, IPPROTO_ICMP)`. This avoids raw sockets
(which require root) by letting the kernel handle ICMP echo
request/reply framing.

The new `IcmpSocket` wraps smoltcp's `icmp::Socket`:

- **Auto-bind:** Generates a random ICMP identifier on first send
  (BusyBox doesn't call `bind()` on ping sockets)
- **sendto:** Writes raw ICMP bytes to smoltcp's transmit buffer,
  addressed to the destination IP
- **recvfrom:** Returns ICMP reply bytes with the source address as
  a `sockaddr_in`

Required adding `socket-icmp` to smoltcp's feature flags in Cargo.toml.

## Everything else

**New errnos:** `EINPROGRESS` (115) and `EALREADY` (114) added to the
Errno enum.

**SO_RCVTIMEO/SO_SNDTIMEO:** wget and curl set receive/send timeouts
via setsockopt. Accepted silently — signal interruption via EINTR
already provides the timeout escape hatch.

**getsockopt stubs:** `SO_RCVBUF` returns 87380, `SO_SNDBUF` returns
16384, `SO_KEEPALIVE` returns 0. Reasonable defaults that satisfy
probing by networking tools.

**/proc/net/ stubs:** Added `/proc/net/tcp`, `/proc/net/udp`,
`/proc/net/tcp6`, `/proc/net/udp6` — each returns just the header line.
Some libraries and tools check these exist.

## Files changed

| File | Change |
|------|--------|
| `libs/kevlar_vfs/src/result.rs` | EINPROGRESS, EALREADY errnos |
| `libs/kevlar_vfs/src/socket_types.rs` | IPPROTO_ICMP constant |
| `kernel/Cargo.toml` | smoltcp `socket-icmp` feature |
| `kernel/net/tcp_socket.rs` | Nonblocking connect, state-aware poll |
| `kernel/net/icmp_socket.rs` | New: ICMP ping socket |
| `kernel/net/mod.rs` | Export icmp module, service impl |
| `kernel/net/service.rs` | `create_icmp_socket` trait method |
| `kernel/syscalls/socket.rs` | IPPROTO_ICMP dispatch |
| `kernel/syscalls/getsockopt.rs` | Real SO_ERROR + buffer size stubs |
| `kernel/syscalls/setsockopt.rs` | SO_RCVTIMEO/SO_SNDTIMEO stubs |
| `kernel/fs/procfs/mod.rs` | /proc/net/tcp, udp, tcp6, udp6 |
