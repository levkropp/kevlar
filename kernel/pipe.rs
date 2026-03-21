// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Unnamed pipe (`pipe(2)`).
use alloc::boxed::Box;
use core::fmt;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use kevlar_platform::spinlock::SpinLock;
use kevlar_utils::ring_buffer::RingBuffer;

use crate::{
    fs::{
        inode::{FileLike, PollStatus},
        opened_file::OpenOptions,
    },
    prelude::*,
    process::WaitQueue,
    user_buffer::{UserBufReader, UserBufWriter, UserBuffer, UserBufferMut},
};

/// Pipe buffer size: 64KB, matching Linux default (16 × 4KB pages).
/// Larger buffers reduce context switches for pipeline workloads
/// (sort | uniq | sort) where data fits in one buffer fill.
const PIPE_SIZE: usize = 65536;

struct PipeInner {
    buf: RingBuffer<u8, PIPE_SIZE>,
    closed_by_reader: bool,
    closed_by_writer: bool,
}

/// Shared state for a pipe: data buffer + per-pipe wait queue.
struct PipeShared {
    /// Box-allocated because PipeInner is 64KB+ (the ring buffer).
    /// Can't live on the 16KB kernel stack.
    inner: SpinLock<Box<PipeInner>>,
    waitq: WaitQueue,
    /// Monotonically increasing generation counter for edge-triggered epoll.
    /// Incremented on every state change (read, write, close).
    state_gen: AtomicU64,
    /// Number of EPOLLET watchers on this pipe. When zero, skip the
    /// state_gen fetch_add (~8-10ns per RMW) on the hot path.
    et_watcher_count: AtomicU32,
}

pub struct Pipe(Arc<PipeShared>);

impl Pipe {
    #[allow(unsafe_code)]
    pub fn new() -> Pipe {
        // Allocate PipeInner directly on the heap via alloc_zeroed.
        // Box::new() would construct the 65KB RingBuffer on the stack first,
        // overflowing the 16KB kernel stack when the call stack is deep
        // (e.g. benchmark #43 in a 44-benchmark suite).
        // All fields are correct when zeroed: rp=0, wp=0, full=false,
        // closed_by_reader=false, closed_by_writer=false, MaybeUninit is uninit.
        let inner = unsafe {
            let layout = core::alloc::Layout::new::<PipeInner>();
            let ptr = alloc::alloc::alloc_zeroed(layout) as *mut PipeInner;
            assert!(!ptr.is_null(), "pipe: failed to allocate PipeInner");
            Box::from_raw(ptr)
        };
        Pipe(Arc::new(PipeShared {
            inner: SpinLock::new(inner),
            waitq: WaitQueue::new(),
            state_gen: AtomicU64::new(1),
            et_watcher_count: AtomicU32::new(0),
        }))
    }

    pub fn write_end(&self) -> Arc<PipeWriter> {
        Arc::new(PipeWriter(self.0.clone()))
    }

    pub fn read_end(&self) -> Arc<PipeReader> {
        Arc::new(PipeReader(self.0.clone()))
    }
}

/// Copy from user buffer directly into the pipe's ring buffer.
fn copy_user_to_pipe(
    ring: &mut RingBuffer<u8, PIPE_SIZE>,
    buf: &UserBuffer<'_>,
) -> Result<usize> {
    let mut reader = UserBufReader::from(buf.clone());
    let mut written_len = 0;
    loop {
        let dst = ring.writable_contiguous();
        if dst.is_empty() || reader.remaining_len() == 0 {
            break;
        }
        let copied = reader.read_bytes(dst)?;
        if copied == 0 {
            break;
        }
        ring.advance_write(copied);
        written_len += copied;
    }
    Ok(written_len)
}

pub struct PipeWriter(Arc<PipeShared>);

impl FileLike for PipeWriter {
    fn is_seekable(&self) -> bool {
        false
    }

    fn write(&self, _offset: usize, buf: UserBuffer<'_>, options: &OpenOptions) -> Result<usize> {
        // Fast path: try writing without wait queue overhead.
        {
            let mut pipe = self.0.inner.lock_no_irq();
            if pipe.closed_by_reader {
                return Err(Errno::EPIPE.into());
            }

            if pipe.buf.is_writable() {
                let written_len = copy_user_to_pipe(&mut pipe.buf, &buf)?;
                if written_len > 0 {
                    drop(pipe);
                    if self.0.et_watcher_count.load(Ordering::Relaxed) > 0 {
                        self.0.state_gen.fetch_add(1, Ordering::Relaxed);
                    }
                    self.0.waitq.wake_all();
                    return Ok(written_len);
                }
            }

            if options.nonblock {
                return Err(Errno::EAGAIN.into());
            }
        }

        // Slow path: buffer full, wait for reader to drain.
        let ret_value = self.0.waitq.sleep_signalable_until(|| {
            let mut pipe = self.0.inner.lock_no_irq();
            if pipe.closed_by_reader {
                return Err(Errno::EPIPE.into());
            }

            let written_len = copy_user_to_pipe(&mut pipe.buf, &buf)?;
            if written_len > 0 {
                Ok(Some(written_len))
            } else if options.nonblock {
                Ok(Some(0))
            } else {
                Ok(None)
            }
        });

        if self.0.et_watcher_count.load(Ordering::Relaxed) > 0 {
            self.0.state_gen.fetch_add(1, Ordering::Relaxed);
        }
        self.0.waitq.wake_all();
        ret_value
    }

    fn read(
        &self,
        _offset: usize,
        _buf: UserBufferMut<'_>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        Err(Errno::EINVAL.into())
    }

    fn poll(&self) -> Result<PollStatus> {
        let mut status = PollStatus::empty();
        let inner = self.0.inner.lock_no_irq();

        if inner.buf.is_writable() {
            status |= PollStatus::POLLOUT;
        }

        Ok(status)
    }

    fn poll_gen(&self) -> u64 {
        // Only return a valid generation when ET watchers exist and
        // state_gen is being maintained.  Otherwise return 0 to disable
        // both EPOLLET edge detection and the poll result cache.
        if self.0.et_watcher_count.load(Ordering::Relaxed) > 0 {
            self.0.state_gen.load(Ordering::Relaxed)
        } else {
            0
        }
    }

    fn notify_epoll_et(&self, added: bool) {
        if added {
            self.0.et_watcher_count.fetch_add(1, Ordering::Relaxed);
        } else {
            self.0.et_watcher_count.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

impl fmt::Debug for PipeWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PipeWriter").finish()
    }
}

impl Drop for PipeWriter {
    fn drop(&mut self) {
        self.0.inner.lock_no_irq().closed_by_writer = true;
        if self.0.et_watcher_count.load(Ordering::Relaxed) > 0 {
            self.0.state_gen.fetch_add(1, Ordering::Relaxed);
        }
        self.0.waitq.wake_all();
    }
}

pub struct PipeReader(Arc<PipeShared>);

impl FileLike for PipeReader {
    fn is_seekable(&self) -> bool {
        false
    }

    fn poll(&self) -> Result<PollStatus> {
        let mut status = PollStatus::empty();
        let inner = self.0.inner.lock_no_irq();

        if inner.buf.is_readable() || inner.closed_by_writer {
            status |= PollStatus::POLLIN;
        }

        Ok(status)
    }

    fn write(&self, _offset: usize, _buf: UserBuffer<'_>, _options: &OpenOptions) -> Result<usize> {
        Err(Errno::EINVAL.into())
    }

    fn read(&self, _offset: usize, buf: UserBufferMut<'_>, options: &OpenOptions) -> Result<usize> {
        let mut writer = UserBufWriter::from(buf);
        // Fast path: try reading without wait queue overhead.
        {
            let mut pipe = self.0.inner.lock_no_irq();
            while let Some(src) = pipe.buf.pop_slice(writer.remaining_len()) {
                writer.write_bytes(src)?;
            }

            if writer.written_len() > 0 {
                drop(pipe);
                if self.0.et_watcher_count.load(Ordering::Relaxed) > 0 {
                    self.0.state_gen.fetch_add(1, Ordering::Relaxed);
                }
                self.0.waitq.wake_all();
                return Ok(writer.written_len());
            }

            if pipe.closed_by_writer {
                return Ok(0);
            }
            if options.nonblock {
                return Err(Errno::EAGAIN.into());
            }
        }

        // Slow path: buffer empty, wait for writer.
        let ret_value = self.0.waitq.sleep_signalable_until(|| {
            let mut pipe = self.0.inner.lock_no_irq();

            while let Some(src) = pipe.buf.pop_slice(writer.remaining_len()) {
                writer.write_bytes(src)?;
            }

            if writer.written_len() > 0 {
                Ok(Some(writer.written_len()))
            } else if pipe.closed_by_writer {
                Ok(Some(0))
            } else if options.nonblock {
                Err(Errno::EAGAIN.into())
            } else {
                Ok(None)
            }
        });

        if self.0.et_watcher_count.load(Ordering::Relaxed) > 0 {
            self.0.state_gen.fetch_add(1, Ordering::Relaxed);
        }
        self.0.waitq.wake_all();
        ret_value
    }

    fn poll_gen(&self) -> u64 {
        if self.0.et_watcher_count.load(Ordering::Relaxed) > 0 {
            self.0.state_gen.load(Ordering::Relaxed)
        } else {
            0
        }
    }

    fn notify_epoll_et(&self, added: bool) {
        if added {
            self.0.et_watcher_count.fetch_add(1, Ordering::Relaxed);
        } else {
            self.0.et_watcher_count.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

impl fmt::Debug for PipeReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PipeReader").finish()
    }
}

impl Drop for PipeReader {
    fn drop(&mut self) {
        self.0.inner.lock_no_irq().closed_by_reader = true;
        if self.0.et_watcher_count.load(Ordering::Relaxed) > 0 {
            self.0.state_gen.fetch_add(1, Ordering::Relaxed);
        }
        self.0.waitq.wake_all();
    }
}

pub fn init() {
    // Per-pipe wait queues replaced the global PIPE_WAIT_QUEUE.
    // This function is kept for API compatibility.
}
