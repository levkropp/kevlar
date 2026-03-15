// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! inotify(7) — File/directory change notification.
//!
//! Provenance: Own (Linux inotify(7) man page, FreeBSD linux_inotify.c BSD-2-Clause).
use core::fmt;
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use crate::fs::inode::{FileLike, PollStatus};
use crate::fs::path::{Path, PathBuf};
use crate::poll::POLL_WAIT_QUEUE;
use crate::prelude::*;
use crate::user_buffer::{UserBufWriter, UserBufferMut};
use kevlar_platform::spinlock::SpinLock;
use kevlar_vfs::inode::OpenOptions;
use kevlar_vfs::stat::Stat;

// ── inotify event mask constants ─────────────────────────────────

pub const IN_ACCESS: u32 = 0x00000001;
pub const IN_MODIFY: u32 = 0x00000002;
pub const IN_ATTRIB: u32 = 0x00000004;
pub const IN_CLOSE_WRITE: u32 = 0x00000008;
pub const IN_CLOSE_NOWRITE: u32 = 0x00000010;
pub const IN_OPEN: u32 = 0x00000020;
pub const IN_MOVED_FROM: u32 = 0x00000040;
pub const IN_MOVED_TO: u32 = 0x00000080;
pub const IN_CREATE: u32 = 0x00000100;
pub const IN_DELETE: u32 = 0x00000200;
#[allow(dead_code)]
pub const IN_DELETE_SELF: u32 = 0x00000400;
#[allow(dead_code)]
pub const IN_MOVE_SELF: u32 = 0x00000800;

pub const IN_CLOEXEC: i32 = 0o2000000;
pub const IN_NONBLOCK: i32 = 0o4000;

/// All events that can be watched on a directory (events about children).
const IN_DIR_EVENTS: u32 = IN_CREATE | IN_DELETE | IN_MOVED_FROM | IN_MOVED_TO
    | IN_OPEN | IN_CLOSE_WRITE | IN_CLOSE_NOWRITE | IN_MODIFY | IN_ACCESS | IN_ATTRIB;

// ── Global inotify registry ──────────────────────────────────────

static REGISTRY: SpinLock<Vec<Arc<InotifyInstance>>> = SpinLock::new(Vec::new());
static NEXT_COOKIE: AtomicU32 = AtomicU32::new(1);

/// Notify all inotify instances about a VFS event.
///
/// `dir_path` is the path of the directory containing the affected file.
/// `name` is the filename within that directory (empty for self-events).
/// `mask` is the event type (IN_CREATE, IN_DELETE, etc.).
pub fn notify(dir_path: &str, name: &str, mask: u32) {
    let registry = REGISTRY.lock();
    if registry.is_empty() {
        return;
    }
    for instance in registry.iter() {
        instance.match_and_queue(dir_path, name, mask, 0);
    }
    drop(registry);
    POLL_WAIT_QUEUE.wake_all();
}

/// Notify about a rename (paired IN_MOVED_FROM + IN_MOVED_TO with same cookie).
pub fn notify_rename(old_dir: &str, old_name: &str, new_dir: &str, new_name: &str) {
    let registry = REGISTRY.lock();
    if registry.is_empty() {
        return;
    }
    let cookie = NEXT_COOKIE.fetch_add(1, Ordering::Relaxed);
    for instance in registry.iter() {
        instance.match_and_queue(old_dir, old_name, IN_MOVED_FROM, cookie);
        instance.match_and_queue(new_dir, new_name, IN_MOVED_TO, cookie);
    }
    drop(registry);
    POLL_WAIT_QUEUE.wake_all();
}

fn register(instance: &Arc<InotifyInstance>) {
    REGISTRY.lock().push(instance.clone());
}

fn unregister(instance: &Arc<InotifyInstance>) {
    let mut registry = REGISTRY.lock();
    registry.retain(|i| !Arc::ptr_eq(i, instance));
}

// ── InotifyInstance ──────────────────────────────────────────────

struct InotifyWatch {
    wd: i32,
    path: PathBuf,
    mask: u32,
}

struct QueuedEvent {
    wd: i32,
    mask: u32,
    cookie: u32,
    name: String,
}

pub struct InotifyInstance {
    watches: SpinLock<Vec<InotifyWatch>>,
    events: SpinLock<VecDeque<QueuedEvent>>,
    next_wd: AtomicI32,
    self_ref: SpinLock<Option<Arc<InotifyInstance>>>,
}

impl InotifyInstance {
    pub fn new() -> Arc<InotifyInstance> {
        let inst = Arc::new(InotifyInstance {
            watches: SpinLock::new(Vec::new()),
            events: SpinLock::new(VecDeque::new()),
            next_wd: AtomicI32::new(1),
            self_ref: SpinLock::new(None),
        });
        *inst.self_ref.lock() = Some(inst.clone());
        register(&inst);
        inst
    }

    pub fn add_watch(&self, path: &Path, mask: u32) -> i32 {
        let mut watches = self.watches.lock();
        // If a watch already exists for this path, update the mask.
        for w in watches.iter_mut() {
            if w.path.as_str() == path.as_str() {
                w.mask = mask;
                return w.wd;
            }
        }
        let wd = self.next_wd.fetch_add(1, Ordering::Relaxed);
        watches.push(InotifyWatch {
            wd,
            path: path.to_path_buf(),
            mask,
        });
        wd
    }

    pub fn rm_watch(&self, wd: i32) -> Result<()> {
        let mut watches = self.watches.lock();
        let len_before = watches.len();
        watches.retain(|w| w.wd != wd);
        if watches.len() == len_before {
            return Err(Error::new(Errno::EINVAL));
        }
        Ok(())
    }

    fn match_and_queue(&self, dir_path: &str, name: &str, mask: u32, cookie: u32) {
        let watches = self.watches.lock();
        for w in watches.iter() {
            if (w.mask & mask) == 0 {
                continue;
            }
            let watch_path = w.path.as_str();
            // Directory watch: dir_path matches the watched path.
            if dir_path == watch_path && (mask & IN_DIR_EVENTS) != 0 {
                self.events.lock().push_back(QueuedEvent {
                    wd: w.wd,
                    mask,
                    cookie,
                    name: String::from(name),
                });
            }
            // Self-event: the watched path itself is the target.
            if !name.is_empty() {
                let full = if dir_path.ends_with('/') {
                    alloc::format!("{}{}", dir_path, name)
                } else {
                    alloc::format!("{}/{}", dir_path, name)
                };
                if full == watch_path {
                    self.events.lock().push_back(QueuedEvent {
                        wd: w.wd,
                        mask,
                        cookie,
                        name: String::new(),
                    });
                }
            }
        }
    }
}

impl Drop for InotifyInstance {
    fn drop(&mut self) {
        if let Some(ref self_arc) = *self.self_ref.lock() {
            unregister(self_arc);
        }
    }
}

impl fmt::Debug for InotifyInstance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InotifyInstance").finish()
    }
}

/// Serialize a queued event into the Linux `struct inotify_event` wire format.
/// Returns the number of bytes written.
fn write_event(writer: &mut UserBufWriter<'_>, event: &QueuedEvent) -> Result<usize> {
    let name_bytes = event.name.as_bytes();
    // len includes the NUL terminator, padded to 4-byte alignment.
    let raw_len = if name_bytes.is_empty() {
        0u32
    } else {
        let with_nul = name_bytes.len() + 1;
        // Round up to 4-byte alignment.
        ((with_nul + 3) & !3) as u32
    };
    let total = 16 + raw_len as usize; // 4+4+4+4 header + name

    if writer.remaining_len() < total {
        return Ok(0);
    }

    writer.write_bytes(&event.wd.to_ne_bytes())?;
    writer.write_bytes(&event.mask.to_ne_bytes())?;
    writer.write_bytes(&event.cookie.to_ne_bytes())?;
    writer.write_bytes(&raw_len.to_ne_bytes())?;
    if raw_len > 0 {
        writer.write_bytes(name_bytes)?;
        // NUL terminator + padding.
        let padding_len = raw_len as usize - name_bytes.len();
        let zeros = [0u8; 4];
        writer.write_bytes(&zeros[..padding_len])?;
    }
    Ok(total)
}

impl FileLike for InotifyInstance {
    fn stat(&self) -> Result<Stat> {
        Ok(Stat::zeroed())
    }

    fn read(
        &self,
        _offset: usize,
        buf: UserBufferMut<'_>,
        options: &OpenOptions,
    ) -> Result<usize> {
        let mut writer = UserBufWriter::from(buf);

        // Fast path: drain pending events.
        {
            let mut events = self.events.lock();
            while let Some(event) = events.front() {
                let written = write_event(&mut writer, event)?;
                if written == 0 {
                    break; // Buffer full.
                }
                events.pop_front();
            }
        }

        if writer.written_len() > 0 {
            return Ok(writer.written_len());
        }

        if options.nonblock {
            return Err(Errno::EAGAIN.into());
        }

        // Slow path: block until events arrive.
        POLL_WAIT_QUEUE.sleep_signalable_until(|| {
            let mut events = self.events.lock();
            if let Some(event) = events.front() {
                let written = write_event(&mut writer, event)?;
                if written > 0 {
                    events.pop_front();
                    // Drain remaining events that fit.
                    while let Some(event) = events.front() {
                        let w = write_event(&mut writer, event)?;
                        if w == 0 {
                            break;
                        }
                        events.pop_front();
                    }
                    Ok(Some(writer.written_len()))
                } else {
                    // Event doesn't fit in buffer at all.
                    Err(Errno::EINVAL.into())
                }
            } else {
                Ok(None)
            }
        })
    }

    fn poll(&self) -> Result<PollStatus> {
        let events = self.events.lock();
        if !events.is_empty() {
            Ok(PollStatus::POLLIN)
        } else {
            Ok(PollStatus::empty())
        }
    }
}
