// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Unnamed pipe (`pipe(2)`).
use core::fmt;

use kevlar_platform::spinlock::SpinLock;
use kevlar_utils::{once::Once, ring_buffer::RingBuffer};

use crate::{
    fs::{
        inode::{FileLike, PollStatus},
        opened_file::OpenOptions,
    },
    prelude::*,
    process::WaitQueue,
    user_buffer::{UserBufReader, UserBufWriter, UserBuffer, UserBufferMut},
};

const PIPE_SIZE: usize = 4096;

// TODO: Fine-grained wait queue, say, embed a queue in every pipe.
static PIPE_WAIT_QUEUE: Once<WaitQueue> = Once::new();

struct PipeInner {
    buf: RingBuffer<u8, PIPE_SIZE>,
    closed_by_reader: bool,
    closed_by_writer: bool,
}

pub struct Pipe(Arc<SpinLock<PipeInner>>);

impl Pipe {
    pub fn new() -> Pipe {
        Pipe(Arc::new(SpinLock::new(PipeInner {
            buf: RingBuffer::new(),
            closed_by_reader: false,
            closed_by_writer: false,
        })))
    }

    pub fn write_end(&self) -> Arc<PipeWriter> {
        Arc::new(PipeWriter(self.0.clone()))
    }

    pub fn read_end(&self) -> Arc<PipeReader> {
        Arc::new(PipeReader(self.0.clone()))
    }
}

/// Copy from user buffer directly into the pipe's ring buffer.
/// Avoids the intermediate stack buffer by writing into the ring buffer's
/// contiguous free space directly.
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

pub struct PipeWriter(Arc<SpinLock<PipeInner>>);

impl FileLike for PipeWriter {
    fn write(&self, _offset: usize, buf: UserBuffer<'_>, options: &OpenOptions) -> Result<usize> {
        // Fast path: try writing without wait queue overhead.
        // Avoids process enqueue/dequeue + 3 lock cycles when data fits immediately.
        {
            let mut pipe = self.0.lock_no_irq();
            if pipe.closed_by_reader {
                return Err(Errno::EPIPE.into());
            }

            if pipe.buf.is_writable() {
                let written_len = copy_user_to_pipe(&mut pipe.buf, &buf)?;
                if written_len > 0 {
                    drop(pipe);
                    PIPE_WAIT_QUEUE.wake_all();
                    return Ok(written_len);
                }
            }

            if options.nonblock {
                return Ok(0);
            }
        }

        // Slow path: buffer full, wait for reader to drain.
        let ret_value = PIPE_WAIT_QUEUE.sleep_signalable_until(|| {
            let mut pipe = self.0.lock_no_irq();
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

        PIPE_WAIT_QUEUE.wake_all();
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
        let inner = self.0.lock_no_irq();

        if inner.buf.is_writable() {
            status |= PollStatus::POLLOUT;
        }

        Ok(status)
    }
}

impl fmt::Debug for PipeWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PipeWriter").finish()
    }
}

impl Drop for PipeWriter {
    fn drop(&mut self) {
        self.0.lock_no_irq().closed_by_writer = true;
        PIPE_WAIT_QUEUE.wake_all();
    }
}

pub struct PipeReader(Arc<SpinLock<PipeInner>>);

impl FileLike for PipeReader {
    fn write(&self, _offset: usize, _buf: UserBuffer<'_>, _options: &OpenOptions) -> Result<usize> {
        Err(Errno::EINVAL.into())
    }

    fn read(&self, _offset: usize, buf: UserBufferMut<'_>, options: &OpenOptions) -> Result<usize> {
        let mut writer = UserBufWriter::from(buf);

        // Fast path: try reading without wait queue overhead.
        {
            let mut pipe = self.0.lock_no_irq();
            while let Some(src) = pipe.buf.pop_slice(writer.remaining_len()) {
                writer.write_bytes(src)?;
            }

            if writer.written_len() > 0 {
                drop(pipe);
                PIPE_WAIT_QUEUE.wake_all();
                return Ok(writer.written_len());
            }

            if options.nonblock || pipe.closed_by_writer {
                return Ok(0);
            }
        }

        // Slow path: buffer empty, wait for writer.
        let ret_value = PIPE_WAIT_QUEUE.sleep_signalable_until(|| {
            let mut pipe = self.0.lock_no_irq();

            while let Some(src) = pipe.buf.pop_slice(writer.remaining_len()) {
                writer.write_bytes(src)?;
            }

            if writer.written_len() > 0 {
                Ok(Some(writer.written_len()))
            } else if options.nonblock || pipe.closed_by_writer {
                Ok(Some(0))
            } else {
                Ok(None)
            }
        });

        PIPE_WAIT_QUEUE.wake_all();
        ret_value
    }

    fn poll(&self) -> Result<PollStatus> {
        let mut status = PollStatus::empty();
        let inner = self.0.lock_no_irq();

        if inner.buf.is_readable() {
            status |= PollStatus::POLLIN;
        }

        Ok(status)
    }
}

impl fmt::Debug for PipeReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PipeReader").finish()
    }
}

impl Drop for PipeReader {
    fn drop(&mut self) {
        self.0.lock_no_irq().closed_by_reader = true;
        PIPE_WAIT_QUEUE.wake_all();
    }
}

pub fn init() {
    PIPE_WAIT_QUEUE.init(WaitQueue::new);
}
