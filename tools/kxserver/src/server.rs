// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// Server main loop.
//
// * Create `/tmp/.X11-unix/` (0755) if it does not exist.
// * Bind an AF_UNIX stream socket on the filesystem path
//   `/tmp/.X11-unix/Xn` where n is the configured display number.
// * Mark the socket non-blocking and listen.
// * Run a poll() loop: listen socket + each accepted client.
// * On accept: wrap into a Client and initialize with HandshakeState::NeedHeader.
// * On readable: read as much as we can into client.read_buf, then pump the
//   state machine: parse SetupRequest → build reply → drain requests via
//   dispatch::dispatch_request.
// * On writable: flush client.write_buf; if the full buffer is written and
//   the client is marked for close, drop it.

use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::client::{Client, HandshakeState};
use crate::device::{InputEvent, KeyboardReader, MouseReader};
use crate::dispatch::{self, route_input_event, DispatchResult};
use crate::log;
use crate::setup::{self, SetupError, SetupRequest};
use crate::state::ServerState;

/// Shutdown flag, set by SIGTERM/SIGINT handler.  The poll loop checks
/// this between iterations and returns `Interrupted` so main can dump a
/// final PPM snapshot.
pub static SHUTDOWN_FLAG: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_term_signal(_sig: libc::c_int) {
    SHUTDOWN_FLAG.store(true, Ordering::SeqCst);
}

/// Install SIGTERM + SIGINT handlers that flip `SHUTDOWN_FLAG`.  Idempotent.
pub fn install_shutdown_handlers() {
    unsafe {
        let mut sa: libc::sigaction = MaybeUninit::zeroed().assume_init();
        sa.sa_sigaction = handle_term_signal as *const () as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        sa.sa_flags = 0;
        libc::sigaction(libc::SIGTERM, &sa, core::ptr::null_mut());
        libc::sigaction(libc::SIGINT,  &sa, core::ptr::null_mut());
    }
}

/// Result of a Server run.
pub enum RunError {
    BindFailed(String),
    PollFailed(String),
    Interrupted,
}

pub struct Server {
    /// Filesystem-path listener (`/tmp/.X11-unix/X1`).  Bind may succeed
    /// even on kernels that do not make the inode visible via stat/readdir
    /// (Kevlar); clients that probe the path first may still fall back to
    /// the abstract listener below.
    listener: UnixListener,
    /// Abstract-namespace listener (`\0/tmp/.X11-unix/X1`).  Required for
    /// xlib fallback when the filesystem path is not stat-able.  Held as a
    /// raw fd because Rust stdlib does not expose abstract binding.
    abstract_fd: Option<RawFd>,
    socket_path: PathBuf,
    clients: Vec<Client>,
    next_client_id: u32,
    /// Shared state visible to every handler (atoms, screens, resources).
    /// Held in a separate field so the dispatch hot path can borrow
    /// `&mut state` disjointly from the current `&mut Client`.
    state: ServerState,
    /// Optional device readers.  `None` on the host dev environment
    /// where /dev/input/* is permission-denied; the server still runs
    /// correctly, just without real input.
    mouse: Option<MouseReader>,
    keyboard: Option<KeyboardReader>,
    /// Pending synthetic InputEvents from `--inject=...`.  These are
    /// drained on the first poll_once after a client has connected,
    /// so smoke tests can set up window state before events route.
    pending_inject: Vec<InputEvent>,
    injection_fired: bool,
}

/// Bind a Unix stream socket in the abstract namespace.  Path is serialized
/// as `\0<path_bytes>` in sun_path.  Returns the listening fd.
fn bind_abstract(abs_path: &[u8]) -> Result<RawFd, String> {
    unsafe {
        let fd = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_NONBLOCK, 0);
        if fd < 0 {
            return Err(format!("socket() errno={}", *libc::__errno_location()));
        }
        let mut sun: libc::sockaddr_un = MaybeUninit::zeroed().assume_init();
        sun.sun_family = libc::AF_UNIX as libc::sa_family_t;
        // sun_path is signed char on Linux.
        // sun_path[0] = 0 (abstract namespace marker).
        if abs_path.len() + 1 > sun.sun_path.len() {
            libc::close(fd);
            return Err(format!(
                "abstract path too long: {} bytes",
                abs_path.len() + 1
            ));
        }
        sun.sun_path[0] = 0;
        for (i, b) in abs_path.iter().enumerate() {
            sun.sun_path[i + 1] = *b as _;
        }
        // Address length includes the leading sun_family (2 bytes) + 1 null +
        // path bytes.
        let addr_len = (core::mem::size_of::<libc::sa_family_t>()
                        + 1
                        + abs_path.len()) as libc::socklen_t;
        let rc = libc::bind(
            fd,
            &sun as *const _ as *const libc::sockaddr,
            addr_len,
        );
        if rc < 0 {
            let errno = *libc::__errno_location();
            libc::close(fd);
            return Err(format!("bind abstract errno={errno}"));
        }
        let rc = libc::listen(fd, 16);
        if rc < 0 {
            let errno = *libc::__errno_location();
            libc::close(fd);
            return Err(format!("listen abstract errno={errno}"));
        }
        Ok(fd)
    }
}

/// Accept a connection on a raw AF_UNIX fd.  Returns the accepted fd or None
/// if no pending connection.
fn accept_raw(listen_fd: RawFd) -> Option<RawFd> {
    unsafe {
        let fd = libc::accept4(
            listen_fd,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            libc::SOCK_NONBLOCK,
        );
        if fd >= 0 {
            Some(fd)
        } else {
            let errno = *libc::__errno_location();
            if errno != libc::EAGAIN && errno != libc::EWOULDBLOCK {
                log::warn(format_args!("accept4 abstract errno={errno}"));
            }
            None
        }
    }
}

/// Parse one `--inject=` spec into an `InputEvent`.  Returns None
/// for unrecognized specs, which the caller logs.
fn parse_inject(spec: &str) -> Option<InputEvent> {
    let mut parts = spec.split(':');
    let kind = parts.next()?;
    match kind {
        "motion" => {
            let dx: i16 = parts.next()?.parse().ok()?;
            let dy: i16 = parts.next()?.parse().ok()?;
            Some(InputEvent::MouseMotion { dx, dy })
        }
        "button" => {
            let button: u8 = parts.next()?.parse().ok()?;
            let pressed = parts.next()? == "down";
            Some(InputEvent::MouseButton { button, pressed })
        }
        "key" => {
            let keycode: u8 = parts.next()?.parse().ok()?;
            let pressed = parts.next()? == "down";
            Some(InputEvent::Key { keycode, pressed })
        }
        _ => None,
    }
}

impl Server {
    pub fn bind(display: u16) -> Result<Self, RunError> {
        let dir = PathBuf::from("/tmp/.X11-unix");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return Err(RunError::BindFailed(
                format!("create /tmp/.X11-unix: {e}"),
            ));
        }
        // 0777 + sticky bit, matching real Xorg.  ignore permission errors —
        // the dir may already exist with other perms.
        let _ = std::fs::set_permissions(
            &dir,
            std::os::unix::fs::PermissionsExt::from_mode(0o1777),
        );

        let path = dir.join(format!("X{display}"));
        // Remove any stale socket from a previous crashed server.
        let _ = std::fs::remove_file(&path);

        let listener = UnixListener::bind(&path).map_err(|e| {
            RunError::BindFailed(format!("bind {}: {e}", path.display()))
        })?;
        listener.set_nonblocking(true).map_err(|e| {
            RunError::BindFailed(format!("set non-blocking: {e}"))
        })?;
        log::info(format_args!(
            "listening (filesystem) on {} (display :{display})",
            path.display()
        ));

        // Also bind the abstract namespace.  Some kernels (e.g. Kevlar at
        // the time of writing) accept filesystem bind() but don't make the
        // socket inode visible via stat/readdir — xlib does `stat()` before
        // `connect()` and falls back to other transports when that fails.
        // Binding the abstract path as a second listener lets xlib find us
        // via its @<path> fallback.
        let abstract_path = format!("/tmp/.X11-unix/X{display}");
        let abstract_fd = match bind_abstract(abstract_path.as_bytes()) {
            Ok(fd) => {
                log::info(format_args!(
                    "listening (abstract) on @{} (display :{display})",
                    abstract_path
                ));
                Some(fd)
            }
            Err(e) => {
                log::warn(format_args!(
                    "abstract bind failed: {e} (filesystem listener only)"
                ));
                None
            }
        };

        let mouse = MouseReader::open();
        if mouse.is_some() {
            log::info(format_args!("mouse: /dev/input/mice opened"));
        } else {
            log::info(format_args!("mouse: no device (running without mouse input)"));
        }
        let keyboard = KeyboardReader::open();
        if keyboard.is_none() {
            log::info(format_args!("keyboard: no device (running without keyboard input)"));
        }

        Ok(Server {
            listener,
            abstract_fd,
            socket_path: path,
            clients: Vec::new(),
            next_client_id: 1,
            state: ServerState::new(),
            mouse,
            keyboard,
            pending_inject: Vec::new(),
            injection_fired: false,
        })
    }

    /// Queue synthetic input events (from `--inject=` CLI flags).
    /// Events fire once, on the first poll_once iteration after
    /// `self.clients.len() > 0`, so tests can set up windows first.
    pub fn inject_events(&mut self, specs: &[String]) {
        for spec in specs {
            match parse_inject(spec) {
                Some(ev) => self.pending_inject.push(ev),
                None => log::warn(format_args!("bad --inject spec: {spec}")),
            }
        }
    }

    pub fn run(&mut self) -> Result<(), RunError> {
        // We use libc::poll directly for simplicity.  The Rust stdlib has
        // no poll wrapper and pulling in mio for a diagnostic tool is overkill.
        loop {
            if SHUTDOWN_FLAG.load(Ordering::SeqCst) {
                return Err(RunError::Interrupted);
            }
            self.poll_once()?;
        }
    }

    /// Exposed for `main` so it can dump the final framebuffer snapshot
    /// to a PPM file when `--ppm-on-exit` was supplied.
    pub fn framebuffer_mut(&mut self) -> &mut crate::fb::Framebuffer {
        &mut self.state.fb
    }

    fn poll_once(&mut self) -> Result<(), RunError> {
        // Layout: [filesystem listener, abstract listener (optional),
        //          mouse fd (optional), keyboard fd (optional),
        //          clients...]
        let mut fds: Vec<libc::pollfd> = Vec::with_capacity(4 + self.clients.len());
        fds.push(libc::pollfd {
            fd: self.listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        });
        if let Some(fd) = self.abstract_fd {
            fds.push(libc::pollfd { fd, events: libc::POLLIN, revents: 0 });
        }
        let mouse_idx = if let Some(m) = &self.mouse {
            let idx = fds.len();
            fds.push(libc::pollfd {
                fd: m.raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            });
            Some(idx)
        } else { None };
        let kbd_idx = if let Some(k) = &self.keyboard {
            let idx = fds.len();
            fds.push(libc::pollfd {
                fd: k.raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            });
            Some(idx)
        } else { None };
        let client_base = fds.len();
        for c in &self.clients {
            let mut events = libc::POLLIN;
            if !c.write_buf.is_empty() {
                events |= libc::POLLOUT;
            }
            fds.push(libc::pollfd {
                fd: c.raw_fd(),
                events,
                revents: 0,
            });
        }

        let rc = unsafe {
            libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1)
        };
        if rc < 0 {
            let errno = unsafe { *libc::__errno_location() };
            if errno == libc::EINTR {
                return Ok(());
            }
            return Err(RunError::PollFailed(format!("poll errno={errno}")));
        }

        // Clients FIRST, then listeners.  Processing a listener grows
        // self.clients, which would invalidate our precomputed pollfd
        // indices.  Newly-accepted clients wait for the next poll_once.
        //
        // Borrow `self.state` outside the loop so we can pass `&mut state`
        // into each client's pump call — the borrow checker can prove
        // this disjoint from `self.clients.iter_mut()` because `state`
        // is a separate field.
        let state = &mut self.state;
        let mut drops: Vec<usize> = Vec::new();
        for (i, c) in self.clients.iter_mut().enumerate() {
            let pollfd = fds[client_base + i];
            if pollfd.revents & libc::POLLERR != 0 {
                log::info(format_args!("C{} socket error", c.id));
                drops.push(i);
                continue;
            }
            // Always drain POLLIN first, even when POLLHUP is set.  Linux
            // reports POLLIN | POLLHUP together when the peer closes its
            // write side but we still have buffered data from before the
            // close — dropping on HUP without reading loses the tail.
            let mut saw_eof = false;
            if pollfd.revents & libc::POLLIN != 0 {
                match read_from(c) {
                    Ok(0) => { saw_eof = true; }
                    Ok(_) => {}
                    Err(e) => {
                        log::warn(format_args!("C{} read error: {e}", c.id));
                        drops.push(i);
                        continue;
                    }
                }
                if let Err(reason) = pump(c, state) {
                    log::warn(format_args!("C{} protocol error: {reason}", c.id));
                    drops.push(i);
                    continue;
                }
            }
            if pollfd.revents & libc::POLLOUT != 0 {
                match write_to(c) {
                    Ok(()) => {}
                    Err(e) => {
                        log::warn(format_args!("C{} write error: {e}", c.id));
                        drops.push(i);
                        continue;
                    }
                }
            }
            // Only drop on HUP after drain.  saw_eof covers the case
            // where the peer closed with no queued data.
            if saw_eof || pollfd.revents & libc::POLLHUP != 0 {
                log::info(format_args!("C{} hangup", c.id));
                drops.push(i);
            }
        }
        // Drop hung-up clients, highest index first.
        for i in drops.into_iter().rev() {
            let c = self.clients.remove(i);
            log::info(format_args!("C{} disconnected", c.id));
        }

        // ── Drain device readers and route to windows ──
        // This happens AFTER the client pump so any state changes
        // made by the client (focus, pointer, grabs) are visible to
        // the router; and BEFORE the pending-events drain so routed
        // events land in the same poll iteration.
        let mut input_events: Vec<InputEvent> = Vec::new();
        if let Some(idx) = mouse_idx {
            if fds[idx].revents & libc::POLLIN != 0 {
                if let Some(m) = &mut self.mouse {
                    m.read_events(&mut input_events);
                }
            }
        }
        if let Some(idx) = kbd_idx {
            if fds[idx].revents & libc::POLLIN != 0 {
                if let Some(k) = &mut self.keyboard {
                    k.read_events(&mut input_events);
                }
            }
        }
        // Fire synthetic --inject events exactly once, when the
        // server has reached a quiescent state: at least one client
        // has dispatched a request (seq > 0), every connected
        // client has an empty read_buf (no more requests pending),
        // and nothing is waiting in the cross-client event queue.
        // This guarantees the test has finished its setup sequence
        // before the injected events route to the tree.
        let ready_to_inject =
            !self.injection_fired
            && !self.pending_inject.is_empty()
            && self.state.inject_armed;
        if ready_to_inject {
            input_events.extend(self.pending_inject.drain(..));
            self.injection_fired = true;
        }
        for ev in input_events {
            route_input_event(&mut self.state, ev);
        }

        // Drain cross-client events staged during dispatch and route
        // them to their target clients.  Each target's `queue_event`
        // stamps the current seq; then we immediately flush so the
        // bytes land in `write_buf` ahead of the next poll — POLLOUT
        // will push them out on the next iteration (or sooner, if the
        // socket was already writable).
        if !self.state.pending_events.is_empty() {
            let pending = core::mem::take(&mut self.state.pending_events);
            for pe in pending {
                if let Some(target) = self.clients.iter_mut()
                    .find(|c| c.id == pe.target_client)
                {
                    target.queue_event(pe.ev);
                    target.flush_events();
                } else {
                    log::warn(format_args!(
                        "cross-client event target C{} not found — dropping",
                        pe.target_client
                    ));
                }
            }
        }

        // Listeners come last — see comment above about index stability.
        if fds[0].revents & libc::POLLIN != 0 {
            self.accept_client();
        }
        if self.abstract_fd.is_some() && fds[1].revents & libc::POLLIN != 0 {
            self.accept_abstract();
        }
        Ok(())
    }

    fn accept_abstract(&mut self) {
        let Some(listen_fd) = self.abstract_fd else { return };
        loop {
            let Some(fd) = accept_raw(listen_fd) else { break };
            let owned = unsafe { OwnedFd::from_raw_fd(fd) };
            let id = self.next_client_id;
            self.next_client_id += 1;
            let client = Client::new(owned, id);
            log::info(format_args!(
                "C{id} accepted (abstract) fd={}",
                client.raw_fd()
            ));
            self.clients.push(client);
        }
    }

    fn accept_client(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((stream, _addr)) => {
                    if let Err(e) = stream.set_nonblocking(true) {
                        log::warn(format_args!("accept: set_nonblocking: {e}"));
                        continue;
                    }
                    let fd = unsafe { OwnedFd::from_raw_fd(stream.into_raw_fd()) };
                    let id = self.next_client_id;
                    self.next_client_id += 1;
                    let client = Client::new(fd, id);
                    log::info(format_args!("C{id} accepted fd={}", client.raw_fd()));
                    self.clients.push(client);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    log::warn(format_args!("accept error: {e}"));
                    break;
                }
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
        if let Some(fd) = self.abstract_fd.take() {
            unsafe { libc::close(fd); }
        }
    }
}

/// Read as much as we can into `c.read_buf`.  Returns number of bytes read,
/// 0 on EOF, or an error.
fn read_from(c: &mut Client) -> Result<usize, std::io::Error> {
    // Re-wrap the fd as a UnixStream for read convenience.  We need to
    // avoid consuming the fd, so use BorrowedFd via std::io::Read on a
    // ManuallyDrop<UnixStream>.
    let fd = c.raw_fd();
    let mut buf = [0u8; 4096];
    let mut total = 0usize;
    loop {
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n < 0 {
            let errno = unsafe { *libc::__errno_location() };
            if errno == libc::EAGAIN || errno == libc::EWOULDBLOCK {
                return Ok(total);
            }
            if errno == libc::EINTR { continue; }
            return Err(std::io::Error::from_raw_os_error(errno));
        }
        if n == 0 {
            return Ok(if total == 0 { 0 } else { total });
        }
        c.read_buf.extend_from_slice(&buf[..n as usize]);
        total += n as usize;
        if (n as usize) < buf.len() { return Ok(total); }
    }
}

/// Flush as much of `c.write_buf` as the socket will accept.
fn write_to(c: &mut Client) -> Result<(), std::io::Error> {
    if c.write_buf.is_empty() { return Ok(()); }
    let fd = c.raw_fd();
    loop {
        if c.write_buf.is_empty() { return Ok(()); }
        let n = unsafe {
            libc::write(
                fd,
                c.write_buf.as_ptr() as *const _,
                c.write_buf.len(),
            )
        };
        if n < 0 {
            let errno = unsafe { *libc::__errno_location() };
            if errno == libc::EAGAIN || errno == libc::EWOULDBLOCK { return Ok(()); }
            if errno == libc::EINTR { continue; }
            return Err(std::io::Error::from_raw_os_error(errno));
        }
        if n == 0 { return Ok(()); }
        c.write_buf.drain(..n as usize);
    }
}

/// Drive the handshake + request loop for a single client until no more
/// progress is possible on the current read_buf contents.
fn pump(c: &mut Client, state: &mut ServerState) -> Result<(), &'static str> {
    loop {
        match c.state {
            HandshakeState::NeedHeader | HandshakeState::NeedAuth { .. } => {
                // Try to parse a complete SetupRequest.  parse_setup_request
                // returns ShortRead when not enough data.
                match setup::parse_setup_request(&c.read_buf) {
                    Ok((req, used)) => {
                        handle_handshake(c, &req);
                        c.read_buf.drain(..used);
                    }
                    Err(SetupError::ShortRead) => return Ok(()),
                    Err(SetupError::BigEndian) => {
                        log::warn(format_args!(
                            "C{} sent big-endian handshake; rejecting",
                            c.id
                        ));
                        let reply = setup::build_failed_reply(
                            11, 0, "big-endian clients not supported"
                        );
                        c.write_buf.extend_from_slice(&reply);
                        c.state = HandshakeState::Failed;
                        return Err("big-endian client");
                    }
                    Err(SetupError::BadProtocolVersion(maj, min)) => {
                        log::warn(format_args!(
                            "C{} requested protocol {maj}.{min}; only 11.0 supported",
                            c.id
                        ));
                        let reply = setup::build_failed_reply(
                            11, 0, "only X11 protocol 11.0 supported"
                        );
                        c.write_buf.extend_from_slice(&reply);
                        c.state = HandshakeState::Failed;
                        return Err("unsupported protocol version");
                    }
                }
            }
            HandshakeState::Established => {
                // Drain as many complete requests as we can.  After each
                // request, any events it produced get pushed onto the
                // outgoing wire buffer in FIFO order.
                match dispatch::dispatch_request(c, state) {
                    DispatchResult::Consumed(_) => {
                        c.flush_events();
                        continue;
                    }
                    DispatchResult::NeedMore    => {
                        c.flush_events();
                        return Ok(());
                    }
                    DispatchResult::Fatal(msg)  => return Err(msg),
                }
            }
            HandshakeState::Failed => return Err("handshake failed"),
        }
    }
}

fn handle_handshake(c: &mut Client, req: &SetupRequest) {
    log::info(format_args!(
        "C{} setup major={} minor={} auth_name={:?} auth_data_len={}",
        c.id,
        req.major,
        req.minor,
        String::from_utf8_lossy(&req.auth_name),
        req.auth_data.len(),
    ));
    // We accept every cookie.  If the client presented MIT-MAGIC-COOKIE-1,
    // log it at trace level so we can see what xauth is passing.
    if !req.auth_name.is_empty() && log::enabled(log::Sev::Trace, Some(c.id), None) {
        log::info(format_args!(
            "C{} auth cookie (first 16 bytes): {:02x?}",
            c.id,
            &req.auth_data[..req.auth_data.len().min(16)],
        ));
    }

    let reply = setup::build_success_reply(c.id, req);
    log::info(format_args!(
        "C{} setup reply {} bytes (expected 8+{}*4)",
        c.id,
        reply.len(),
        (reply.len() - 8) / 4
    ));
    c.write_buf.extend_from_slice(&reply);
    c.state = HandshakeState::Established;
}
