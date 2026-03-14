// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::ctypes::*;
use crate::prelude::*;
use crate::process::current_process;
use crate::process::signal::{SigAction, DEFAULT_ACTIONS, SIGCHLD, SIG_DFL, SIG_IGN};
use crate::syscalls::SyscallHandler;
use kevlar_platform::address::UserVAddr;

// Linux x86_64 kernel sigaction layout:
//   [0]:  sa_handler  (8 bytes)
//   [8]:  sa_flags    (8 bytes)
//   [16]: sa_restorer (8 bytes)
//   [24]: sa_mask     (8 bytes)
const SA_RESTORER: usize = 0x04000000;

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
                SigAction::Handler { handler, .. } => handler.value(),
            };
            oldact_ptr.write::<usize>(&handler_value)?;
        }

        if let Some(act) = UserVAddr::new(act) {
            // Read the entire sigaction struct in one usercopy (32 bytes)
            // instead of 3 separate reads.
            let raw: [usize; 4] = act.read::<[usize; 4]>()?;
            let handler = raw[0];
            let sa_flags = raw[1];
            let restorer = if sa_flags & SA_RESTORER != 0 {
                UserVAddr::new(raw[2])
            } else {
                None
            };

            let new_action = match handler {
                SIG_IGN => SigAction::Ignore,
                SIG_DFL => match DEFAULT_ACTIONS.get(signum as usize) {
                    Some(default_action) => *default_action,
                    None => return Err(Errno::EINVAL.into()),
                },
                _ => SigAction::Handler {
                    handler: UserVAddr::new(handler).ok_or_else(|| Error::new(Errno::EFAULT))?,
                    restorer,
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
