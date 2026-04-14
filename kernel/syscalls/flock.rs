// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! flock(2) — advisory file locking.
//!
//! Locks are per-open-file-description (OFD), keyed by inode identity
//! (dev_id, inode_no). Fork'd children share the OFD and thus share
//! the lock. Independent opens create separate OFDs that must contend.
use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use hashbrown::HashMap;
use kevlar_platform::spinlock::SpinLock;

use crate::{
    fs::opened_file::Fd,
    process::current_process,
    result::{Errno, Error, Result},
    syscalls::SyscallHandler,
};

const LOCK_SH: i32 = 1;
const LOCK_EX: i32 = 2;
const LOCK_UN: i32 = 8;
const LOCK_NB: i32 = 4;

/// Identity of an open file description (raw Arc pointer).
type Ofd = usize;

/// Key: (dev_id, inode_no).
type InodeKey = (usize, u64);

#[derive(Debug, Clone)]
struct FlockEntry {
    /// Set of OFDs holding a shared lock.
    shared: BTreeSet<Ofd>,
    /// OFD holding the exclusive lock, if any.
    exclusive: Option<Ofd>,
}

static FLOCK_TABLE: SpinLock<Option<HashMap<InodeKey, FlockEntry>>> = SpinLock::new(None);

fn flock_table() -> &'static SpinLock<Option<HashMap<InodeKey, FlockEntry>>> {
    &FLOCK_TABLE
}

/// Release all flock locks held by the given OFD.
/// Called when an OpenedFile is dropped (last fd closed or process exit).
pub fn release_all_flocks(ofd: Ofd) {
    let mut guard = flock_table().lock_no_irq();
    let table = match guard.as_mut() {
        Some(t) => t,
        None => return,
    };

    // Collect keys to remove to avoid borrowing issues.
    let mut empty_keys = Vec::new();
    for (key, entry) in table.iter_mut() {
        entry.shared.remove(&ofd);
        if entry.exclusive == Some(ofd) {
            entry.exclusive = None;
        }
        if entry.shared.is_empty() && entry.exclusive.is_none() {
            empty_keys.push(*key);
        }
    }
    for key in empty_keys {
        table.remove(&key);
    }
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_flock(&mut self, fd: i32, operation: i32) -> Result<isize> {
        let opened_file = current_process().get_opened_file_by_fd(Fd::new(fd))?;

        // OFD identity: raw Arc pointer (stable for the lifetime of the description).
        let ofd = alloc::sync::Arc::as_ptr(&opened_file) as usize;

        // Get inode identity for lock keying (avoids full stat()).
        let key: InodeKey = opened_file.inode().inode_key()?;

        let op = operation & !LOCK_NB;
        let nonblock = (operation & LOCK_NB) != 0;

        let mut guard = flock_table().lock_no_irq();
        let table = guard.get_or_insert_with(HashMap::new);

        match op {
            LOCK_UN => {
                if let Some(entry) = table.get_mut(&key) {
                    entry.shared.remove(&ofd);
                    if entry.exclusive == Some(ofd) {
                        entry.exclusive = None;
                    }
                    if entry.shared.is_empty() && entry.exclusive.is_none() {
                        table.remove(&key);
                    }
                }
                Ok(0)
            }
            LOCK_SH => {
                let entry = table.entry(key).or_insert_with(|| FlockEntry {
                    shared: BTreeSet::new(),
                    exclusive: None,
                });
                // Can acquire shared if: no exclusive holder, or we ARE the exclusive holder.
                match entry.exclusive {
                    Some(holder) if holder != ofd => {
                        if nonblock {
                            return Err(Error::new(Errno::EAGAIN));
                        }
                        // Non-blocking fallback: in practice contention is rare.
                        // Return EWOULDBLOCK rather than deadlocking.
                        return Err(Error::new(Errno::EAGAIN));
                    }
                    Some(_) => {
                        // Downgrade: exclusive → shared.
                        entry.exclusive = None;
                        entry.shared.insert(ofd);
                    }
                    None => {
                        entry.shared.insert(ofd);
                    }
                }
                Ok(0)
            }
            LOCK_EX => {
                let entry = table.entry(key).or_insert_with(|| FlockEntry {
                    shared: BTreeSet::new(),
                    exclusive: None,
                });
                // Can acquire exclusive if:
                // - no holders at all, OR
                // - only this OFD holds shared, OR
                // - this OFD already holds exclusive.
                if let Some(holder) = entry.exclusive {
                    if holder == ofd {
                        // Already holds exclusive — no-op.
                        return Ok(0);
                    }
                    // Another OFD holds exclusive.
                    if nonblock {
                        return Err(Error::new(Errno::EAGAIN));
                    }
                    return Err(Error::new(Errno::EAGAIN));
                }
                // Check shared holders: only this OFD (or nobody) is OK.
                let other_shared = entry.shared.iter().any(|&h| h != ofd);
                if other_shared {
                    if nonblock {
                        return Err(Error::new(Errno::EAGAIN));
                    }
                    return Err(Error::new(Errno::EAGAIN));
                }
                // Upgrade or acquire.
                entry.shared.remove(&ofd);
                entry.exclusive = Some(ofd);
                Ok(0)
            }
            _ => {
                Err(Error::new(Errno::EINVAL))
            }
        }
    }
}
