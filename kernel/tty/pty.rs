// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Pseudo-terminal (PTY).

use core::{cmp::min, fmt};

use alloc::sync::Arc;
use alloc::vec::Vec;
use kevlar_platform::spinlock::SpinLock;
use kevlar_utils::id_table::IdTable;

use crate::{
    ctypes::c_int,
    debug,
    fs::{
        inode::{FileLike, INodeNo, PollStatus},
        opened_file::OpenOptions,
        stat::{FileMode, Stat, S_IFCHR},
        tmpfs,
    },
    poll::POLL_WAIT_QUEUE,
    process::{process_group::{PgId, ProcessGroup}, WaitQueue},
    result::{Errno, Error, Result},
    user_buffer::{UserBufReader, UserBufWriter, UserBuffer, UserBufferMut},
};
use kevlar_platform::address::UserVAddr;

use super::line_discipline::{LineControl, LineDiscipline, Termios, WinSize};

static PTY_INDEX_TABLE: SpinLock<IdTable<16>> = SpinLock::new(IdTable::new());

pub struct PtyMaster {
    index: usize,
    wait_queue: WaitQueue,
    buf: SpinLock<Vec<u8>>,
    discipline: LineDiscipline,
}

impl PtyMaster {
    pub fn new() -> Result<(Arc<PtyMaster>, Arc<PtySlave>)> {
        let master = Arc::new(PtyMaster {
            index: PTY_INDEX_TABLE
                .lock()
                .alloc()
                .ok_or_else(|| Error::new(Errno::ENOMEM))?,
            wait_queue: WaitQueue::new(),
            buf: SpinLock::new(Vec::new()),
            discipline: LineDiscipline::new(),
        });

        let slave = Arc::new(PtySlave::new(master.clone()));
        Ok((master, slave))
    }

    pub fn index(&self) -> usize {
        self.index
    }
}

impl Drop for PtyMaster {
    fn drop(&mut self) {
        PTY_INDEX_TABLE.lock().free(self.index);
    }
}

impl FileLike for PtyMaster {
    fn read(
        &self,
        _offset: usize,
        buf: UserBufferMut<'_>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        let mut writer = UserBufWriter::from(buf);
        let read_len = self.wait_queue.sleep_signalable_until(|| {
            let mut buf_lock = self.buf.lock();
            if buf_lock.is_empty() {
                // TODO: NOBLOCK
                return Ok(None);
            }

            let copy_len = min(buf_lock.len(), writer.remaining_len());
            writer.write_bytes(&buf_lock[..copy_len])?;
            buf_lock.drain(..copy_len);
            Ok(Some(copy_len))
        })?;

        if read_len > 0 {
            POLL_WAIT_QUEUE.wake_all();
        }

        Ok(read_len)
    }

    fn write(&self, _offset: usize, buf: UserBuffer<'_>, _options: &OpenOptions) -> Result<usize> {
        let written_len = self.discipline.write(buf, |ctrl| {
            let mut master_buf = self.buf.lock();
            match ctrl {
                LineControl::Backspace => {
                    // Remove the previous character by overwriting with a whitespace.
                    master_buf.extend_from_slice(b"\x08 \x08");
                }
                LineControl::Echo(ch) => {
                    master_buf.push(ch);
                }
            }
        })?;

        if written_len > 0 {
            POLL_WAIT_QUEUE.wake_all();
        }

        Ok(written_len)
    }

    fn ioctl(&self, cmd: usize, arg: usize) -> Result<isize> {
        const TCGETS: usize = 0x5401;
        const TCSETS: usize = 0x5402;
        const TCSETSW: usize = 0x5403;
        const TCSETSF: usize = 0x5404;
        const TCGETS2: usize = 0x802c542a;
        const TCSETS2: usize = 0x402c542b;
        const TIOCGWINSZ: usize = 0x5413;
        const TIOCSWINSZ: usize = 0x5414;
        const TIOCSPTLCK: usize = 0x40045431;
        const TIOCGPTN: usize = 0x80045430;

        match cmd {
            TCGETS | TCGETS2 => {
                let termios = self.discipline.termios();
                let arg = UserVAddr::new_nonnull(arg)?;
                debug::usercopy::set_context("pty_master:TCGETS");
                let r = arg.write::<Termios>(&termios);
                debug::usercopy::clear_context();
                r?;
                Ok(0)
            }
            TCSETS | TCSETSW | TCSETSF | TCSETS2 => {
                let arg = UserVAddr::new_nonnull(arg)?;
                debug::usercopy::set_context("pty_master:TCSETS");
                let termios = arg.read::<Termios>();
                debug::usercopy::clear_context();
                self.discipline.set_termios(termios?);
                Ok(0)
            }
            TIOCGPTN => {
                let arg = UserVAddr::new_nonnull(arg)?;
                debug::usercopy::set_context("pty_master:TIOCGPTN");
                let r = arg.write::<u32>(&(self.index as u32));
                debug::usercopy::clear_context();
                r?;
                Ok(0)
            }
            TIOCGWINSZ => {
                let ws = self.discipline.winsize();
                let arg = UserVAddr::new_nonnull(arg)?;
                debug::usercopy::set_context("pty_master:TIOCGWINSZ");
                let r = arg.write::<WinSize>(&ws);
                debug::usercopy::clear_context();
                r?;
                Ok(0)
            }
            TIOCSWINSZ => {
                let arg = UserVAddr::new_nonnull(arg)?;
                debug::usercopy::set_context("pty_master:TIOCSWINSZ");
                let ws = arg.read::<WinSize>();
                debug::usercopy::clear_context();
                self.discipline.set_winsize(ws?);
                Ok(0)
            }
            TIOCSPTLCK => Ok(0),
            _ => {
                debug_warn!("pty_master: unknown cmd={:x}", cmd);
                Ok(0)
            }
        }
    }

    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            inode_no: INodeNo::new(5), // FIXME:
            mode: FileMode::new(S_IFCHR | 0o666),
            ..Stat::zeroed()
        })
    }

    fn poll(&self) -> Result<PollStatus> {
        let mut status = PollStatus::empty();

        if !self.buf.lock().is_empty() {
            status |= PollStatus::POLLIN;
        }

        if self.discipline.is_writable() {
            status |= PollStatus::POLLOUT;
        }

        Ok(status)
    }
}

impl fmt::Debug for PtyMaster {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PtyMaster")
            .field("index", &self.index)
            .finish()
    }
}

pub struct PtySlave {
    master: Arc<PtyMaster>,
}

impl PtySlave {
    pub fn new(master: Arc<PtyMaster>) -> PtySlave {
        PtySlave { master }
    }
}

impl FileLike for PtySlave {
    fn read(
        &self,
        _offset: usize,
        buf: UserBufferMut<'_>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        let read_len = self.master.discipline.read(buf)?;
        if read_len > 0 {
            POLL_WAIT_QUEUE.wake_all();
        }
        Ok(read_len)
    }

    fn write(&self, _offset: usize, buf: UserBuffer<'_>, _options: &OpenOptions) -> Result<usize> {
        let mut written_len = 0;
        let mut master_buf = self.master.buf.lock();
        let mut reader = UserBufReader::from(buf);
        while reader.remaining_len() > 0 {
            let mut tmp = [0; 128];
            let copied_len = reader.read_bytes(&mut tmp)?;
            for ch in &tmp[..copied_len] {
                // FIXME: Block if the buffer become too large.
                // TODO: check termios
                match *ch {
                    b'\n' => {
                        // ONLCR: Convert NL to CR + NL
                        master_buf.push(b'\r');
                        master_buf.push(b'\n');
                    }
                    _ => {
                        master_buf.push(*ch);
                    }
                }
            }

            written_len += copied_len;
        }

        if written_len > 0 {
            POLL_WAIT_QUEUE.wake_all();
        }
        Ok(written_len)
    }

    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            inode_no: INodeNo::new(6), // FIXME:
            mode: FileMode::new(S_IFCHR | 0o666),
            ..Stat::zeroed()
        })
    }

    fn ioctl(&self, cmd: usize, arg: usize) -> Result<isize> {
        const TCGETS: usize = 0x5401;
        const TCSETS: usize = 0x5402;
        const TCSETSW: usize = 0x5403;
        const TCSETSF: usize = 0x5404;
        const TCGETS2: usize = 0x802c542a;
        const TCSETS2: usize = 0x402c542b;
        const TIOCGPGRP: usize = 0x540f;
        const TIOCSPGRP: usize = 0x5410;
        const TIOCGWINSZ: usize = 0x5413;
        const TIOCSWINSZ: usize = 0x5414;
        const TIOCSPTLCK: usize = 0x40045431;

        match cmd {
            TCGETS | TCGETS2 => {
                let termios = self.master.discipline.termios();
                let arg = UserVAddr::new_nonnull(arg)?;
                debug::usercopy::set_context("pty_slave:TCGETS");
                let r = arg.write::<Termios>(&termios);
                debug::usercopy::clear_context();
                r?;
                Ok(0)
            }
            TCSETS | TCSETSW | TCSETSF | TCSETS2 => {
                let arg = UserVAddr::new_nonnull(arg)?;
                debug::usercopy::set_context("pty_slave:TCSETS");
                let termios = arg.read::<Termios>();
                debug::usercopy::clear_context();
                self.master.discipline.set_termios(termios?);
                Ok(0)
            }
            TIOCGPGRP => {
                let pg = self.master.discipline
                    .foreground_process_group()
                    .ok_or_else(|| Error::new(Errno::ENOENT))?;
                let pgid = pg.lock().pgid().as_i32();
                let arg = UserVAddr::new_nonnull(arg)?;
                debug::usercopy::set_context("pty_slave:TIOCGPGRP");
                let r = arg.write::<c_int>(&pgid);
                debug::usercopy::clear_context();
                r?;
                Ok(0)
            }
            TIOCSPGRP => {
                let arg = UserVAddr::new_nonnull(arg)?;
                debug::usercopy::set_context("pty_slave:TIOCSPGRP");
                let pgid = arg.read::<c_int>();
                debug::usercopy::clear_context();
                let pg = ProcessGroup::find_by_pgid(PgId::new(pgid?))
                    .ok_or_else(|| Error::new(Errno::ESRCH))?;
                self.master.discipline
                    .set_foreground_process_group(Arc::downgrade(&pg));
                Ok(0)
            }
            TIOCGWINSZ => {
                let ws = self.master.discipline.winsize();
                let arg = UserVAddr::new_nonnull(arg)?;
                debug::usercopy::set_context("pty_slave:TIOCGWINSZ");
                let r = arg.write::<WinSize>(&ws);
                debug::usercopy::clear_context();
                r?;
                Ok(0)
            }
            TIOCSWINSZ => {
                let arg = UserVAddr::new_nonnull(arg)?;
                debug::usercopy::set_context("pty_slave:TIOCSWINSZ");
                let ws = arg.read::<WinSize>();
                debug::usercopy::clear_context();
                self.master.discipline.set_winsize(ws?);
                Ok(0)
            }
            TIOCSPTLCK => Ok(0),
            _ => {
                debug_warn!("pty_slave: unknown cmd={:x}", cmd);
                Ok(0)
            }
        }
    }

    fn poll(&self) -> Result<PollStatus> {
        let mut status = PollStatus::empty();

        if self.master.discipline.is_readable() {
            status |= PollStatus::POLLIN;
        }

        // TODO: if self.master.discipline.lock().len() > FULL {
        status |= PollStatus::POLLOUT;

        Ok(status)
    }
}

impl fmt::Debug for PtySlave {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PtySlave")
            .field("master", &self.master.index)
            .finish()
    }
}

pub struct Ptmx {
    pts_dir: Arc<tmpfs::Dir>,
}

impl Ptmx {
    pub fn new(pts_dir: Arc<tmpfs::Dir>) -> Ptmx {
        Ptmx { pts_dir }
    }
}

impl FileLike for Ptmx {
    fn open(&self, _options: &OpenOptions) -> Result<Option<Arc<dyn FileLike>>> {
        let (master, slave) = PtyMaster::new()?;
        self.pts_dir.add_file(&format!("{}", master.index()), slave);
        Ok(Some(master as Arc<dyn FileLike>))
    }

    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            inode_no: INodeNo::new(4),
            mode: FileMode::new(S_IFCHR | 0o666),
            ..Stat::zeroed()
        })
    }

    fn read(
        &self,
        _offset: usize,
        _buf: UserBufferMut<'_>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        unreachable!();
    }

    fn write(&self, _offset: usize, _buf: UserBuffer<'_>, _options: &OpenOptions) -> Result<usize> {
        unreachable!();
    }

    fn poll(&self) -> Result<PollStatus> {
        let status = PollStatus::empty();
        // TODO: What should we return?
        Ok(status)
    }
}

impl fmt::Debug for Ptmx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ptmx").finish()
    }
}
