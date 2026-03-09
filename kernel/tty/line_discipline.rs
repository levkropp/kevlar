// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Line discipline.
//!
//! Provenance: Own (POSIX termios(3), Linux termbits.h, tty_ioctl(4) man pages).

use crate::{
    prelude::*,
    process::{current_process, process_group::ProcessGroup, signal::{SIGINT, SIGTSTP}, WaitQueue},
    user_buffer::{UserBufReader, UserBufWriter, UserBuffer, UserBufferMut},
};
use bitflags::bitflags;
use kevlar_runtime::spinlock::SpinLock;
use kevlar_utils::ring_buffer::RingBuffer;

// c_cc indices (Linux ABI, from asm-generic/termbits.h)
pub const VINTR: usize = 0;
pub const VQUIT: usize = 1;
pub const VERASE: usize = 2;
pub const VKILL: usize = 3;
pub const VEOF: usize = 4;
pub const VTIME: usize = 5;
pub const VMIN: usize = 6;
pub const VSUSP: usize = 10;
pub const NCCS: usize = 19;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct IFlag: u32 {
        const IGNBRK  = 0o0000001;
        const BRKINT  = 0o0000002;
        const IGNPAR  = 0o0000004;
        const INPCK   = 0o0000020;
        const ISTRIP  = 0o0000040;
        const INLCR   = 0o0000100;
        const IGNCR   = 0o0000200;
        const ICRNL   = 0o0000400;
        const IXON    = 0o0002000;
        const IXOFF   = 0o0010000;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct OFlag: u32 {
        const OPOST = 0o0000001;
        const ONLCR = 0o0000004;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct CFlag: u32 {
        const CS5    = 0o0000000;
        const CS6    = 0o0000020;
        const CS7    = 0o0000040;
        const CS8    = 0o0000060;
        const CREAD  = 0o0000200;
        const HUPCL  = 0o0002000;
        const B9600  = 0o0000015;
        const B38400 = 0o0000017;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct LFlag: u32 {
        const ISIG   = 0o0000001;
        const ICANON = 0o0000002;
        const ECHO   = 0o0000010;
        const ECHOE  = 0o0000020;
        const ECHOK  = 0o0000040;
        const ECHONL = 0o0000100;
        const NOFLSH = 0o0000200;
        const IEXTEN = 0o0100000;
    }
}

/// Linux kernel `struct termios` (36 bytes, matches asm-generic/termbits.h).
#[derive(Clone, Copy)]
#[repr(C)]
pub struct Termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line: u8,
    pub c_cc: [u8; NCCS],
}

impl Termios {
    pub fn is_cooked_mode(&self) -> bool {
        LFlag::from_bits_truncate(self.c_lflag).contains(LFlag::ICANON)
    }

    pub fn echo_enabled(&self) -> bool {
        LFlag::from_bits_truncate(self.c_lflag).contains(LFlag::ECHO)
    }

    pub fn signals_enabled(&self) -> bool {
        LFlag::from_bits_truncate(self.c_lflag).contains(LFlag::ISIG)
    }

    pub fn icrnl(&self) -> bool {
        IFlag::from_bits_truncate(self.c_iflag).contains(IFlag::ICRNL)
    }
}

impl Default for Termios {
    fn default() -> Termios {
        let mut c_cc = [0u8; NCCS];
        c_cc[VINTR] = 0x03;  // ^C
        c_cc[VQUIT] = 0x1c;  // ^\
        c_cc[VERASE] = 0x7f; // DEL
        c_cc[VKILL] = 0x15;  // ^U
        c_cc[VEOF] = 0x04;   // ^D
        c_cc[VTIME] = 0;
        c_cc[VMIN] = 1;
        c_cc[VSUSP] = 0x1a;  // ^Z
        Termios {
            c_iflag: (IFlag::ICRNL | IFlag::IXON).bits(),
            c_oflag: (OFlag::OPOST | OFlag::ONLCR).bits(),
            c_cflag: (CFlag::B38400 | CFlag::CS8 | CFlag::CREAD | CFlag::HUPCL).bits(),
            c_lflag: (LFlag::ISIG | LFlag::ICANON | LFlag::ECHO | LFlag::ECHOE | LFlag::ECHOK | LFlag::IEXTEN).bits(),
            c_line: 0,
            c_cc,
        }
    }
}

/// Window size, matches Linux `struct winsize`.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct WinSize {
    pub ws_row: u16,
    pub ws_col: u16,
    pub ws_xpixel: u16,
    pub ws_ypixel: u16,
}

impl Default for WinSize {
    fn default() -> WinSize {
        WinSize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
    }
}

// TODO: cursor
pub struct LineEdit {
    buf: Vec<u8>,
}

impl LineEdit {
    pub fn new() -> LineEdit {
        LineEdit { buf: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    pub fn insert(&mut self, ch: u8) {
        self.buf.push(ch);
    }

    pub fn backspace(&mut self) {
        self.buf.pop();
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }
}

#[derive(Debug, Clone, Copy)]
pub enum LineControl {
    Backspace,
    Echo(u8),
}

pub struct LineDiscipline {
    wait_queue: WaitQueue,
    current_line: SpinLock<LineEdit>,
    buf: SpinLock<RingBuffer<u8, 4096>>,
    termios: SpinLock<Termios>,
    winsize: SpinLock<WinSize>,
    foreground_process_group: SpinLock<Weak<SpinLock<ProcessGroup>>>,
}

impl LineDiscipline {
    pub fn new() -> LineDiscipline {
        LineDiscipline {
            wait_queue: WaitQueue::new(),
            current_line: SpinLock::new(LineEdit::new()),
            buf: SpinLock::new(RingBuffer::new()),
            termios: SpinLock::new(Default::default()),
            winsize: SpinLock::new(Default::default()),
            foreground_process_group: SpinLock::new(Weak::new()),
        }
    }

    pub fn is_readable(&self) -> bool {
        self.buf.lock().is_readable()
    }

    pub fn is_writable(&self) -> bool {
        self.buf.lock().is_writable()
    }

    pub fn termios(&self) -> Termios {
        *self.termios.lock()
    }

    pub fn set_termios(&self, termios: Termios) {
        *self.termios.lock() = termios;
    }

    pub fn winsize(&self) -> WinSize {
        *self.winsize.lock()
    }

    pub fn set_winsize(&self, ws: WinSize) {
        *self.winsize.lock() = ws;
    }

    pub fn foreground_process_group(&self) -> Option<Arc<SpinLock<ProcessGroup>>> {
        self.foreground_process_group.lock().upgrade()
    }

    pub fn set_foreground_process_group(&self, pg: Weak<SpinLock<ProcessGroup>>) {
        *self.foreground_process_group.lock() = pg;
    }

    fn is_current_foreground(&self) -> bool {
        let foreground_pg = &*self.foreground_process_group.lock();
        current_process().belongs_to_process_group(foreground_pg)
        // If the foreground process is not yet set, allow any processes to read
        // from the tty. I'm not sure whether it is a correct behaviour.
        || foreground_pg.upgrade().is_none()
    }

    pub fn write<F>(&self, buf: UserBuffer<'_>, callback: F) -> Result<usize>
    where
        F: Fn(LineControl),
    {
        let termios = *self.termios.lock();
        let mut current_line = self.current_line.lock();
        let mut ringbuf = self.buf.lock();
        let mut written_len = 0;
        let mut reader = UserBufReader::from(buf);
        while reader.remaining_len() > 0 {
            let mut tmp = [0; 128];
            let copied_len = reader.read_bytes(&mut tmp)?;
            for ch in &tmp.as_slice()[..copied_len] {
                // Signal-generating characters (when ISIG is set).
                if termios.signals_enabled() {
                    if *ch == termios.c_cc[VINTR] {
                        if let Some(pg) = self.foreground_process_group() {
                            pg.lock().signal(SIGINT);
                        }
                        written_len += 1;
                        continue;
                    }
                    if *ch == termios.c_cc[VSUSP] {
                        if let Some(pg) = self.foreground_process_group() {
                            pg.lock().signal(SIGTSTP);
                        }
                        written_len += 1;
                        continue;
                    }
                }

                if termios.is_cooked_mode() {
                    // EOF (^D): flush current line without adding the EOF char itself.
                    if *ch == termios.c_cc[VEOF] {
                        if !current_line.is_empty() {
                            ringbuf.push_slice(current_line.as_bytes());
                            current_line.clear();
                        } else {
                            // Empty line + ^D = signal EOF by pushing zero bytes.
                            // The reader will see 0 bytes read = EOF.
                            ringbuf.push(0).ok();
                        }
                        self.wait_queue.wake_all();
                        written_len += 1;
                        continue;
                    }

                    // Backspace / erase character.
                    if *ch == termios.c_cc[VERASE] || *ch == 0x7f {
                        if !current_line.is_empty() {
                            current_line.backspace();
                            callback(LineControl::Backspace);
                        }
                        written_len += 1;
                        continue;
                    }
                }

                match ch {
                    b'\r' if termios.icrnl() => {
                        current_line.insert(b'\n');
                        ringbuf.push_slice(current_line.as_bytes());
                        current_line.clear();
                        if termios.echo_enabled() {
                            callback(LineControl::Echo(b'\r'));
                            callback(LineControl::Echo(b'\n'));
                        }
                    }
                    b'\n' => {
                        current_line.insert(b'\n');
                        ringbuf.push_slice(current_line.as_bytes());
                        current_line.clear();
                        if termios.echo_enabled() {
                            callback(LineControl::Echo(b'\n'));
                        }
                    }
                    ch if termios.is_cooked_mode() => {
                        if 0x20 <= *ch && *ch < 0x7f {
                            current_line.insert(*ch);
                            if termios.echo_enabled() {
                                callback(LineControl::Echo(*ch));
                            }
                        }
                    }
                    _ => {
                        // In the raw mode.
                        ringbuf.push(*ch).ok();
                    }
                }

                written_len += 1;
            }
        }

        self.wait_queue.wake_all();
        Ok(written_len)
    }

    pub fn read(&self, dst: UserBufferMut<'_>) -> Result<usize> {
        let mut writer = UserBufWriter::from(dst);
        self.wait_queue.sleep_signalable_until(|| {
            if !self.is_current_foreground() {
                return Ok(None);
            }

            let mut buf_lock = self.buf.lock();
            while writer.remaining_len() > 0 {
                if let Some(slice) = buf_lock.pop_slice(writer.remaining_len()) {
                    writer.write_bytes(slice)?;
                } else {
                    break;
                }
            }

            if writer.written_len() > 0 {
                Ok(Some(writer.written_len()))
            } else {
                Ok(None)
            }
        })
    }
}
