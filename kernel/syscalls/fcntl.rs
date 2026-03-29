// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use alloc::vec::Vec;
use hashbrown::HashMap;
use kevlar_platform::address::UserVAddr;
use kevlar_platform::spinlock::SpinLock;

use crate::fs::opened_file::{Fd, OpenFlags, OpenOptions};
use crate::result::{Errno, Error, Result};
use crate::syscalls::SyscallHandler;
use crate::{ctypes::*, process::current_process};

const F_DUPFD: c_int = 0;
const F_GETFD: c_int = 1;
const F_SETFD: c_int = 2;
const F_GETFL: c_int = 3;
const F_SETFL: c_int = 4;
const F_GETLK: c_int = 5;
const F_SETLK: c_int = 6;
const F_SETLKW: c_int = 7;

// Lock types in struct flock.
const F_RDLCK: i16 = 0;
const F_WRLCK: i16 = 1;
const F_UNLCK: i16 = 2;

// Whence values.
const SEEK_SET: i16 = 0;
const SEEK_CUR: i16 = 1;
const SEEK_END: i16 = 2;

// Linux-specific commands.
const F_LINUX_SPECIFIC_BASE: c_int = 1024;
const F_DUPFD_CLOEXEC: c_int = F_LINUX_SPECIFIC_BASE + 6;

// ── struct flock (x86_64 layout: 32 bytes) ──

#[repr(C)]
#[derive(Clone, Copy)]
struct Flock {
    l_type: i16,   // F_RDLCK, F_WRLCK, F_UNLCK
    l_whence: i16, // SEEK_SET, SEEK_CUR, SEEK_END
    _pad: i32,
    l_start: i64,  // starting offset
    l_len: i64,    // length (0 = to EOF)
    l_pid: i32,    // PID of lock holder (returned by F_GETLK)
    _pad2: i32,
}

// ── Record lock table ──

/// Key: (dev_id, inode_no).
type InodeKey = (usize, u64);

#[derive(Clone)]
struct RecordLock {
    start: u64,
    end: u64, // exclusive; u64::MAX means to EOF
    l_type: i16,
    pid: i32,
}

static RECORD_LOCKS: SpinLock<Option<HashMap<InodeKey, Vec<RecordLock>>>> = SpinLock::new(None);
static RECORD_LOCK_WAITQ: spin::Once<crate::process::WaitQueue> = spin::Once::new();

fn record_lock_waitq() -> &'static crate::process::WaitQueue {
    RECORD_LOCK_WAITQ.call_once(crate::process::WaitQueue::new)
}

/// Release all POSIX record locks held by a given PID on all inodes.
/// Called from process exit.
pub fn release_all_record_locks(pid: i32) {
    let mut guard = RECORD_LOCKS.lock_no_irq();
    let table = match guard.as_mut() {
        Some(t) => t,
        None => return,
    };
    let mut any_removed = false;
    let mut empty_keys = Vec::new();
    for (key, locks) in table.iter_mut() {
        let before = locks.len();
        locks.retain(|l| l.pid != pid);
        if locks.len() < before {
            any_removed = true;
        }
        if locks.is_empty() {
            empty_keys.push(*key);
        }
    }
    for key in empty_keys {
        table.remove(&key);
    }
    drop(guard);
    if any_removed {
        record_lock_waitq().wake_all();
    }
}

/// Resolve lock range from struct flock fields + file state.
fn resolve_range(fl: &Flock, file_size: u64) -> (u64, u64) {
    let base = match fl.l_whence {
        SEEK_CUR => 0u64, // Simplified: treat as SEEK_SET (we don't track file pos here)
        SEEK_END => file_size,
        _ => 0, // SEEK_SET
    };
    let start = (base as i64 + fl.l_start).max(0) as u64;
    let end = if fl.l_len == 0 {
        u64::MAX // Lock to EOF
    } else {
        (start as i64 + fl.l_len).max(0) as u64
    };
    (start, end)
}

/// Check if two ranges overlap.
fn ranges_overlap(s1: u64, e1: u64, s2: u64, e2: u64) -> bool {
    s1 < e2 && s2 < e1
}

/// Find a conflicting lock for the given range and type.
fn find_conflict(
    locks: &[RecordLock],
    start: u64,
    end: u64,
    l_type: i16,
    pid: i32,
) -> Option<&RecordLock> {
    for lock in locks {
        if lock.pid == pid {
            continue; // Same process can overlap its own locks
        }
        if !ranges_overlap(lock.start, lock.end, start, end) {
            continue;
        }
        // Conflict: write locks conflict with everything; read locks only conflict with write locks
        if l_type == F_WRLCK || lock.l_type == F_WRLCK {
            return Some(lock);
        }
    }
    None
}

/// Set or clear a lock range for a given PID.
fn set_lock(
    locks: &mut Vec<RecordLock>,
    start: u64,
    end: u64,
    l_type: i16,
    pid: i32,
) {
    // First, remove/trim any existing locks from this PID that overlap.
    let mut i = 0;
    let mut to_add: Vec<RecordLock> = Vec::new();
    while i < locks.len() {
        if locks[i].pid != pid || !ranges_overlap(locks[i].start, locks[i].end, start, end) {
            i += 1;
            continue;
        }
        let old = locks.remove(i);
        // Preserve portions of old lock outside [start, end).
        if old.start < start {
            to_add.push(RecordLock {
                start: old.start,
                end: start,
                l_type: old.l_type,
                pid,
            });
        }
        if old.end > end {
            to_add.push(RecordLock {
                start: end,
                end: old.end,
                l_type: old.l_type,
                pid,
            });
        }
    }
    for l in to_add {
        locks.push(l);
    }

    // If not unlock, add the new lock.
    if l_type != F_UNLCK {
        locks.push(RecordLock { start, end, l_type, pid });
    }
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_fcntl(&mut self, fd: Fd, cmd: c_int, arg: usize) -> Result<isize> {
        let current = current_process();
        let mut opened_files = current.opened_files_no_irq();
        match cmd {
            F_GETFD => {
                let cloexec = if opened_files.get_cloexec(fd)? { 1 } else { 0 };
                Ok(cloexec)
            }
            F_SETFD => {
                opened_files.set_cloexec(fd, arg == 1)?;
                Ok(0)
            }
            F_GETFL => {
                let file = opened_files.get(fd)?;
                let opts = file.options();
                let mut flags: i32 = opts.access_mode;
                if opts.nonblock {
                    flags |= OpenFlags::O_NONBLOCK.bits();
                }
                if opts.append {
                    flags |= OpenFlags::O_APPEND.bits();
                }
                Ok(flags as isize)
            }
            F_SETFL => {
                opened_files
                    .get(fd)?
                    .set_flags(OpenFlags::from_bits_truncate(arg as i32))?;
                Ok(0)
            }
            F_DUPFD => {
                let fd = opened_files.dup(fd, Some(arg as i32), OpenOptions::new(false, false))?;
                Ok(fd.as_int() as isize)
            }
            F_DUPFD_CLOEXEC => {
                let fd = opened_files.dup(fd, Some(arg as i32), OpenOptions::new(false, true))?;
                Ok(fd.as_int() as isize)
            }
            F_GETLK => {
                let file = opened_files.get(fd)?;
                let stat = file.inode().stat()?;
                let key: InodeKey = (stat.dev.as_usize(), stat.inode_no.as_u64());
                let file_size = stat.size.0.max(0) as u64;
                let pid = current.pid().as_i32();

                let fl_ptr = UserVAddr::new_nonnull(arg)?;
                let fl: Flock = fl_ptr.read()?;
                let (start, end) = resolve_range(&fl, file_size);

                let mut guard = RECORD_LOCKS.lock_no_irq();
                let table = guard.get_or_insert_with(HashMap::new);
                let locks = table.get(&key);

                if let Some(locks) = locks {
                    if let Some(conflict) = find_conflict(locks, start, end, fl.l_type, pid) {
                        // Return info about the conflicting lock.
                        let result = Flock {
                            l_type: conflict.l_type,
                            l_whence: SEEK_SET,
                            _pad: 0,
                            l_start: conflict.start as i64,
                            l_len: if conflict.end == u64::MAX { 0 } else { (conflict.end - conflict.start) as i64 },
                            l_pid: conflict.pid,
                            _pad2: 0,
                        };
                        drop(guard);
                        fl_ptr.write(&result)?;
                        return Ok(0);
                    }
                }

                // No conflict — set l_type = F_UNLCK to indicate lock would succeed.
                let result = Flock {
                    l_type: F_UNLCK,
                    l_whence: fl.l_whence,
                    _pad: 0,
                    l_start: fl.l_start,
                    l_len: fl.l_len,
                    l_pid: 0,
                    _pad2: 0,
                };
                drop(guard);
                fl_ptr.write(&result)?;
                Ok(0)
            }
            F_SETLK | F_SETLKW => {
                let file = opened_files.get(fd)?;
                let stat = file.inode().stat()?;
                let key: InodeKey = (stat.dev.as_usize(), stat.inode_no.as_u64());
                let file_size = stat.size.0.max(0) as u64;
                let pid = current.pid().as_i32();

                let fl_ptr = UserVAddr::new_nonnull(arg)?;
                let fl: Flock = fl_ptr.read()?;
                let (start, end) = resolve_range(&fl, file_size);

                // Unlock: always non-blocking.
                if fl.l_type == F_UNLCK {
                    let mut guard = RECORD_LOCKS.lock_no_irq();
                    let table = guard.get_or_insert_with(HashMap::new);
                    if let Some(locks) = table.get_mut(&key) {
                        set_lock(locks, start, end, F_UNLCK, pid);
                        if locks.is_empty() {
                            table.remove(&key);
                        }
                    }
                    drop(guard);
                    record_lock_waitq().wake_all();
                    return Ok(0);
                }

                // Try to acquire the lock.
                {
                    let mut guard = RECORD_LOCKS.lock_no_irq();
                    let table = guard.get_or_insert_with(HashMap::new);
                    let locks = table.entry(key).or_insert_with(Vec::new);
                    if find_conflict(locks, start, end, fl.l_type, pid).is_none() {
                        set_lock(locks, start, end, fl.l_type, pid);
                        return Ok(0);
                    }
                }

                // Conflict exists.
                if cmd == F_SETLK {
                    return Err(Error::new(Errno::EAGAIN));
                }

                // F_SETLKW: block until the lock can be acquired.
                // Drop opened_files lock before sleeping to avoid deadlock.
                drop(opened_files);
                record_lock_waitq().sleep_signalable_until(|| {
                    let mut guard = RECORD_LOCKS.lock_no_irq();
                    let table = guard.get_or_insert_with(HashMap::new);
                    let locks = table.entry(key).or_insert_with(Vec::new);
                    if find_conflict(locks, start, end, fl.l_type, pid).is_none() {
                        set_lock(locks, start, end, fl.l_type, pid);
                        Ok(Some(()))
                    } else {
                        Ok(None) // Still conflicting — keep sleeping
                    }
                })?;
                Ok(0)
            }
            _ => Err(Errno::ENOSYS.into()),
        }
    }
}
