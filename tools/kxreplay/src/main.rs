// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//
// kxreplay — replay a captured openbox→Xorg C2S byte stream
// verbatim against /tmp/.X11-unix/X0, then sit idle so the test's
// xprop probe lands.  See blog 240 / task #41.
//
// The trace is embedded at build time from `trace.log` (a kxproxy
// hex dump), parsed by build.rs into a packed binary blob:
//   for each C2S chunk: u32 little-endian length, then `length` bytes.
// We `include_bytes!` that blob and walk it at runtime.
//
// If kxreplay reproduces the same xprop hang real openbox produces,
// the trigger is fully in the bytes and we have a 100% in-tree
// repro that doesn't need openbox installed at all.  If it doesn't,
// the trigger is in some non-byte state (per-process FD count,
// signal masks, exact byte-arrival timing) that a script can't
// match.

#![deny(unsafe_op_in_unsafe_fn)]

use std::env;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const TRACE_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/trace.bin"));

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
        let mut sa_ign: libc::sigaction = core::mem::zeroed();
        sa_ign.sa_sigaction = libc::SIG_IGN;
        libc::sigaction(libc::SIGPIPE, &sa_ign, core::ptr::null_mut());
    }
}

/// Walk the embedded blob, returning the next chunk (or None when done).
struct ChunkIter<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for ChunkIter<'a> {
    type Item = &'a [u8];
    fn next(&mut self) -> Option<&'a [u8]> {
        if self.rest.len() < 4 { return None; }
        let len = u32::from_le_bytes([self.rest[0], self.rest[1], self.rest[2], self.rest[3]]) as usize;
        if self.rest.len() < 4 + len { return None; }
        let chunk = &self.rest[4..4 + len];
        self.rest = &self.rest[4 + len..];
        Some(chunk)
    }
}

fn parse_display() -> u8 {
    let args: Vec<String> = env::args().collect();
    let s = if args.len() >= 2 && !args[1].starts_with("--") {
        args[1].clone()
    } else {
        env::var("DISPLAY").unwrap_or_else(|_| ":0".into())
    };
    let after_colon = s.split(':').nth(1).unwrap_or("0");
    let display_str = after_colon.split('.').next().unwrap_or("0");
    display_str.parse::<u8>().unwrap_or(0)
}

fn run() -> std::io::Result<()> {
    install_signal_handlers();

    let display = parse_display();
    let path = format!("/tmp/.X11-unix/X{}", display);
    eprintln!("kxreplay: connecting to {}", path);
    let mut sock = UnixStream::connect(&path)?;
    eprintln!("kxreplay: trace blob is {} bytes", TRACE_BLOB.len());

    // Spawn a draining thread for S2C so the server's send buffer
    // doesn't backfill (which would block the server's writev and
    // mask any actual hang we're trying to reproduce).
    let read_sock = sock.try_clone()?;
    let drained = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let dr = drained.clone();
    let drainer = std::thread::spawn(move || {
        let mut sock = read_sock;
        let mut buf = [0u8; 8192];
        loop {
            match sock.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    dr.fetch_add(n as u64, Ordering::Relaxed);
                }
                Err(_) => break,
            }
        }
    });

    // Walk the embedded chunks and write each verbatim.
    // KXREPLAY_LIMIT env var: stop after this many chunks (default
    // = unlimited).  Used to bisect which chunks are essential.
    let limit: usize = env::var("KXREPLAY_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(usize::MAX);
    let skip: usize = env::var("KXREPLAY_SKIP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    eprintln!("kxreplay: limit={} skip={}", limit, skip);

    // KXREPLAY_TAIL_BYTES: if set, only write the first N bytes of
    // the LAST chunk (chunk index `limit-1` after skip).  Used to
    // bisect within a specific chunk byte-by-byte.
    let tail_bytes: Option<usize> = env::var("KXREPLAY_TAIL_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n > 0); // tail=0 → no truncation (the test harness defaults this to "0")

    // KXREPLAY_TAIL_IDX: chunk index to truncate (defaults to the
    // last included chunk).  Lets us truncate a non-final chunk
    // (e.g., the one defining the cursor that the trigger references).
    let tail_idx_env: Option<usize> = env::var("KXREPLAY_TAIL_IDX")
        .ok()
        .and_then(|s| s.parse().ok());

    // KXREPLAY_FORCE_HANDSHAKE: when skip>0, prepend chunk 0 (the
    // connection setup) so the X11 connection is valid even when
    // we skip past the early bytes.  Without this, skipping any
    // chunks invalidates the whole connection.
    let force_handshake: bool = env::var("KXREPLAY_FORCE_HANDSHAKE")
        .ok()
        .map(|s| s != "0")
        .unwrap_or(true); // default-on

    // KXREPLAY_INCLUDE: comma-separated list of chunk indices to
    // write (e.g., "0,131,137").  Overrides skip/limit when set.
    // Used for non-contiguous chunk ablation.
    let include_list: Option<Vec<usize>> = env::var("KXREPLAY_INCLUDE")
        .ok()
        .map(|s| s.split(',')
                  .filter_map(|t| t.trim().parse::<usize>().ok())
                  .collect::<Vec<_>>())
        .filter(|v: &Vec<usize>| !v.is_empty());
    if let Some(v) = &include_list {
        eprintln!("kxreplay: INCLUDE list = {:?}", v);
    }
    let need_force = force_handshake && match &include_list {
        Some(v) => !v.contains(&0),
        None => skip > 0,
    };
    if need_force {
        // Find chunk 0 and write it.
        if let Some(first) = (ChunkIter { rest: TRACE_BLOB }).next() {
            sock.write_all(first)?;
            eprintln!("kxreplay: force-prepended chunk 0 ({} bytes) for handshake",
                      first.len());
        }
    }

    let mut chunk_count = 0u64;
    let mut total_written = 0u64;
    let iter = ChunkIter { rest: TRACE_BLOB };
    let last_idx_for_tail: Option<usize> = tail_idx_env.or_else(||
        include_list.as_ref()
            .and_then(|v| v.last().copied())
            .or_else(|| if limit == usize::MAX { None } else { Some(skip + limit - 1) })
    );
    for (idx, chunk) in iter.enumerate() {
        if SHUTDOWN.load(Ordering::Relaxed) { break; }
        let included = match &include_list {
            Some(v) => v.contains(&idx),
            None => idx >= skip && (idx - skip) < limit,
        };
        if !included { continue; }
        let to_write = if Some(idx) == last_idx_for_tail {
            match tail_bytes {
                Some(n) => &chunk[..n.min(chunk.len())],
                None => chunk,
            }
        } else {
            chunk
        };
        sock.write_all(to_write)?;
        chunk_count += 1;
        total_written += to_write.len() as u64;
        if chunk_count % 8 == 0 {
            std::thread::sleep(Duration::from_micros(200));
        }
    }
    eprintln!("kxreplay: wrote {} chunks, {} total bytes; entering idle",
              chunk_count, total_written);

    // Sit idle so the test's xprop probe lands while we're still
    // connected (and the server is in whatever state we put it in).
    while !SHUTDOWN.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = sock.shutdown(std::net::Shutdown::Both);
    let _ = drainer.join();
    eprintln!("kxreplay: drained {} bytes from server",
              drained.load(Ordering::Relaxed));
    Ok(())
}

fn main() -> ExitCode {
    if TRACE_BLOB.is_empty() {
        eprintln!("kxreplay: trace.log was empty at build time; rebuild with a captured trace");
        return ExitCode::from(1);
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kxreplay: fatal: {}", e);
            ExitCode::from(1)
        }
    }
}
