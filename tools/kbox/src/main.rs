// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// kbox — minimal Rust openbox replacement for the Kevlar openbox test.
//
// Mission: satisfy `xprop -root _NET_SUPPORTING_WM_CHECK` against an
// already-running Xorg, and stay alive (so /proc/N/comm shows
// "openbox") for the life of the test.
//
// Why we wrote this rather than fixing real openbox: real openbox
// hangs Xorg in a way that produces ZERO syscalls for 30+ seconds
// (verified via kernel-side strace).  A from-scratch Rust WM whose
// every wire byte we control gives us a byte-precise diff between
// the X11 conversation that works and the one that hangs.  See
// `Documentation/blog/233-...` and the plan at
// `/Users/neo/.claude/plans/ethereal-nibbling-treehouse.md`.

#![deny(unsafe_op_in_unsafe_fn)]
#![allow(clippy::needless_return)]

#[macro_use]
mod log;
mod conn;
mod reply;
mod req;
mod wire;
mod wm;

use std::env;
use std::io;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::reply::{read_frame, Frame};

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_signal(_sig: libc::c_int) {
    SHUTDOWN.store(true, Ordering::Relaxed);
}

fn install_signal_handlers() {
    unsafe {
        let mut sa: libc::sigaction = core::mem::zeroed();
        sa.sa_sigaction = handle_signal as *const () as usize;
        libc::sigaction(libc::SIGTERM, &sa, core::ptr::null_mut());
        libc::sigaction(libc::SIGINT,  &sa, core::ptr::null_mut());
        // Ignore SIGPIPE — we'll detect closed sockets via read/write errors.
        let mut sa_ign: libc::sigaction = core::mem::zeroed();
        sa_ign.sa_sigaction = libc::SIG_IGN;
        libc::sigaction(libc::SIGPIPE, &sa_ign, core::ptr::null_mut());
    }
}

fn parse_display() -> u8 {
    // Args: kbox [DISPLAY], else $DISPLAY else ":0".
    let args: Vec<String> = env::args().collect();
    let s = if args.len() >= 2 && !args[1].starts_with("--") {
        args[1].clone()
    } else {
        env::var("DISPLAY").unwrap_or_else(|_| ":0".into())
    };
    // ":0" → 0,  ":3" → 3,  "host:7.1" → 7
    let after_colon = s.split(':').nth(1).unwrap_or("0");
    let display_str = after_colon.split('.').next().unwrap_or("0");
    display_str.parse::<u8>().unwrap_or(0)
}

fn parse_log_level() -> u8 {
    // KBOX_LOG=hex  → enable hex dumps
    // KBOX_LOG=req  → log requests + replies but not hex
    // KBOX_LOG=info → quieter
    match env::var("KBOX_LOG").ok().as_deref() {
        Some("hex")   => log::LVL_HEX,
        Some("rep")   => log::LVL_REP,
        Some("req")   => log::LVL_REQ,
        Some("info")  => log::LVL_INFO,
        Some("warn")  => log::LVL_WARN,
        Some("err")   => log::LVL_ERR,
        Some("fatal") => log::LVL_FATAL,
        _             => log::LVL_REP,  // default: log every request + reply
    }
}

fn print_help() {
    eprintln!(concat!(
        "kbox — minimal Rust X11 window manager (openbox replacement)\n",
        "\n",
        "Usage: kbox [DISPLAY]\n",
        "\n",
        "Environment:\n",
        "  DISPLAY   X server display (default :0)\n",
        "  KBOX_LOG  log level: hex|rep|req|info|warn|err|fatal (default rep)\n",
    ));
}

fn run() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return Ok(());
    }

    log::set_max_level(parse_log_level());
    install_signal_handlers();

    let display = parse_display();
    let phase: u8 = env::var("KBOX_PHASE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    info!("kbox starting on display :{} phase={}", display, phase);

    let mut conn = conn::connect(display)?;
    let _state = wm::become_wm(&mut conn)?;

    // Apply additional phases on top of the WM_S0 + EWMH baseline.
    // Each phase emits more openbox-like X11 traffic; the bisect
    // strategy is to keep advancing KBOX_PHASE until the openbox
    // test starts failing — that phase's last X11 request is the
    // kernel-bug trigger.  See plan
    // /Users/neo/.claude/plans/ethereal-nibbling-treehouse.md.
    if phase >= 1 { wm::phase1_substructure_redirect(&mut conn)?; }
    if phase >= 2 { wm::phase2_query_tree(&mut conn)?; }
    if phase >= 3 { wm::phase3_root_properties(&mut conn)?; }
    if phase >= 4 { wm::phase4_keyboard_grabs(&mut conn)?; }
    if phase >= 5 { wm::phase5_button_grabs(&mut conn)?; }
    if phase >= 6 { wm::phase6_focus(&mut conn)?; }
    // Phases 7 and 8 are 30-second busy spinners.  They're meant
    // to be run *terminally* (KBOX_PHASE=7 or 8 exactly), not as
    // a setup step before higher phases — otherwise phase 9+
    // wouldn't run until 30/60s in.  Higher phases skip them.
    if phase == 7 { wm::phase7_event_loop(&mut conn)?; }
    if phase == 8 { wm::phase8_async_storm(&mut conn)?; }
    if phase >= 9 { wm::phase9_query_extensions(&mut conn)?; }
    if phase >= 10 { wm::phase10_xkb(&mut conn)?; }
    if phase >= 11 { wm::phase11_mit_shm(&mut conn)?; }
    if phase >= 12 { wm::phase12_resource_cascade(&mut conn)?; }
    // Phase 13 is a 30-second spinner; phase 14 is also a 30-
    // second spinner.  Neither composes with later phases — pick
    // one or the other terminally.
    if phase == 13 { wm::phase13_libxcb_burst(&mut conn)?; }
    if phase == 14 { wm::phase14_threaded_burst(&mut conn)?; }
    if phase == 15 { wm::phase15_kitchen_sink(&mut conn)?; }
    if phase == 16 { wm::phase16_triple_fs(&mut conn)?; }
    if phase == 17 { wm::phase17_replay_openbox_trigger(&mut conn)?; }
    if phase == 18 { wm::phase18_cursor_change(&mut conn)?; }
    if phase == 19 { wm::phase19_minimal_cursor_trigger(&mut conn)?; }

    info!("entering idle loop; will exit on SIGTERM/SIGINT or X11 disconnect");

    // Drain incoming frames forever so the per-client output buffer
    // on the server side doesn't backfill.  Most arrivals will be
    // events (none subscribed), errors (logged), or a disconnect.
    //
    // Use a poll() with a 250ms timeout so we periodically check
    // SHUTDOWN even when Xorg is silent.
    let fd = conn.fd();
    loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            info!("shutdown requested, exiting cleanly");
            break;
        }
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut pfd, 1, 250) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) { continue; }
            return Err(err);
        }
        if rc == 0 { continue; }
        if pfd.revents & libc::POLLIN != 0 {
            // Drain at least one frame.  read_frame blocks until a
            // full 32-byte frame is available, which we know is
            // imminent because POLLIN fired.
            match read_frame(&mut conn) {
                Ok(Frame::Error { .. }) | Ok(Frame::Reply { .. }) | Ok(Frame::Event { .. }) => {
                    // Already logged by reply.rs.
                }
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    info!("Xorg closed the connection, exiting");
                    break;
                }
                Err(e) => {
                    err_!("read error: {} ({:?}); exiting", e, e.kind());
                    return Err(e);
                }
            }
        }
        if pfd.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
            info!("Xorg hangup (revents={:#x}), exiting", pfd.revents);
            break;
        }
        // Avoid 100% CPU even if poll() lies — sleep a frame.
        std::thread::sleep(Duration::from_millis(1));
    }

    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kbox: fatal: {}", e);
            ExitCode::from(1)
        }
    }
}
