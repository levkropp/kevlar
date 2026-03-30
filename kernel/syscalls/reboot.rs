// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use kevlar_platform::arch::halt;

use crate::{ctypes::c_int, result::Result, syscalls::SyscallHandler};

const LINUX_REBOOT_CMD_CAD_OFF: usize = 0x0;
const LINUX_REBOOT_CMD_CAD_ON: usize = 0x89abcdef;
const LINUX_REBOOT_CMD_RESTART: usize = 0x01234567;
const LINUX_REBOOT_CMD_HALT: usize = 0xcdef0123;
const LINUX_REBOOT_CMD_POWER_OFF: usize = 0x4321fedc;

impl<'a> SyscallHandler<'a> {
    pub fn sys_reboot(&mut self, _magic: c_int, _magic2: c_int, cmd: usize) -> Result<isize> {
        match cmd {
            LINUX_REBOOT_CMD_CAD_OFF | LINUX_REBOOT_CMD_CAD_ON => {
                // Enable/disable Ctrl-Alt-Del: accept silently.
                Ok(0)
            }
            LINUX_REBOOT_CMD_RESTART | LINUX_REBOOT_CMD_HALT | LINUX_REBOOT_CMD_POWER_OFF => {
                // Dump profiler/tracer data before halting.
                if crate::debug::profiler::is_enabled() {
                    crate::debug::profiler::dump_syscall_profile(
                        crate::syscalls::syscall_name_by_number,
                    );
                }
                if crate::debug::tracer::is_enabled() {
                    crate::debug::tracer::dump_span_profile();
                }
                // Sync all filesystems before halting to prevent data loss.
                info!("Syncing filesystems before halt...");
                let _ = kevlar_ext2::sync_all();
                info!("Halting the system by reboot(2) cmd={:#x}", cmd);
                halt();
            }
            _ => {
                warn!("reboot: unknown command {:#x}", cmd);
                Ok(0)
            }
        }
    }
}
