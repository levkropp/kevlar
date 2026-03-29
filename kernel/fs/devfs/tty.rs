// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use arrayvec::ArrayString;
use core::fmt;

use crate::{
    ctypes::*,
    debug,
    fs::{
        inode::{FileLike, INodeNo, PollStatus},
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
    /// Device major/minor encoded as makedev(major, minor).
    rdev: usize,
}

impl Tty {
    #[allow(dead_code)]
    pub fn new(name: &str) -> Tty {
        Self::with_rdev(name, 0)
    }

    pub fn with_rdev(name: &str, rdev: usize) -> Tty {
        let mut name_buf = ArrayString::new();
        let _ = name_buf.try_push_str(name);
        Tty {
            name: name_buf,
            discipline: LineDiscipline::new(),
            rdev,
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
const TCGETS2: usize = 0x802c542a;  // _IOR('T', 0x2A, struct termios2)
const TCSETS2: usize = 0x402c542b;  // _IOW('T', 0x2B, struct termios2)
const TCSBRK: usize = 0x5409;
const TCFLSH: usize = 0x540b;
const TIOCSCTTY: usize = 0x540e;
const TIOCGPGRP: usize = 0x540f;
const TIOCSPGRP: usize = 0x5410;
const TIOCGWINSZ: usize = 0x5413;
const TIOCSWINSZ: usize = 0x5414;
const TIOCMGET: usize = 0x5415;
const TIOCMBIS: usize = 0x5416;
const TIOCMBIC: usize = 0x5417;
const TIOCMSET: usize = 0x5418;
const TIOCNOTTY: usize = 0x5422;
const TIOCGSID: usize = 0x5429;

impl fmt::Debug for Tty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tty").field("name", &self.name).finish()
    }
}

impl FileLike for Tty {
    fn ioctl(&self, cmd: usize, arg: usize) -> Result<isize> {
        match cmd {
            TCGETS | TCGETS2 => {
                let termios = self.discipline.termios();
                let arg = UserVAddr::new_nonnull(arg)?;
                debug::usercopy::set_context("ioctl:TCGETS");
                let r = arg.write::<Termios>(&termios);
                debug::usercopy::clear_context();
                r?;
            }
            TCSBRK => {
                // tcdrain: wait for output to drain. Our serial output is
                // synchronous, so there is nothing to wait for.
            }
            TCFLSH => {
                // tcflush: discard pending input/output. Accept silently.
                // arg encodes TCIFLUSH(0)/TCOFLUSH(1)/TCIOFLUSH(2).
            }
            TCSETS | TCSETSW | TCSETSF | TCSETS2 => {
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
            TIOCSCTTY => {
                // Set controlling terminal for the calling session.
                // Per POSIX, only the session leader can set a controlling terminal.
                // We accept the request and set the foreground process group to
                // the caller's process group.
                use crate::process::current_process;
                let proc = current_process();
                let pg = proc.process_group();
                self.discipline
                    .set_foreground_process_group(Arc::downgrade(&pg));
            }
            TIOCMGET => {
                // Virtual serial port: always report carrier detect + DSR present.
                const TIOCM_CAR: c_int = 0x040;
                const TIOCM_DSR: c_int = 0x100;
                let status: c_int = TIOCM_CAR | TIOCM_DSR;
                let arg = UserVAddr::new_nonnull(arg)?;
                arg.write::<c_int>(&status)?;
            }
            TIOCMSET | TIOCMBIS | TIOCMBIC => {
                // Ignore modem control writes on virtual serial.
            }
            TIOCGSID => {
                // Return the session ID (= the session leader's PID).
                use crate::process::current_process;
                let sid = current_process().session_id();
                let arg = UserVAddr::new_nonnull(arg)?;
                arg.write::<c_int>(&sid)?;
            }
            TIOCNOTTY => {
                // Detach from controlling terminal. Accept silently.
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
        use crate::fs::stat::DevId;
        Ok(Stat {
            inode_no: INodeNo::new(3),
            mode: FileMode::new(S_IFCHR | 0o666),
            rdev: DevId::new(self.rdev),
            ..Stat::zeroed()
        })
    }

    fn poll(&self) -> Result<PollStatus> {
        let mut status = PollStatus::POLLOUT; // serial write is always ready
        if self.discipline.is_readable() {
            status |= PollStatus::POLLIN;
        }
        Ok(status)
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
