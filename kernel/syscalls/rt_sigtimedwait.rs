// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! rt_sigtimedwait(2) — synchronously wait for a pending signal.
//!
//! Provenance: Own (Linux rt_sigtimedwait(2) man page).
use crate::{
    ctypes::{c_long, c_time},
    prelude::*,
    process::{current_process, signal::Signal},
    syscalls::SyscallHandler,
    timer::read_monotonic_clock,
};
use kevlar_platform::address::UserVAddr;

#[repr(C)]
struct Timespec {
    tv_sec: c_time,
    tv_nsec: c_long,
}

/// Minimal siginfo_t layout written to userspace (128 bytes on Linux x86_64).
/// We only fill si_signo, si_errno, si_code; rest is zeroed.
const SIGINFO_SIZE: usize = 128;

impl<'a> SyscallHandler<'a> {
    /// `rt_sigtimedwait(set, info, timeout, sigsetsize)`
    ///
    /// Wait for one of the signals in `set` to become pending. If a matching
    /// signal is already pending, dequeue it immediately. Otherwise block until
    /// a signal arrives or the timeout expires.
    pub fn sys_rt_sigtimedwait(
        &mut self,
        set_ptr: UserVAddr,
        info_ptr: Option<UserVAddr>,
        timeout_ptr: Option<UserVAddr>,
        sigsetsize: usize,
    ) -> Result<isize> {
        // Read the signal set from userspace.
        let set_bytes = if sigsetsize >= 8 {
            set_ptr.read::<[u8; 8]>()?
        } else {
            let mut b = [0u8; 8];
            let raw = set_ptr.read::<[u8; 4]>()?;
            b[..4].copy_from_slice(&raw);
            b
        };
        let mask = u64::from_le_bytes(set_bytes);

        // Parse timeout.
        let deadline_ns: Option<u64> = if let Some(tp) = timeout_ptr {
            let ts = tp.read::<Timespec>()?;
            if ts.tv_sec < 0 || ts.tv_nsec < 0 || ts.tv_nsec >= 1_000_000_000 {
                return Err(Errno::EINVAL.into());
            }
            if ts.tv_sec == 0 && ts.tv_nsec == 0 {
                // Zero timeout = poll only, don't block.
                None
            } else {
                let now = read_monotonic_clock().nanosecs() as u64;
                let delta = (ts.tv_sec as u64) * 1_000_000_000 + (ts.tv_nsec as u64);
                Some(now + delta)
            }
        } else {
            // No timeout pointer = block indefinitely.
            Some(u64::MAX)
        };

        // Fast path: check if a matching signal is already pending.
        if let Some(sig) = try_dequeue_signal(mask) {
            write_siginfo(info_ptr, sig)?;
            return Ok(sig as isize);
        }

        // Zero timeout (or tv_sec=0,tv_nsec=0): return EAGAIN immediately.
        let deadline = match deadline_ns {
            Some(d) => d,
            None => return Err(Errno::EAGAIN.into()),
        };

        // Sleep path: wait for a matching signal or timeout.
        let result: core::result::Result<Signal, Error> =
            crate::poll::POLL_WAIT_QUEUE.sleep_signalable_until(|| {
                // Check for matching pending signal.
                if let Some(sig) = try_dequeue_signal(mask) {
                    return Ok(Some(sig));
                }

                // Check timeout.
                if deadline != u64::MAX {
                    let now = read_monotonic_clock().nanosecs() as u64;
                    if now >= deadline {
                        // Timeout expired — we need to signal this as EAGAIN.
                        // Return None to keep looping isn't right; we need to break.
                        // Use a special signal value to indicate timeout.
                        return Ok(Some(0));
                    }
                }

                Ok(None)
            });

        match result {
            Ok(0) => {
                // Timeout.
                Err(Errno::EAGAIN.into())
            }
            Ok(sig) => {
                write_siginfo(info_ptr, sig)?;
                Ok(sig as isize)
            }
            Err(e) => {
                // Interrupted by a signal not in our set.
                // Check one more time if a matching signal arrived.
                if let Some(sig) = try_dequeue_signal(mask) {
                    write_siginfo(info_ptr, sig)?;
                    return Ok(sig as isize);
                }
                Err(e)
            }
        }
    }
}

/// Try to dequeue a signal matching `mask` from the current process.
fn try_dequeue_signal(mask: u64) -> Option<Signal> {
    let current = current_process();
    let mut signals = current.signals().lock();
    signals.pop_pending_masked(mask)
}

/// Write a minimal siginfo_t to userspace.
fn write_siginfo(info_ptr: Option<UserVAddr>, sig: Signal) -> Result<()> {
    if let Some(ptr) = info_ptr {
        let mut buf = [0u8; SIGINFO_SIZE];
        // si_signo at offset 0 (i32)
        buf[0..4].copy_from_slice(&(sig as i32).to_ne_bytes());
        // si_errno at offset 4 (i32) = 0
        // si_code at offset 8 (i32) = SI_USER (0)
        ptr.write_bytes(&buf)?;
    }
    Ok(())
}
