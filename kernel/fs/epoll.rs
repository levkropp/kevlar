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
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

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
const EPOLLET: u32  = 1 << 31;
const EPOLLONESHOT: u32 = 1 << 30;

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
    /// Events mask requested by the user (EPOLLIN, EPOLLOUT, EPOLLET, etc.).
    /// Atomic so EPOLLONESHOT can disable via shared reference.
    events: AtomicU32,
    /// Opaque user data returned in epoll_wait results.
    data: u64,
    /// For EPOLLET: the file's poll_gen() at the time we last reported this fd.
    /// 0 means "never reported" (will fire on first ready poll).
    last_gen: AtomicU64,
    /// Cached poll_gen from the last file.poll() call. 0 = no cache.
    cached_poll_gen: AtomicU64,
    /// Cached PollStatus::bits() from the last file.poll() call.
    cached_poll_bits: AtomicU64,
    /// Direct pointer to file's poll_gen AtomicU64, avoiding vtable dispatch.
    /// Null if the file doesn't implement poll_gen_atomic (falls back to vtable).
    /// Valid as long as `file` Arc is alive (same struct lifetime).
    poll_gen_ptr: *const AtomicU64,
}

// SAFETY: poll_gen_ptr points into the Arc<dyn FileLike>'s allocation which
// is kept alive by the Interest's `file` field. The AtomicU64 it points to
// is Sync. Interest is only accessed under the EpollInstance lock or the
// lock-free path (which guarantees single-threaded access).
#[allow(unsafe_code)]
unsafe impl Send for Interest {}
#[allow(unsafe_code)]
unsafe impl Sync for Interest {}

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
        let mut interests = self.interests.lock_no_irq();
        if interests.contains_key(&fd.as_int()) {
            return Err(Errno::EEXIST.into());
        }
        if event.events & EPOLLET != 0 {
            file.notify_epoll_et(true);
        }
        let poll_gen_ptr = file.poll_gen_atomic()
            .map_or(core::ptr::null(), |a| a as *const AtomicU64);
        interests.insert(fd.as_int(), Interest {
            file,
            events: AtomicU32::new(event.events),
            data: event.data,
            last_gen: AtomicU64::new(0),
            cached_poll_gen: AtomicU64::new(0),
            cached_poll_bits: AtomicU64::new(0),
            poll_gen_ptr,
        });
        Ok(())
    }

    /// `epoll_ctl(EPOLL_CTL_MOD)` — modify an existing registration.
    pub fn modify(&self, fd: Fd, event: &EpollEvent) -> Result<()> {
        let mut interests = self.interests.lock_no_irq();
        let entry = interests.get_mut(&fd.as_int())
            .ok_or(Error::new(Errno::ENOENT))?;
        let old_et = entry.events.load(Ordering::Relaxed) & EPOLLET != 0;
        let new_et = event.events & EPOLLET != 0;
        if old_et != new_et {
            entry.file.notify_epoll_et(new_et);
        }
        entry.events.store(event.events, Ordering::Relaxed);
        entry.data = event.data;
        // Invalidate poll cache on modify.
        entry.cached_poll_gen.store(0, Ordering::Relaxed);
        Ok(())
    }

    /// `epoll_ctl(EPOLL_CTL_DEL)` — remove a registration.
    pub fn delete(&self, fd: Fd) -> Result<()> {
        let mut interests = self.interests.lock_no_irq();
        if let Some(removed) = interests.remove(&fd.as_int()) {
            if removed.events.load(Ordering::Relaxed) & EPOLLET != 0 {
                removed.file.notify_epoll_et(false);
            }
        } else {
            return Err(Errno::ENOENT.into());
        }
        Ok(())
    }

    /// Check if this interest should fire and return the events mask.
    /// Returns None if suppressed (EPOLLONESHOT disabled, or edge-triggered
    /// with no state change). Returns Some(ev) with the events mask otherwise.
    #[inline(always)]
    fn check_interest(interest: &Interest) -> Option<u32> {
        let ev = interest.events.load(Ordering::Relaxed);
        if ev == 0 {
            return None; // Disabled by EPOLLONESHOT — needs EPOLL_CTL_MOD to re-arm.
        }
        if ev & EPOLLET == 0 {
            return Some(ev); // Level-triggered: always report when ready.
        }
        let cur_gen = interest.file.poll_gen();
        if cur_gen == 0 {
            return Some(ev); // File doesn't track generations — fall back to LT.
        }
        let prev = interest.last_gen.load(Ordering::Relaxed);
        if cur_gen == prev {
            return None; // Same generation — suppress edge.
        }
        interest.last_gen.store(cur_gen, Ordering::Relaxed);
        Some(ev)
    }

    /// Poll all interested fds and return ready events.
    /// Returns the number of ready fds (may be 0 if nothing is ready).
    pub fn collect_ready(&self, out: &mut Vec<EpollEvent>, max: usize) -> usize {
        let interests = self.interests.lock_no_irq();
        let mut count = 0;
        for interest in interests.values() {
            if count >= max { break; }
            let ev = match Self::check_interest(interest) {
                Some(ev) => ev,
                None => continue,
            };
            let status = Self::poll_cached(interest);
            let ready = poll_status_to_epoll(status) & (ev | EPOLLERR | EPOLLHUP);
            if ready != 0 {
                out.push(EpollEvent { events: ready, data: interest.data });
                count += 1;
                if ev & EPOLLONESHOT != 0 {
                    interest.events.store(0, Ordering::Relaxed);
                }
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
        Self::collect_ready_inner(&interests, events_ptr, max)
    }

    /// Lock-free variant — caller guarantees no concurrent access to interests.
    ///
    /// # Safety
    /// The EpollInstance's Arc must have strong_count == 1 (only in fd table).
    #[cfg(not(feature = "profile-fortress"))]
    #[allow(unsafe_code)]
    pub unsafe fn collect_ready_to_user_lockfree(
        &self,
        events_ptr: kevlar_platform::address::UserVAddr,
        max: usize,
    ) -> crate::result::Result<usize> {
        let interests = self.interests.get_unchecked();
        Self::collect_ready_inner(interests, events_ptr, max)
    }

    /// Poll an interest, using the per-interest cache when the file's
    /// generation hasn't changed since the last poll.
    #[inline(always)]
    fn poll_cached(interest: &Interest) -> PollStatus {
        // Fast path: use poll_gen_ptr to avoid vtable dispatch when available.
        #[allow(unsafe_code)]
        let cur_gen = if !interest.poll_gen_ptr.is_null() {
            unsafe { (*interest.poll_gen_ptr).load(Ordering::Relaxed) }
        } else {
            interest.file.poll_gen()
        };
        if cur_gen != 0 && cur_gen == interest.cached_poll_gen.load(Ordering::Relaxed) {
            // Generation unchanged — reuse cached poll result.
            return PollStatus::from_bits_truncate(
                interest.cached_poll_bits.load(Ordering::Relaxed) as i16,
            );
        }
        let status = match interest.file.poll() {
            Ok(s) => s,
            Err(_) => PollStatus::POLLERR,
        };
        if cur_gen != 0 {
            interest.cached_poll_gen.store(cur_gen, Ordering::Relaxed);
            interest.cached_poll_bits.store(status.bits() as u64, Ordering::Relaxed);
        }
        status
    }

    fn collect_ready_inner(
        interests: &BTreeMap<i32, Interest>,
        events_ptr: kevlar_platform::address::UserVAddr,
        max: usize,
    ) -> crate::result::Result<usize> {
        let mut count = 0;
        for interest in interests.values() {
            if count >= max { break; }
            let ev = match Self::check_interest(interest) {
                Some(ev) => ev,
                None => continue,
            };
            let status = Self::poll_cached(interest);
            let ready = poll_status_to_epoll(status) & (ev | EPOLLERR | EPOLLHUP);
            if ready != 0 {
                let event = EpollEvent { events: ready, data: interest.data };
                let dest = events_ptr.add(count * 12);
                let mut buf = [0u8; 12];
                buf[0..4].copy_from_slice(&event.events.to_ne_bytes());
                buf[4..12].copy_from_slice(&event.data.to_ne_bytes());
                dest.write_bytes(&buf)?;
                count += 1;
                if ev & EPOLLONESHOT != 0 {
                    interest.events.store(0, Ordering::Relaxed);
                }
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
