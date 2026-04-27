// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! signalfd(2) — Deliver signals as readable fd events.
//!
//! Provenance: Own (Linux signalfd(2) man page).
//!
//! Design: the signalfd does NOT intercept signal delivery.  Instead, the user
//! blocks the desired signals via sigprocmask, and the signalfd's read/poll
//! methods check the process's pending signal set for matching blocked signals.
//! This requires no modifications to the signal delivery path.
use core::fmt;

use crate::fs::inode::{FileLike, PollStatus};
use crate::poll::POLL_WAIT_QUEUE;
use crate::prelude::*;
use crate::process::current_process;
use crate::user_buffer::{UserBufWriter, UserBufferMut};
use kevlar_vfs::inode::OpenOptions;
use kevlar_vfs::stat::Stat;

// ── Linux signalfd constants ───────────────────────────────────────

pub const SFD_CLOEXEC: i32 = 0o2000000;
pub const SFD_NONBLOCK: i32 = 0o4000;

/// Size of `struct signalfd_siginfo` (128 bytes).
const SIGINFO_SIZE: usize = 128;

// ── SignalFd ───────────────────────────────────────────────────────

pub struct SignalFd {
    /// Bitmask of signals this fd watches (matches the sigset_t passed to
    /// signalfd4).  Only signals in this mask will be dequeued on read.
    /// u64 to cover the full Linux signal range (1..=64) including RT
    /// signals — see signal.rs SIGMAX comment.
    mask: u64,
}

impl SignalFd {
    pub fn new(mask: u64) -> Arc<SignalFd> {
        Arc::new(SignalFd { mask })
    }
}

impl fmt::Debug for SignalFd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignalFd").finish()
    }
}

/// Build a minimal signalfd_siginfo struct (128 bytes, mostly zeroed).
/// Only the essential fields are populated.
fn make_siginfo(signal: i32) -> [u8; SIGINFO_SIZE] {
    let mut buf = [0u8; SIGINFO_SIZE];
    // ssi_signo at offset 0 (u32)
    buf[0..4].copy_from_slice(&(signal as u32).to_ne_bytes());
    // ssi_code at offset 8 (i32) — SI_USER = 0
    // ssi_pid at offset 12 (u32) — 0 (kernel-generated)
    // All other fields zeroed.
    buf
}

impl FileLike for SignalFd {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat::zeroed())
    }

    fn read(
        &self,
        _offset: usize,
        buf: UserBufferMut<'_>,
        options: &OpenOptions,
    ) -> Result<usize> {
        if buf.len() < SIGINFO_SIZE {
            return Err(Errno::EINVAL.into());
        }
        let max_signals = buf.len() / SIGINFO_SIZE;
        let mut writer = UserBufWriter::from(buf);

        // Try dequeuing matching pending signals.
        {
            let current = current_process();
            let mut sigs = current.signals().lock();
            while writer.written_len() / SIGINFO_SIZE < max_signals {
                if let Some(signal) = sigs.pop_pending_masked(self.mask) {
                    writer.write_bytes(&make_siginfo(signal))?;
                } else {
                    break;
                }
            }
            current.sync_signal_pending(sigs.pending_bits());
        }

        if writer.written_len() > 0 {
            return Ok(writer.written_len());
        }

        if options.nonblock {
            return Err(Errno::EAGAIN.into());
        }

        // Block until a matching signal arrives.
        POLL_WAIT_QUEUE.sleep_signalable_until(|| {
            let current = current_process();
            let mut sigs = current.signals().lock();
            if let Some(signal) = sigs.pop_pending_masked(self.mask) {
                current.sync_signal_pending(sigs.pending_bits());
                writer.write_bytes(&make_siginfo(signal))?;
                Ok(Some(writer.written_len()))
            } else {
                Ok(None)
            }
        })
    }

    fn poll(&self) -> Result<PollStatus> {
        let pending = current_process().signal_pending_bits();
        if pending & self.mask != 0 {
            Ok(PollStatus::POLLIN)
        } else {
            Ok(PollStatus::empty())
        }
    }
}
