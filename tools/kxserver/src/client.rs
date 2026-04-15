// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(dead_code)]
//
// Per-client state.
//
// Each connected client has:
//   - a socket fd
//   - a monotonically increasing sequence number (the server's view of the
//     last-processed request; inserted into every reply/event)
//   - read and write byte buffers (we do non-blocking I/O and may need to
//     queue data until the socket drains)
//   - a handshake state (before/after ConnectionSetup is exchanged)
//   - a client id (1-based, assigned on accept).  This becomes the high
//     bits of every XID the client allocates.

use std::os::fd::{AsRawFd, OwnedFd, RawFd};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeState {
    /// Waiting for the 12-byte setup header.
    NeedHeader,
    /// Header received; waiting for auth-name + auth-data.
    NeedAuth { name_len: u16, data_len: u16 },
    /// Setup complete; handling normal requests.
    Established,
    /// Setup failed; socket will be closed.
    Failed,
}

pub struct Client {
    pub fd: OwnedFd,
    pub id: u32,
    pub seq: u16,
    pub read_buf:  Vec<u8>,
    pub write_buf: Vec<u8>,
    pub state: HandshakeState,
    /// Whether we've already logged an unhandled-opcode error for the
    /// current client (once per opcode, to avoid log floods when a client
    /// retries the same request).
    pub logged_unhandled: [bool; 256],
    /// Queued events (each 32 bytes) waiting to be flushed to the wire
    /// at the next opportunity.  Events carry the sequence number of
    /// the LAST REQUEST PROCESSED on this client; we store that value
    /// in bytes 2..=3 at enqueue time.
    pub event_queue: Vec<[u8; 32]>,
}

impl Client {
    pub fn new(fd: OwnedFd, id: u32) -> Self {
        Client {
            fd,
            id,
            seq: 0,
            read_buf:  Vec::with_capacity(4096),
            write_buf: Vec::with_capacity(4096),
            state: HandshakeState::NeedHeader,
            logged_unhandled: [false; 256],
            event_queue: Vec::new(),
        }
    }

    /// Push a 32-byte event onto this client's queue.  The event's
    /// `sequence_number` field (bytes 2..=3) is overwritten with the
    /// current value of `self.seq` — events always carry the sequence
    /// of the last request processed, not the opcode that triggered
    /// them.
    pub fn queue_event(&mut self, mut ev: [u8; 32]) {
        let seq_bytes = self.seq.to_le_bytes();
        ev[2] = seq_bytes[0];
        ev[3] = seq_bytes[1];
        self.event_queue.push(ev);
    }

    /// Move all queued events into the outgoing write buffer.  Called
    /// at the end of each `pump()` iteration so events flow out in
    /// predictable FIFO order.
    pub fn flush_events(&mut self) {
        for ev in self.event_queue.drain(..) {
            self.write_buf.extend_from_slice(&ev);
        }
    }

    pub fn raw_fd(&self) -> RawFd { self.fd.as_raw_fd() }

    pub fn next_seq(&mut self) -> u16 {
        // X11 sequence numbers increment monotonically per request processed.
        // We overflow naturally at u16::MAX.
        self.seq = self.seq.wrapping_add(1);
        self.seq
    }
}
