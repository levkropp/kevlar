// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! epoll(7) — I/O event notification facility.
//!
//! Provenance: Own (Linux epoll(7), epoll_create(2), epoll_ctl(2), epoll_wait(2) man pages).
//!
//! This is a simple level-triggered implementation that reuses the existing
//! POLL_WAIT_QUEUE.  On each wakeup, epoll_wait re-polls interested fds to
//! check readiness.  This is O(n) per wakeup where n = number of interests,
//! but correct and sufficient for systemd's ~10-fd event loop.
use core::fmt;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::fs::inode::{FileLike, PollStatus};
use crate::fs::opened_file::Fd;
use crate::prelude::*;
use kevlar_platform::spinlock::SpinLock;
use kevlar_vfs::stat::Stat;

// ── Linux epoll constants ───────────────────────────────────────────

pub const EPOLL_CTL_ADD: i32 = 1;
pub const EPOLL_CTL_DEL: i32 = 2;
pub const EPOLL_CTL_MOD: i32 = 3;

pub const EPOLL_CLOEXEC: i32 = 0o2000000; // == O_CLOEXEC

// EPOLLIN/EPOLLOUT match POLLIN/POLLOUT values.
const EPOLLIN: u32  = 0x001;
const EPOLLOUT: u32 = 0x004;
const EPOLLERR: u32 = 0x008;
const EPOLLHUP: u32 = 0x010;

// ── epoll_event (matches Linux struct epoll_event) ──────────────────

/// Packed to match Linux's `__attribute__((packed))` on x86_64.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct EpollEvent {
    pub events: u32,
    pub data: u64,
}

// ── Interest entry ──────────────────────────────────────────────────

struct Interest {
    /// The file description being watched (keep alive via Arc).
    file: Arc<dyn FileLike>,
    /// Events mask requested by the user (EPOLLIN, EPOLLOUT, etc.).
    events: u32,
    /// Opaque user data returned in epoll_wait results.
    data: u64,
}

// ── EpollInstance ───────────────────────────────────────────────────

pub struct EpollInstance {
    interests: SpinLock<BTreeMap<i32, Interest>>,
}

impl EpollInstance {
    pub fn new() -> Arc<EpollInstance> {
        Arc::new(EpollInstance {
            interests: SpinLock::new(BTreeMap::new()),
        })
    }

    /// `epoll_ctl(EPOLL_CTL_ADD)` — register a new fd.
    pub fn add(&self, fd: Fd, file: Arc<dyn FileLike>, event: &EpollEvent) -> Result<()> {
        let mut interests = self.interests.lock();
        if interests.contains_key(&fd.as_int()) {
            return Err(Errno::EEXIST.into());
        }
        interests.insert(fd.as_int(), Interest {
            file,
            events: event.events,
            data: event.data,
        });
        Ok(())
    }

    /// `epoll_ctl(EPOLL_CTL_MOD)` — modify an existing registration.
    pub fn modify(&self, fd: Fd, event: &EpollEvent) -> Result<()> {
        let mut interests = self.interests.lock();
        let entry = interests.get_mut(&fd.as_int())
            .ok_or(Error::new(Errno::ENOENT))?;
        entry.events = event.events;
        entry.data = event.data;
        Ok(())
    }

    /// `epoll_ctl(EPOLL_CTL_DEL)` — remove a registration.
    pub fn delete(&self, fd: Fd) -> Result<()> {
        let mut interests = self.interests.lock();
        if interests.remove(&fd.as_int()).is_none() {
            return Err(Errno::ENOENT.into());
        }
        Ok(())
    }

    /// Poll all interested fds and return ready events.
    /// Returns the number of ready fds (may be 0 if nothing is ready).
    pub fn collect_ready(&self, out: &mut Vec<EpollEvent>, max: usize) -> usize {
        let interests = self.interests.lock_no_irq();
        let mut count = 0;
        for interest in interests.values() {
            if count >= max {
                break;
            }
            let status = match interest.file.poll() {
                Ok(s) => s,
                Err(_) => PollStatus::POLLERR,
            };
            let ready = poll_status_to_epoll(status) & (interest.events | EPOLLERR | EPOLLHUP);
            if ready != 0 {
                out.push(EpollEvent {
                    events: ready,
                    data: interest.data,
                });
                count += 1;
            }
        }
        count
    }

    /// Poll all interested fds and write ready events directly to userspace.
    /// Avoids heap allocation — used by the non-blocking (timeout=0) fast path.
    pub fn collect_ready_to_user(
        &self,
        events_ptr: kevlar_platform::address::UserVAddr,
        max: usize,
    ) -> crate::result::Result<usize> {
        let interests = self.interests.lock_no_irq();
        let mut count = 0;
        for interest in interests.values() {
            if count >= max {
                break;
            }
            let status = match interest.file.poll() {
                Ok(s) => s,
                Err(_) => PollStatus::POLLERR,
            };
            let ready = poll_status_to_epoll(status) & (interest.events | EPOLLERR | EPOLLHUP);
            if ready != 0 {
                let event = EpollEvent { events: ready, data: interest.data };
                let dest = events_ptr.add(count * 12); // EPOLL_EVENT_SIZE = 12
                let mut buf = [0u8; 12];
                buf[0..4].copy_from_slice(&event.events.to_ne_bytes());
                buf[4..12].copy_from_slice(&event.data.to_ne_bytes());
                dest.write_bytes(&buf)?;
                count += 1;
            }
        }
        Ok(count)
    }
}

impl fmt::Debug for EpollInstance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EpollInstance").finish()
    }
}

/// Convert PollStatus flags to epoll event bits.
/// They happen to share the same bit positions for IN/OUT/ERR/HUP.
fn poll_status_to_epoll(status: PollStatus) -> u32 {
    let mut bits = 0u32;
    if status.contains(PollStatus::POLLIN)  { bits |= EPOLLIN; }
    if status.contains(PollStatus::POLLOUT) { bits |= EPOLLOUT; }
    if status.contains(PollStatus::POLLERR) { bits |= EPOLLERR; }
    if status.contains(PollStatus::POLLHUP) { bits |= EPOLLHUP; }
    bits
}

// ── FileLike implementation (so epoll fd can be in the fd table) ────

impl FileLike for EpollInstance {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat::zeroed())
    }

    fn poll(&self) -> Result<PollStatus> {
        // An epoll fd is itself pollable (for nested epoll).
        // It's readable when any of its interests have ready events.
        let mut events = Vec::new();
        let count = self.collect_ready(&mut events, 1);
        if count > 0 {
            Ok(PollStatus::POLLIN)
        } else {
            Ok(PollStatus::empty())
        }
    }
}
