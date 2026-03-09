// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::ctypes::*;
use crate::debug;
use crate::prelude::*;
use crate::process::current_process;
use crate::process::signal::{SigAction, DEFAULT_ACTIONS, SIGCHLD, SIG_DFL, SIG_IGN};
use crate::syscalls::SyscallHandler;
use kevlar_platform::address::UserVAddr;

// Provenance: Own (POSIX sigaction(2) man page).
impl<'a> SyscallHandler<'a> {
    pub fn sys_rt_sigaction(
        &mut self,
        signum: c_int,
        act: usize,
        oldact: Option<UserVAddr>,
    ) -> Result<isize> {
        let signals = current_process().signals();

        // Return the old action before overwriting it.
        if let Some(oldact_ptr) = oldact {
            let old_action = signals.lock().get_action(signum);
            let handler_value: usize = match old_action {
                SigAction::Ignore => SIG_IGN,
                SigAction::Terminate | SigAction::Stop | SigAction::Continue => SIG_DFL,
                SigAction::Handler { handler } => handler.value(),
            };
            debug::usercopy::set_context("sys_rt_sigaction:oldact");
            let r = oldact_ptr.write::<usize>(&handler_value);
            debug::usercopy::clear_context();
            r?;
        }

        if let Some(act) = UserVAddr::new(act) {
            let handler = act.read::<usize>()?;
            let new_action = match handler {
                SIG_IGN => SigAction::Ignore,
                SIG_DFL => match DEFAULT_ACTIONS.get(signum as usize) {
                    Some(default_action) => *default_action,
                    None => return Err(Errno::EINVAL.into()),
                },
                _ => SigAction::Handler {
                    handler: UserVAddr::new(handler).ok_or_else(|| Error::new(Errno::EFAULT))?,
                },
            };

            let mut signals = signals.lock();
            signals.set_action(signum, new_action)?;

            // Track explicit SIG_IGN on SIGCHLD for auto-reap semantics.
            if signum == SIGCHLD {
                signals.set_nocldwait(handler == SIG_IGN);
            }
        }

        Ok(0)
    }
}
