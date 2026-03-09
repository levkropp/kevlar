// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use arrayvec::ArrayString;
use core::fmt;

use crate::{
    ctypes::*,
    debug,
    fs::{
        inode::{FileLike, INodeNo},
        opened_file::OpenOptions,
        stat::{FileMode, Stat, S_IFCHR},
    },
    prelude::*,
    process::process_group::{PgId, ProcessGroup},
    result::Result,
    tty::line_discipline::{LineControl, LineDiscipline, Termios, WinSize},
    user_buffer::UserBuffer,
    user_buffer::{UserBufReader, UserBufferMut},
};
use kevlar_platform::{address::UserVAddr, print::get_printer, spinlock::SpinLock};

pub struct Tty {
    name: ArrayString<8>,
    discipline: LineDiscipline,
}

impl Tty {
    pub fn new(name: &str) -> Tty {
        let mut name_buf = ArrayString::new();
        let _ = name_buf.try_push_str(name);
        Tty {
            name: name_buf,
            discipline: LineDiscipline::new(),
        }
    }

    pub fn input_char(&self, ch: u8) {
        self.discipline
            .write(([ch].as_slice()).into(), |ctrl| {
                match ctrl {
                    LineControl::Backspace => {
                        // Remove the previous character by overwriting with a whitespace.
                        get_printer().print_bytes(b"\x08 \x08");
                    }
                    LineControl::Echo(ch) => {
                        self.write(0, [ch].as_slice().into(), &OpenOptions::readwrite())
                            .ok();
                    }
                }
            })
            .ok();
    }

    pub fn set_foreground_process_group(&self, pg: Weak<SpinLock<ProcessGroup>>) {
        self.discipline.set_foreground_process_group(pg);
    }
}

const TCGETS: usize = 0x5401;
const TCSETS: usize = 0x5402;
const TCSETSW: usize = 0x5403;
const TCSETSF: usize = 0x5404;
const TIOCGPGRP: usize = 0x540f;
const TIOCSPGRP: usize = 0x5410;
const TIOCGWINSZ: usize = 0x5413;
const TIOCSWINSZ: usize = 0x5414;

impl fmt::Debug for Tty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tty").field("name", &self.name).finish()
    }
}

impl FileLike for Tty {
    fn ioctl(&self, cmd: usize, arg: usize) -> Result<isize> {
        match cmd {
            TCGETS => {
                let termios = self.discipline.termios();
                let arg = UserVAddr::new_nonnull(arg)?;
                debug::usercopy::set_context("ioctl:TCGETS");
                let r = arg.write::<Termios>(&termios);
                debug::usercopy::clear_context();
                r?;
            }
            TCSETS | TCSETSW | TCSETSF => {
                let arg = UserVAddr::new_nonnull(arg)?;
                debug::usercopy::set_context("ioctl:TCSETS");
                let termios = arg.read::<Termios>();
                debug::usercopy::clear_context();
                self.discipline.set_termios(termios?);
            }
            TIOCGPGRP => {
                let process_group = self
                    .discipline
                    .foreground_process_group()
                    .ok_or_else(|| Error::new(Errno::ENOENT))?;

                let pgid = process_group.lock().pgid().as_i32();
                let arg = UserVAddr::new_nonnull(arg)?;
                debug::usercopy::set_context("ioctl:TIOCGPGRP");
                let r = arg.write::<c_int>(&pgid);
                debug::usercopy::clear_context();
                r?;
            }
            TIOCSPGRP => {
                let arg = UserVAddr::new_nonnull(arg)?;
                debug::usercopy::set_context("ioctl:TIOCSPGRP");
                let pgid = arg.read::<c_int>();
                debug::usercopy::clear_context();
                let pg = ProcessGroup::find_by_pgid(PgId::new(pgid?))
                    .ok_or_else(|| Error::new(Errno::ESRCH))?;
                self.discipline
                    .set_foreground_process_group(Arc::downgrade(&pg));
            }
            TIOCGWINSZ => {
                let ws = self.discipline.winsize();
                let arg = UserVAddr::new_nonnull(arg)?;
                debug::usercopy::set_context("ioctl:TIOCGWINSZ");
                let r = arg.write::<WinSize>(&ws);
                debug::usercopy::clear_context();
                r?;
            }
            TIOCSWINSZ => {
                let arg = UserVAddr::new_nonnull(arg)?;
                debug::usercopy::set_context("ioctl:TIOCSWINSZ");
                let ws = arg.read::<WinSize>();
                debug::usercopy::clear_context();
                self.discipline.set_winsize(ws?);
            }
            _ => return Err(Errno::ENOSYS.into()),
        }

        Ok(0)
    }

    fn stat(&self) -> Result<Stat> {
        Ok(Stat {
            inode_no: INodeNo::new(3),
            mode: FileMode::new(S_IFCHR | 0o666),
            ..Stat::zeroed()
        })
    }

    fn read(
        &self,
        _offset: usize,
        dst: UserBufferMut<'_>,
        _options: &OpenOptions,
    ) -> Result<usize> {
        self.discipline.read(dst)
    }

    fn write(&self, _offset: usize, buf: UserBuffer<'_>, _options: &OpenOptions) -> Result<usize> {
        let mut tmp = [0; 32];
        let mut total_len = 0;
        let mut reader = UserBufReader::from(buf);
        while reader.remaining_len() > 0 {
            let copied_len = reader.read_bytes(&mut tmp)?;
            get_printer().print_bytes(&tmp.as_slice()[..copied_len]);
            total_len += copied_len;
        }
        Ok(total_len)
    }
}
