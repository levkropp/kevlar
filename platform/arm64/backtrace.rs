// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::address::VAddr;
use core::arch::asm;

const BACKTRACE_MAX: usize = 16;

#[repr(C)]
pub struct StackFrame {
    next: *const StackFrame, // x29 (FP) of caller
    return_addr: u64,        // x30 (LR) of caller
}

pub struct Backtrace {
    frame: *const StackFrame,
}

impl Backtrace {
    pub fn current_frame() -> Backtrace {
        let fp: u64;
        unsafe { asm!("mov {}, x29", out(reg) fp) };
        Backtrace {
            frame: fp as *const StackFrame,
        }
    }

    pub fn traverse<F>(self, mut callback: F)
    where
        F: FnMut(usize, VAddr),
    {
        let mut frame = self.frame;
        for i in 0..BACKTRACE_MAX {
            if frame.is_null() || !VAddr::is_accessible_from_kernel(frame as usize) {
                break;
            }

            unsafe {
                callback(i, VAddr::new((*frame).return_addr as usize));
                frame = (*frame).next;
            }
        }
    }
}
