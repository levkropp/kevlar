// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{ctypes::c_int, prelude::*};
use bitvec::prelude::*;
use kevlar_platform::address::UserVAddr;

pub type Signal = c_int;
#[allow(unused)]
pub const SIGHUP: Signal = 1;
#[allow(unused)]
pub const SIGINT: Signal = 2;
#[allow(unused)]
pub const SIGQUIT: Signal = 3;
#[allow(unused)]
pub const SIGILL: Signal = 4;
#[allow(unused)]
pub const SIGTRAP: Signal = 5;
#[allow(unused)]
pub const SIGABRT: Signal = 6;
#[allow(unused)]
pub const SIGBUS: Signal = 7;
#[allow(unused)]
pub const SIGFPE: Signal = 8;
#[allow(unused)]
pub const SIGKILL: Signal = 9;
#[allow(unused)]
pub const SIGUSR1: Signal = 10;
#[allow(unused)]
pub const SIGSEGV: Signal = 11;
#[allow(unused)]
pub const SIGUSR2: Signal = 12;
#[allow(unused)]
pub const SIGPIPE: Signal = 13;
#[allow(unused)]
pub const SIGALRM: Signal = 14;
#[allow(unused)]
pub const SIGTERM: Signal = 15;
#[allow(unused)]
pub const SIGSTKFLT: Signal = 16;
#[allow(unused)]
pub const SIGCHLD: Signal = 17;
#[allow(unused)]
pub const SIGCONT: Signal = 18;
#[allow(unused)]
pub const SIGSTOP: Signal = 19;
#[allow(unused)]
pub const SIGTSTP: Signal = 20;
#[allow(unused)]
pub const SIGTTIN: Signal = 21;
#[allow(unused)]
pub const SIGTTOU: Signal = 22;
#[allow(unused)]
pub const SIGURG: Signal = 23;
#[allow(unused)]
pub const SIGXCPU: Signal = 24;
#[allow(unused)]
pub const SIGXFSZ: Signal = 25;
#[allow(unused)]
pub const SIGVTALRM: Signal = 26;
#[allow(unused)]
pub const SIGPROF: Signal = 27;
#[allow(unused)]
pub const SIGWINCH: Signal = 28;
#[allow(unused)]
pub const SIGIO: Signal = 29;
#[allow(unused)]
pub const SIGPWR: Signal = 30;
#[allow(unused)]
pub const SIGSYS: Signal = 31;

const SIGMAX: c_int = 32;

pub const SIG_DFL: usize = 0;
pub const SIG_IGN: usize = 1;

#[derive(Clone, Copy, PartialEq)]
pub enum SigAction {
    Ignore,
    Terminate,
    Stop,
    Continue,
    Handler { handler: UserVAddr },
}

// Default signal dispositions per POSIX / signal(7).
// Provenance: Own (POSIX standard, signal(7) man page).
pub const DEFAULT_ACTIONS: [SigAction; SIGMAX as usize] = [
    /* (unused)  */ SigAction::Ignore,
    /* SIGHUP    */ SigAction::Terminate,
    /* SIGINT    */ SigAction::Terminate,
    /* SIGQUIT   */ SigAction::Terminate,
    /* SIGILL    */ SigAction::Terminate,
    /* SIGTRAP   */ SigAction::Terminate,
    /* SIGABRT   */ SigAction::Terminate,
    /* SIGBUS    */ SigAction::Terminate,
    /* SIGFPE    */ SigAction::Terminate,
    /* SIGKILL   */ SigAction::Terminate,
    /* SIGUSR1   */ SigAction::Terminate,
    /* SIGSEGV   */ SigAction::Terminate,
    /* SIGUSR2   */ SigAction::Terminate,
    /* SIGPIPE   */ SigAction::Terminate,
    /* SIGALRM   */ SigAction::Terminate,
    /* SIGTERM   */ SigAction::Terminate,
    /* SIGSTKFLT */ SigAction::Terminate,
    /* SIGCHLD   */ SigAction::Ignore,
    /* SIGCONT   */ SigAction::Continue,
    /* SIGSTOP   */ SigAction::Stop,
    /* SIGTSTP   */ SigAction::Stop,
    /* SIGTTIN   */ SigAction::Stop,
    /* SIGTTOU   */ SigAction::Stop,
    /* SIGURG    */ SigAction::Ignore,
    /* SIGXCPU   */ SigAction::Terminate,
    /* SIGXFSZ   */ SigAction::Terminate,
    /* SIGVTALRM */ SigAction::Terminate,
    /* SIGPROF   */ SigAction::Terminate,
    /* SIGWINCH  */ SigAction::Ignore,
    /* SIGIO     */ SigAction::Terminate,
    /* SIGPWR    */ SigAction::Terminate,
    /* SIGSYS    */ SigAction::Terminate,
];

pub struct SignalDelivery {
    pending: u32,
    actions: [SigAction; SIGMAX as usize],
    /// True when the user explicitly called `sigaction(SIGCHLD, SIG_IGN)` or
    /// set `SA_NOCLDWAIT`.  This enables auto-reaping of child zombies.
    /// The default SIGCHLD disposition (`Ignore`) does NOT set this flag.
    nocldwait: bool,
}

impl SignalDelivery {
    pub fn new() -> SignalDelivery {
        SignalDelivery {
            pending: 0,
            actions: DEFAULT_ACTIONS,
            nocldwait: false,
        }
    }

    pub fn get_action(&self, signal: Signal) -> SigAction {
        self.actions[signal as usize]
    }

    /// Whether the process has explicitly requested auto-reaping of children
    /// (via `sigaction(SIGCHLD, SIG_IGN)` or `SA_NOCLDWAIT`).
    pub fn nocldwait(&self) -> bool {
        self.nocldwait
    }

    pub fn set_action(&mut self, signal: Signal, action: SigAction) -> Result<()> {
        if signal > SIGMAX {
            return Err(Errno::EINVAL.into());
        }

        self.actions[signal as usize] = action;
        Ok(())
    }

    /// Called from `rt_sigaction` when SIGCHLD disposition changes.
    pub fn set_nocldwait(&mut self, value: bool) {
        self.nocldwait = value;
    }

    pub fn is_pending(&self) -> bool {
        self.pending != 0
    }

    pub fn pop_pending(&mut self) -> Option<(Signal, SigAction)> {
        if self.pending == 0 {
            return None;
        }

        let signal = self.pending.trailing_zeros();
        self.pending &= !(1 << signal);
        Some((signal as Signal, self.actions[signal as usize]))
    }

    pub fn signal(&mut self, signal: Signal) {
        self.pending |= 1 << (signal);
    }

    /// Reset signal dispositions for execve: per POSIX, all signals with
    /// handler functions are reset to SIG_DFL.  SIG_IGN dispositions are
    /// preserved.  Pending signals are preserved.
    pub fn reset_on_exec(&mut self) {
        for i in 0..SIGMAX as usize {
            if matches!(self.actions[i], SigAction::Handler { .. }) {
                self.actions[i] = DEFAULT_ACTIONS[i];
            }
        }
        self.nocldwait = false;
    }
}

//pub type SigSet = BitMap<128 /* 1024 / 8 */>;
pub type SigSet = BitArray<[u8; 1024 / 8], LocalBits>;
pub enum SignalMask {
    Block,
    Unblock,
    Set,
}
