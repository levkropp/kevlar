# Phase 3: Unix Socket Enhancements + D-Bus Support

**Goal:** Extend AF_UNIX sockets with named binding, fd passing (SCM_RIGHTS),
and accept4. This enables D-Bus, which systemd requires for service management.

**Prerequisite:** Phase 1 (Pollable trait for socket epoll integration).

## Syscalls

| Syscall | Number | Priority | Notes |
|---------|--------|----------|-------|
| `sendmsg` | 46 | Required | Send with ancillary data (cmsg) |
| `recvmsg` | 47 | Required | Receive with ancillary data |
| `accept4` | 288 | Required | Accept with SOCK_CLOEXEC/SOCK_NONBLOCK |
| `setsockopt` | 54 | Required | SO_REUSEADDR, SO_PASSCRED at minimum |

## Current Socket State

Kevlar has basic TCP/UDP via smoltcp and a minimal AF_UNIX socketpair
implementation. What's missing for D-Bus:

1. **Named AF_UNIX sockets** — bind() to a filesystem path like
   `/run/dbus/system_bus_socket`, listen(), accept() on it
2. **SCM_RIGHTS** — pass file descriptors between processes via sendmsg/recvmsg
3. **SCM_CREDENTIALS** — pass pid/uid/gid (SO_PASSCRED)
4. **SOCK_STREAM reliable delivery** — our socketpair works but named sockets
   need a connection backlog

## Design

### Named AF_UNIX Sockets

```rust
struct UnixListener {
    path: PathBuf,
    backlog: VecDeque<UnixStream>,  // pending connections
    wait_queue: WaitQueue,          // wake on incoming connection
}

struct UnixStream {
    /// Ring buffer for each direction.
    tx: Arc<SpinLock<RingBuffer>>,
    rx: Arc<SpinLock<RingBuffer>>,
    /// Pending ancillary data (SCM_RIGHTS fds, SCM_CREDENTIALS).
    ancillary_rx: VecDeque<AncillaryData>,
    peer_closed: AtomicBool,
    wait_queue: WaitQueue,
}
```

**Filesystem integration:** `bind()` creates a socket inode in the VFS at the
given path. `connect()` looks up that path and establishes a stream pair.
`accept()` dequeues from the backlog.

### SCM_RIGHTS (fd passing)

The core mechanism D-Bus uses to share resources between processes:

```rust
enum AncillaryData {
    Rights(Vec<Arc<OpenedFile>>),   // file descriptions to transfer
    Credentials { pid: PId, uid: u32, gid: u32 },
}
```

**sendmsg flow:**
1. Parse `struct msghdr` from userspace
2. For each `SCM_RIGHTS` cmsg: look up fds in sender's fd table, clone the
   Arc<OpenedFile>
3. Attach ancillary data to the message in the ring buffer
4. Write message data to ring buffer

**recvmsg flow:**
1. Read message data from ring buffer
2. For each `SCM_RIGHTS` cmsg: allocate new fds in receiver's fd table,
   install the Arc<OpenedFile>
3. Write cmsg back to userspace msghdr

### accept4

Simple extension of accept: after accepting, apply SOCK_CLOEXEC and/or
SOCK_NONBLOCK flags to the new fd. This is what every server uses.

### setsockopt

Minimum set for systemd + D-Bus:
- `SO_REUSEADDR` — allow rebinding to in-use addresses
- `SO_PASSCRED` — enable SCM_CREDENTIALS on AF_UNIX
- `SO_KEEPALIVE` — TCP keepalive (stub OK initially)
- `TCP_NODELAY` — disable Nagle (stub OK initially)

## Files to Create/Modify

- `kernel/net/unix.rs` (NEW or major rewrite) — UnixListener, UnixStream,
  named socket support, ancillary data
- `kernel/syscalls/sendmsg.rs` (NEW) — sendmsg with cmsg parsing
- `kernel/syscalls/recvmsg.rs` (NEW) — recvmsg with cmsg construction
- `kernel/syscalls/accept.rs` — add accept4 variant
- `kernel/syscalls/setsockopt.rs` (NEW or extend)
- `kernel/net/socket.rs` — Pollable impl for unix sockets
- `kernel/fs/inode.rs` — Socket inode type for named sockets

## Integration Test

```c
// Test: named AF_UNIX socket with fd passing
int sfd = socket(AF_UNIX, SOCK_STREAM, 0);
struct sockaddr_un addr = { .sun_family = AF_UNIX };
strcpy(addr.sun_path, "/tmp/test.sock");
bind(sfd, (struct sockaddr *)&addr, sizeof(addr));
listen(sfd, 5);

if (fork() == 0) {
    // Child: connect and send a pipe fd
    int cfd = socket(AF_UNIX, SOCK_STREAM, 0);
    connect(cfd, (struct sockaddr *)&addr, sizeof(addr));

    int pipefd[2];
    pipe(pipefd);
    write(pipefd[1], "hello", 5);

    // Send pipefd[0] via SCM_RIGHTS
    struct msghdr msg = { 0 };
    char cmsgbuf[CMSG_SPACE(sizeof(int))];
    msg.msg_control = cmsgbuf;
    msg.msg_controllen = sizeof(cmsgbuf);
    struct cmsghdr *cmsg = CMSG_FIRSTHDR(&msg);
    cmsg->cmsg_level = SOL_SOCKET;
    cmsg->cmsg_type = SCM_RIGHTS;
    cmsg->cmsg_len = CMSG_LEN(sizeof(int));
    *(int *)CMSG_DATA(cmsg) = pipefd[0];
    struct iovec iov = { .iov_base = "x", .iov_len = 1 };
    msg.msg_iov = &iov;
    msg.msg_iovlen = 1;
    sendmsg(cfd, &msg, 0);
    _exit(0);
}

// Parent: accept and receive the fd
int afd = accept4(sfd, NULL, NULL, SOCK_CLOEXEC);
struct msghdr msg = { 0 };
char cmsgbuf[CMSG_SPACE(sizeof(int))];
char buf[1];
struct iovec iov = { .iov_base = buf, .iov_len = 1 };
msg.msg_iov = &iov;
msg.msg_iovlen = 1;
msg.msg_control = cmsgbuf;
msg.msg_controllen = sizeof(cmsgbuf);
recvmsg(afd, &msg, 0);

struct cmsghdr *cmsg = CMSG_FIRSTHDR(&msg);
int received_fd = *(int *)CMSG_DATA(cmsg);
char data[6] = {0};
read(received_fd, data, 5);
assert(strcmp(data, "hello") == 0);
printf("TEST_PASS unix_fd_passing\n");
```

## Reference

- FreeBSD: `sys/kern/uipc_usrreq.c` (AF_UNIX), `sys/kern/uipc_rights.c`
  (SCM_RIGHTS)
- Linux man pages: unix(7), cmsg(3), sendmsg(2), recvmsg(2)

## Estimated Complexity

~800-1000 lines. This is the largest phase. Named unix sockets and SCM_RIGHTS
are architecturally significant — they touch the VFS (socket inodes), fd table
(fd passing), and network stack (new socket type).
