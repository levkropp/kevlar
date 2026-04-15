// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]  // phase-0 scaffolding; consumed in phase 1+
//
// Diagnostic logging.
//
// Every X11 request, reply, event, and error flows through this module.
// Output is a single line per entry, greppable, with a consistent tag:
//
//     [C1 #0042] REQ  op=53 CreatePixmap   pid=0x200000 drw=0x100 w=80 h=24
//     [C1 #0042] RAW  0035 0004 0000 0020 0000 0001 0050 0018 18
//     [C1 #0042] REP  ok
//
// The filter lets us focus on a specific opcode, client, or severity without
// drowning in output. Every later phase extends this — `log` is built first
// on purpose.

use std::fmt::Display;
use std::io::{self, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Copy, Clone, Eq, PartialEq, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum Sev {
    Trace = 0,
    Req   = 1,
    Rep   = 2,
    Evt   = 3,
    Err   = 4,
    Warn  = 5,
    Fatal = 6,
}

impl Sev {
    fn tag(self) -> &'static str {
        match self {
            Sev::Trace => "TRC",
            Sev::Req   => "REQ",
            Sev::Rep   => "REP",
            Sev::Evt   => "EVT",
            Sev::Err   => "ERR",
            Sev::Warn  => "WRN",
            Sev::Fatal => "FTL",
        }
    }
}

/// 256-bit set indexed by X11 opcode.
#[derive(Clone, Debug)]
pub struct OpSet([u64; 4]);

impl OpSet {
    pub const fn none() -> Self { OpSet([0; 4]) }
    pub const fn all()  -> Self { OpSet([!0; 4]) }

    pub fn insert(&mut self, op: u8) {
        let (w, b) = ((op >> 6) as usize, (op & 63) as usize);
        self.0[w] |= 1u64 << b;
    }

    pub fn contains(&self, op: u8) -> bool {
        let (w, b) = ((op >> 6) as usize, (op & 63) as usize);
        (self.0[w] & (1u64 << b)) != 0
    }

    pub fn is_all(&self) -> bool {
        self.0 == [!0u64; 4]
    }
}

#[derive(Clone, Debug)]
pub struct Filter {
    pub min_sev: Sev,
    pub opcodes: OpSet,
    /// None means "all clients".  Some(vec) means only these client ids.
    pub clients: Option<Vec<u32>>,
}

impl Filter {
    pub const fn default_trace() -> Self {
        Filter {
            min_sev: Sev::Trace,
            opcodes: OpSet::all(),
            clients: None,
        }
    }

    pub fn allows(&self, sev: Sev, client: Option<u32>, opcode: Option<u8>) -> bool {
        if sev < self.min_sev { return false; }
        if let Some(c) = client {
            if let Some(list) = &self.clients {
                if !list.contains(&c) { return false; }
            }
        }
        if let Some(op) = opcode {
            if !self.opcodes.contains(op) { return false; }
        }
        true
    }
}

struct LogState {
    filter: Filter,
    out: Box<dyn Write + Send>,
    dump: Option<Box<dyn Write + Send>>,
}

// Global logger, guarded by Mutex.  Initialized once via `init()`.
static INIT: AtomicU8 = AtomicU8::new(0);
static STATE: Mutex<Option<LogState>> = Mutex::new(None);

/// Initialize the global logger.  Safe to call once; subsequent calls are
/// no-ops so re-entry (e.g. from a panic hook) does not deadlock.
pub fn init(filter: Filter, dump_path: Option<&str>) {
    if INIT.swap(1, Ordering::AcqRel) == 1 {
        return;
    }
    let out: Box<dyn Write + Send> = Box::new(io::stderr());
    let dump: Option<Box<dyn Write + Send>> = dump_path.and_then(|p| {
        match std::fs::OpenOptions::new().create(true).append(true).open(p) {
            Ok(f) => Some(Box::new(f) as Box<dyn Write + Send>),
            Err(e) => {
                eprintln!("[kxserver] log: cannot open dump file {p}: {e}");
                None
            }
        }
    });
    *STATE.lock().unwrap() = Some(LogState { filter, out, dump });
}

fn with_state<F: FnOnce(&mut LogState)>(f: F) {
    if let Ok(mut guard) = STATE.lock() {
        if let Some(state) = guard.as_mut() {
            f(state);
        }
    }
}

/// Return true if an entry with this severity + client + opcode would be logged.
pub fn enabled(sev: Sev, client: Option<u32>, opcode: Option<u8>) -> bool {
    let guard = match STATE.lock() { Ok(g) => g, Err(_) => return false };
    match guard.as_ref() {
        Some(state) => state.filter.allows(sev, client, opcode),
        None => false,
    }
}

fn prefix(client: Option<u32>, seq: Option<u16>, sev: Sev) -> String {
    match (client, seq) {
        (Some(c), Some(s)) => format!("[C{c} #{s:04}] {} ", sev.tag()),
        (Some(c), None)    => format!("[C{c}]        {} ", sev.tag()),
        (None,    Some(s)) => format!("[-- #{s:04}] {} ", sev.tag()),
        (None,    None)    => format!("[--]         {} ", sev.tag()),
    }
}

/// Low-level: write a line with a given prefix.  Used by the helpers below.
fn emit(sev: Sev, client: Option<u32>, seq: Option<u16>, opcode: Option<u8>, body: &str) {
    with_state(|state| {
        if !state.filter.allows(sev, client, opcode) { return; }
        let line = format!("{}{}\n", prefix(client, seq, sev), body);
        let _ = state.out.write_all(line.as_bytes());
        // We do not flush on every line; stderr is line-buffered when attached
        // to a terminal, and fully buffered otherwise.  Fatal errors flush.
        if sev >= Sev::Err {
            let _ = state.out.flush();
        }
    });
}

/// Record a request entry.  `decoded` is rendered via Display; `raw` is also
/// written as a RAW hex line when trace-level is enabled.
pub fn req(client: u32, seq: u16, opcode: u8, name: &str, decoded: impl Display, raw: &[u8]) {
    if !enabled(Sev::Req, Some(client), Some(opcode)) { return; }
    emit(
        Sev::Req,
        Some(client),
        Some(seq),
        Some(opcode),
        &format!("op={opcode:<3} {name:<20} {decoded}"),
    );
    if enabled(Sev::Trace, Some(client), Some(opcode)) {
        emit_hex(Some(client), Some(seq), Sev::Trace, "RAW ", raw);
    }
    dump_bytes(raw);
}

/// Record a reply entry.
pub fn rep(client: u32, seq: u16, decoded: impl Display, raw: &[u8]) {
    if !enabled(Sev::Rep, Some(client), None) { return; }
    emit(Sev::Rep, Some(client), Some(seq), None, &format!("ok {decoded}"));
    if enabled(Sev::Trace, Some(client), None) && !raw.is_empty() {
        emit_hex(Some(client), Some(seq), Sev::Trace, "RAW ", raw);
    }
    dump_bytes(raw);
}

/// Record an outgoing event.
pub fn evt(client: u32, evtype: u8, name: &str, target: u32, decoded: impl Display) {
    if !enabled(Sev::Evt, Some(client), None) { return; }
    emit(
        Sev::Evt,
        Some(client),
        None,
        None,
        &format!("ev={evtype:<3} {name:<18} target=0x{target:08x} {decoded}"),
    );
}

/// Record a protocol error.
pub fn err(client: u32, seq: u16, code: u8, bad: u32, reason: &str) {
    emit(
        Sev::Err,
        Some(client),
        Some(seq),
        None,
        &format!("code={code} bad=0x{bad:08x} {reason}"),
    );
}

pub fn warn(fmt: impl Display) {
    emit(Sev::Warn, None, None, None, &format!("{fmt}"));
}

pub fn info(fmt: impl Display) {
    emit(Sev::Req, None, None, None, &format!("{fmt}"));
}

pub fn fatal(fmt: impl Display) {
    emit(Sev::Fatal, None, None, None, &format!("{fmt}"));
}

/// Dump raw bytes as 16-column hex, one line per 16 bytes, with the given tag.
fn emit_hex(client: Option<u32>, seq: Option<u16>, sev: Sev, tag: &str, bytes: &[u8]) {
    for (i, chunk) in bytes.chunks(16).enumerate() {
        let mut line = format!("{tag}{:04x}  ", i * 16);
        for b in chunk {
            line.push_str(&format!("{b:02x} "));
        }
        emit(sev, client, seq, None, &line);
    }
}

/// Append the raw bytes to the optional dump file (if configured).
fn dump_bytes(raw: &[u8]) {
    with_state(|state| {
        if let Some(dump) = state.dump.as_mut() {
            let _ = dump.write_all(raw);
        }
    });
}
