# Blog 162: kxserver Phase 1 — Wire Protocol and a 3-Year-Old Socket Bug

**Date:** 2026-04-14

## What Phase 1 was supposed to be

The plan for kxserver Phase 1 was straightforward: take the Phase 0
scaffolding (blog 161), add X11 wire-protocol primitives, implement the
`ConnectionSetup` handshake, and get a client to successfully connect.
Success criterion: `xlsclients -display :1` completes the handshake
cleanly.

Five modules, ~1000 LOC of Rust:

- `wire.rs` — little-endian readers/writers, request header parser,
  32-byte error/event block builders, opcode-name lookup, `pad4`
- `setup.rs` — `parse_setup_request` (the 12-byte handshake) and
  `build_success_reply` (the 144-byte server-info block with 1 screen,
  1 depth, 1 TrueColor visual, 2 pixmap formats, RGB masks 0xFF0000 /
  0xFF00 / 0xFF, vendor string "Kevlar kxserver")
- `client.rs` — per-client state machine: `NeedHeader → Established`,
  sequence counter, read/write buffers, resource-id base derivation
- `dispatch.rs` — opcode → handler match, Phase 1 returns
  `BadImplementation` for everything expecting a reply, logs everything
  with seq+name+raw
- `server.rs` — AF_UNIX listener, `poll()` loop, non-blocking accept +
  read + write, handshake pump

## Getting it working on native Linux first

I test everything against native Linux before Kevlar, because on native
Linux I have reference `xtrace` captures, real X clients, and the
ability to sanity-check my byte layout against an X server I didn't
write. It took one bug (index-out-of-bounds when `accept()` grew
`self.clients` mid-poll loop — fixed by processing listeners *after*
the client iteration, not before) and then `xdpyinfo` happily connected
and printed:

```
name of display:   :7
version number:    11.0
vendor string:     Kevlar kxserver
vendor release number:    11000000
...
screen #0:
  dimensions:    1024x768 pixels (340x255 millimeters)
  resolution:    76x76 dots per inch
  depths (1):    24
```

The log shows the full xdpyinfo probe sequence — `QueryExtension →
CreateGC → GetProperty → ListExtensions → QueryBestSize → FreeGC` —
each request logged with decoded fields and raw bytes. For the
opcodes `xdpyinfo` expected replies to (everything except `CreateGC`
and `FreeGC`), the server returned 32-byte `BadImplementation` errors,
which xdpyinfo reported as visible X errors but then exited cleanly.
That's the whole point of Phase 1: prove every byte crossing the wire
is inspectable in our code.

## The abstract-socket surprise

To make the filesystem path `/tmp/.X11-unix/X1` work on clients that
stat() before connect() (as Xlib does on some configurations), I added
a second listener on the abstract namespace path (`\0/tmp/.X11-unix/X1`).
Rust's stdlib `UnixListener::bind` doesn't expose abstract namespace
binding, so I drop to raw `libc::socket` + `libc::bind` + `libc::listen`
with a hand-built `sockaddr_un` that has `sun_path[0] = 0`.

On native Linux, `xdpyinfo -display :7` connected via the **abstract**
listener. The server log showed `C1 accepted (abstract) fd=5` —
xlib's fallback ladder reaches abstract faster than the filesystem
path in some environments. That second listener turned out to be
unnecessary for our use case (we could connect via filesystem just
fine), but it did expose a separate question we needed to answer:
why did stat() on the filesystem path fail on Kevlar?

## The Kevlar socket bug

On native Linux everything worked. On Kevlar, the server bound both
listeners, the log showed the expected messages, the client `connect()`
returned 0, the client `write()` pushed 12 bytes, and then the server's
`poll()` sat there forever. Zero `C1 accepted` log lines. Zero
handshake. Nothing.

I added progress markers, a pollfd timeout, an stat() of the filesystem
socket from inside the server immediately after bind (it failed with
`No such file or directory` — more on that in a moment), tried
abstract-only, tried filesystem-only, tried chrooting the client so it
was in the same filesystem view as the server. Client always reported
success, server never saw anything.

One thing nagged: if this was a general kernel bug, real Xorg wouldn't
work on Kevlar either. But `make test-twm` boots Xorg + twm + xterm
every day and passes. So the bug was specific to *something* kxserver
does that Xorg doesn't — or specific to the **order** in which kxserver
does things.

The answer was in `kernel/net/unix_socket.rs` line 39:

```rust
static UNIX_LISTENERS: SpinLock<VecDeque<(String, Arc<UnixListener>)>> =
    SpinLock::new(VecDeque::new());
```

**Strong Arc.** The registry maps bound paths to listener objects so
that `connect(path)` can find the backlog to push into. When a process
calls `listen()`, the kernel builds an `Arc<UnixListener>`, stores one
reference in the process's fd table, and stores another in this
registry. When the process exits, its fds close, and one of the two
`Arc`s drops. The other stays in the registry. The `UnixListener`'s
`Drop` impl — which is where `unregister_listener(path)` is called —
never runs, because the registry is still holding a strong reference to
it.

So after every kxserver run, a ghost listener is left behind in the
registry. The next kxserver registers a new entry for the same path.
Now there are two entries, and `find_listener` returns the first
match:

```rust
fn find_listener(path: &str) -> Option<Arc<UnixListener>> {
    UNIX_LISTENERS
        .lock()
        .iter()
        .find(|(p, _)| p == path)
        .map(|(_, l)| l.clone())
}
```

New clients get routed to the *dead* listener's backlog. The dead
listener's wait-queue has no subscribers, so the wake is a no-op. The
new kxserver polls its own (empty) backlog forever.

Our Phase 1 test harness hits this perfectly: Phase 3 starts kxserver,
binds, kills it (leaving a ghost). Phase 4 starts a second kxserver,
binds, logs "listening". Phase 5 connects — and goes to the Phase 3
ghost.

Why doesn't real Xorg hit this? Xorg binds exactly once per boot. The
ghost listener accumulates but nothing ever looks it up again. The bug
was dormant, waiting for the first userspace program to try to restart
a Unix-socket server without rebooting the kernel.

## The fix

Three lines of substance in `unix_socket.rs`:

```rust
static UNIX_LISTENERS: SpinLock<VecDeque<(String, Weak<UnixListener>)>> =
    SpinLock::new(VecDeque::new());

fn register_listener(path: &str, listener: &Arc<UnixListener>) {
    let mut table = UNIX_LISTENERS.lock();
    // Purge any existing entries for this path — dead ones (dangling Weak)
    // and live ones (should be very rare; Linux returns EADDRINUSE on a
    // double-bind, but we tolerate it and let the newest listener win).
    table.retain(|(p, _)| p != path);
    table.push_back((String::from(path), Arc::downgrade(listener)));
}

fn find_listener(path: &str) -> Option<Arc<UnixListener>> {
    let mut table = UNIX_LISTENERS.lock();
    let mut found: Option<Arc<UnixListener>> = None;
    table.retain(|(p, w)| {
        match w.upgrade() {
            Some(arc) => {
                if found.is_none() && p == path {
                    found = Some(arc);
                }
                true
            }
            None => false, // dangling — remove
        }
    });
    found
}
```

`Weak` doesn't count toward the strong refcount, so when a process
closes its listener fd the `Arc` count drops to 0, `Drop` fires,
`unregister_listener` runs. `find_listener` walks the registry,
upgrades each `Weak`, returns the first live match, and drops all
dangling entries along the way (so the registry doesn't grow
unbounded). `register_listener` purges any pre-existing entries for
the same path before pushing — `newest wins`, matching "last bind
takes over".

On the first test run after the fix, Phase 5 passed on Kevlar:

```
p5.reply head: 01 00 0b 00 00 00 22 00
p5.server_log_tail:
[--]         REQ C1 accepted fd=5
[--]         REQ C1 setup major=11 minor=0 auth_name="" auth_data_len=0
[--]         REQ C1 setup reply 144 bytes (expected 8+34*4)
[--]         REQ C1 hangup
```

Decoded: tag=1 (Success), protocol-major=11, protocol-minor=0,
length-in-words=0x0022=34, so total reply = 8 + 34×4 = 144 bytes.
Exactly what `build_success_reply` produces. Every byte accounted for.

## A word about the socket-visibility oddity

I mentioned above that `stat()` on `/tmp/.X11-unix/X1` from inside the
server immediately after `bind()` fails with `No such file or
directory`. This is separate from the Arc bug and does **not** prevent
the socket from working — `connect()` goes through a different code
path that consults `UNIX_LISTENERS` directly, not the VFS. So the socket
is invisible to `stat`/`readdir` but perfectly reachable for
`connect`. Real Xorg hits this too; it works for the same reason. The
"correct" fix (which is not urgent) would be to make tmpfs create a
visible socket inode as a side-effect of `bind()` — but that requires
threading socket ownership through the VFS, and for our purposes the
registry lookup is enough.

## Test results

| Suite | Result |
|---|---|
| `make test-kxserver` | **6/6** — mount rootfs, --help, listen log, dual-bind, **raw handshake over filesystem socket**, binary metadata |
| `make test-threads-smp` | **14/14** — including pipe_pingpong, fork_from_thread, thread_storm (heavy fd/process churn) |
| `make test-contracts` | **157–158/159** — only failures are pre-existing TCG timing flakes (`time.nanosleep_basic`, `time.clock_nanosleep_rel`), zero regressions from the `Weak` change |

The stale-listener bug fix is load-bearing for anyone who wants to
test AF_UNIX servers under Kevlar without rebooting between runs. It
also gives us real confidence that the Phase 1 code works
end-to-end inside Kevlar's kernel, not just on native Linux.

## What's next

Phase 2 is atoms: `InternAtom` and `GetAtomName`. The plan: pre-seed
the 68 predefined atoms from `Xatom.h` (PRIMARY=1, SECONDARY=2,
ARC=3, …, WM_ZOOM_HINTS=68), then grow an `AtomTable { by_id,
by_name, next }` dynamically for interns like `_NET_WM_NAME`,
`WM_DELETE_WINDOW`, `UTF8_STRING`. Success criterion: `xprop -display
:1 -root` connects and reports "no properties".

Because every phase lands with a blog post, I'll write blog 163 after
Phase 2. The logging infrastructure built in Phase 0 has already paid
off twice: once for xdpyinfo visibility on native Linux, and once for
making the stale-listener bug obvious ("C1 accepted fd=5" didn't
appear, so I knew the server never saw the client even though connect
reported success). Whatever Phase 2 does, we'll see every byte of it.
