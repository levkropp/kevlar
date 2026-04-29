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
///
/// ASM offsets (hardcoded in `trap.S`, `usermode.S`):
///   0:   sp_el1
///   8:   sp_el0_save
///   16:  preempt_count
///   20:  need_resched
///   24:  fp_owner
///   32:  kabi_task_mock_ptr
/// Do NOT reorder the first four fields — asm accesses them by offset.
/// New fields must be appended at the end.
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
    /// PID of the task whose FP/NEON state is currently loaded in the v-regs
    /// on this CPU.  0 = no FP owner (v-regs are foreign / undefined).
    /// Mirror of Linux's per-CPU `fpsimd_last_state` pointer.  Used by the
    /// EC=0x07 FP-trap handler to decide whose state to save before loading
    /// the current task's.  Cleared on task exit if the exiting task owns
    /// the CPU's FP state.
    pub fp_owner: u64,
    /// Linux task_struct mock pointer for kABI-loaded fs `.ko` modules.
    /// Linux's compiled fs code uses `sp_el0` as a `task_struct *`; trap.S
    /// reads this field on every EL0→EL1 entry and `msr sp_el0, x` so kABI
    /// kernel-mode code sees a Linux-shaped current-task.  Zero on CPUs
    /// where `kabi::task_mock::install_for_current_cpu()` hasn't run yet —
    /// trap.S skips the mock-install in that case.  See
    /// `kernel/kabi/task_mock.rs`.
    pub kabi_task_mock_ptr: u64,
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
