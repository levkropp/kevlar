// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Per-CPU variables using TPIDR_EL1 as the base pointer.
use crate::address::VAddr;
use core::arch::asm;
use core::mem::MaybeUninit;
use core::ptr;

#[macro_export]
macro_rules! __cpu_local_impl {
    ($V:vis, $N:ident, $T:ty, $E:expr) => {
        #[allow(non_camel_case_types)]
        #[allow(clippy::upper_case_acronyms)]
        pub struct $N {
            #[allow(unused)]
            initial_value: $T,
        }

        impl $N {
            #[allow(unused)]
            $V fn get(&self) -> &$T {
                self.as_mut()
            }

            #[allow(unused)]
            $V fn set(&self, value: $T) {
                *self.as_mut() = value;
            }

            #[allow(unused)]
            #[allow(clippy::mut_from_ref)]
            $V fn as_mut(&self) -> &mut $T {
                unsafe { &mut *self.vaddr().as_mut_ptr() }
            }

            #[allow(unused)]
            $V fn vaddr(&self) -> $crate::address::VAddr {
                unsafe extern "C" {
                    static __cpu_local: u8;
                }

                unsafe {
                    let cpu_local_base = &__cpu_local as *const _ as usize;
                    let offset = (self as *const _ as usize) - cpu_local_base;
                    let tpidr: usize;
                    core::arch::asm!("mrs {}, tpidr_el1", out(reg) tpidr);
                    $crate::address::VAddr::new(tpidr + offset)
                }
            }
        }

        #[used]
        #[unsafe(link_section = ".cpu_local")]
        $V static $N: $N = $N { initial_value: $E };
        unsafe impl Sync for $N {}
    };
}

#[macro_export]
macro_rules! cpu_local {
    (static ref $N:ident : $T:ty = $E:expr ;) => {
        __cpu_local_impl!(, $N, $T, $E);
    };
    (pub static ref $N:ident : $T:ty = $E:expr ;) => {
        __cpu_local_impl!(pub, $N, $T, $E);
    };
}

/// The cpu-local structure at the beginning of TPIDR_EL1.
#[repr(C)]
pub struct CpuLocalHead {
    /// The kernel stack pointer for syscall/exception entry.
    pub sp_el1: u64,
    /// Temporary save space for the user stack pointer.
    pub sp_el0_save: u64,
    /// Preemption disable count.  Incremented at the start of `switch()` and
    /// decremented after `do_switch_thread` returns.  The timer preemption
    /// handler skips `process::switch()` while this is non-zero, preventing
    /// nested context switches on the same CPU.
    pub preempt_count: u32,
    /// Set by the timer handler when preemption is disabled but a context
    /// switch is due.  Checked by `preempt_enable()` to reschedule immediately.
    /// Matches Linux's TIF_NEED_RESCHED.
    pub need_resched: u32,
}

#[used]
#[unsafe(link_section = ".cpu_local_head")]
static CPU_LOCAL_HEAD_SPACE: MaybeUninit<CpuLocalHead> = MaybeUninit::uninit();

pub fn cpu_local_head() -> &'static mut CpuLocalHead {
    let tpidr: usize;
    unsafe {
        asm!("mrs {}, tpidr_el1", out(reg) tpidr);
        &mut *(tpidr as *mut CpuLocalHead)
    }
}

pub unsafe fn init(cpu_local_area: VAddr) {
    unsafe extern "C" {
        static __cpu_local: u8;
        static __cpu_local_size: u8;
    }

    let template = VAddr::new(&__cpu_local as *const _ as usize);
    let len = &__cpu_local_size as *const _ as usize;
    ptr::copy_nonoverlapping::<u8>(template.as_ptr(), cpu_local_area.as_mut_ptr(), len);

    asm!("msr tpidr_el1, {}", in(reg) cpu_local_area.value());
}
