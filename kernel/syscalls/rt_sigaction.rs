// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::ctypes::*;
use crate::prelude::*;
use crate::process::current_process;
use crate::process::signal::{SigAction, DEFAULT_ACTIONS, SIG_DFL, SIG_IGN};
use crate::syscalls::SyscallHandler;
use kevlar_runtime::address::UserVAddr;

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
            oldact_ptr.write::<usize>(&handler_value)?;
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

            signals.lock().set_action(signum, new_action)?;
        }

        Ok(0)
    }
}
