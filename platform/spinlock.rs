// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use core::arch::asm;
use core::mem::ManuallyDrop;
use core::ops::{Deref, DerefMut};

use crate::arch::SavedInterruptStatus;
use crate::lockdep;
#[cfg(target_arch = "x86_64")]
use crate::x64::if_trace;

pub struct SpinLock<T: ?Sized> {
    /// Lock rank for dependency checking (0 = unranked, higher = acquired later).
    rank: u8,
    /// Human-readable name for lockdep violation messages.
    name: &'static str,
    /// The actual spin mutex.  Must be the LAST field so ?Sized T works.
    inner: spin::mutex::SpinMutex<T>,
}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> SpinLock<T> {
        SpinLock {
            inner: spin::mutex::SpinMutex::new(value),
            rank: 0,
            name: "<unnamed>",
        }
    }

    /// Create a lock with a rank and name for lock dependency checking.
    /// See `lockdep::rank` for rank constants.  Higher rank = acquired later.
    pub const fn new_ranked(value: T, rank: u8, name: &'static str) -> SpinLock<T> {
        SpinLock {
            inner: spin::mutex::SpinMutex::new(value),
            rank,
            name,
        }
    }

    /// Access the inner data without locking.
    ///
    /// # Safety
    /// Caller must guarantee no concurrent access (e.g., the containing
    /// Arc has strong_count == 1, or the lock is otherwise uncontended).
    #[allow(unsafe_code)]
    pub unsafe fn get_unchecked(&self) -> &T {
        &*self.inner.as_mut_ptr()
    }
}

/// Threshold: if a spin exceeds this many iterations, print a
/// one-shot contention warning with the lock name + CPU.
const SPIN_CONTENTION_THRESHOLD: u32 = 5_000_000;

impl<T: ?Sized> SpinLock<T> {
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        let saved_intr_status = SavedInterruptStatus::save();
        unsafe {
            #[cfg(target_arch = "x86_64")]
            asm!("cli");
            #[cfg(target_arch = "aarch64")]
            asm!("msr daifset, #2"); // Mask IRQs
        }

        let lock_addr = (self as *const Self).cast::<()>() as usize;

        // IF trace: record the cli (IF 1→0 transition).
        #[cfg(target_arch = "x86_64")]
        if_trace::record(if_trace::IfEvent::LockAcquire, lock_addr as u32, false);

        // Lockdep check: verify rank ordering BEFORE acquiring the spin.
        // Interrupts are already disabled, so CPU index is stable.
        lockdep::on_acquire(lock_addr, self.rank, self.name);

        // Try-lock with a contention counter.  If the lock is held for
        // more than SPIN_CONTENTION_THRESHOLD iterations, print a
        // warning — this catches the timer-death scenario where both
        // CPUs spin with IF=0 on the same lock for seconds.
        let guard = if let Some(g) = self.inner.try_lock() {
            g
        } else {
            let mut spins: u32 = 0;
            loop {
                if let Some(g) = self.inner.try_lock() {
                    break g;
                }
                core::hint::spin_loop();
                spins += 1;
                if spins == SPIN_CONTENTION_THRESHOLD {
                    // Re-enable interrupts briefly so we can print
                    // and so the NMI/timer can fire.
                    unsafe {
                        #[cfg(target_arch = "x86_64")]
                        asm!("sti");
                    }
                    let cpu = crate::arch::cpu_id();
                    log::warn!(
                        "SPIN_CONTENTION: cpu={} lock={:?} addr={:#x} spins={}",
                        cpu, self.name, lock_addr, spins,
                    );
                    unsafe {
                        #[cfg(target_arch = "x86_64")]
                        asm!("cli");
                    }
                }
            }
        };

        SpinLockGuard {
            inner: ManuallyDrop::new(guard),
            saved_intr_status: ManuallyDrop::new(saved_intr_status),
            lock_addr,
        }
    }

    /// Acquires the lock **without** disabling interrupts (no cli/sti).
    ///
    /// Use for locks that are never accessed from interrupt context (e.g. fd
    /// tables, root_fs).  Saves the pushfq/cli/sti overhead on every
    /// acquire/release cycle.
    #[inline(always)]
    pub fn lock_no_irq(&self) -> SpinLockGuardNoIrq<'_, T> {
        let lock_addr = (self as *const Self).cast::<()>() as usize;
        lockdep::on_acquire(lock_addr, self.rank, self.name);

        let guard = if let Some(g) = self.inner.try_lock() {
            g
        } else {
            let mut spins: u32 = 0;
            loop {
                if let Some(g) = self.inner.try_lock() {
                    break g;
                }
                core::hint::spin_loop();
                spins += 1;
                if spins == SPIN_CONTENTION_THRESHOLD {
                    let cpu = crate::arch::cpu_id();
                    #[cfg(target_arch = "x86_64")]
                    let if_state = {
                        let flags: u64;
                        unsafe { asm!("pushfq; pop {}", out(reg) flags, options(nomem, preserves_flags)); }
                        (flags >> 9) & 1
                    };
                    #[cfg(not(target_arch = "x86_64"))]
                    let if_state = 0u64;
                    log::warn!(
                        "SPIN_CONTENTION(no_irq): cpu={} lock={:?} addr={:#x} spins={} caller_IF={}",
                        cpu, self.name, lock_addr, spins, if_state,
                    );
                }
            }
        };

        SpinLockGuardNoIrq {
            inner: ManuallyDrop::new(guard),
            lock_addr,
        }
    }

    /// Acquires the lock with preemption disabled but interrupts still enabled.
    ///
    /// Use for locks held across `flush_tlb` / TLB shootdown.  The lock-holder
    /// keeps IF=1 so that remote CPUs can receive and ACK the TLB shootdown IPI
    /// that the lock-holder sends.  At the same time, preemption is disabled so
    /// that the timer interrupt handler cannot call `switch()` on THIS CPU while
    /// the lock is held — that would deadlock (switch tries to re-acquire the
    /// same SpinMutex on the same CPU).
    ///
    /// On drop the SpinMutex is released first, then preemption is re-enabled,
    /// ensuring the invariant that preempt_count > 0 ⟺ lock is held.
    #[inline(always)]
    pub fn lock_preempt(&self) -> SpinLockGuardPreempt<'_, T> {
        crate::arch::preempt_disable();
        lockdep::on_acquire((self as *const Self).cast::<()>() as usize, self.rank, self.name);

        let guard = self.inner.lock();

        SpinLockGuardPreempt {
            inner: ManuallyDrop::new(guard),
            lock_addr: (self as *const Self).cast::<()>() as usize,
        }
    }

    pub fn is_locked(&self) -> bool {
        self.inner.is_locked()
    }
}

unsafe impl<T: ?Sized + Send> Sync for SpinLock<T> {}
unsafe impl<T: ?Sized + Send> Send for SpinLock<T> {}

pub struct SpinLockGuard<'a, T: ?Sized> {
    inner: ManuallyDrop<spin::mutex::SpinMutexGuard<'a, T>>,
    saved_intr_status: ManuallyDrop<SavedInterruptStatus>,
    lock_addr: usize,
}

impl<'a, T: ?Sized> Drop for SpinLockGuard<'a, T> {
    fn drop(&mut self) {
        lockdep::on_release(self.lock_addr);
        unsafe {
            ManuallyDrop::drop(&mut self.inner);
            ManuallyDrop::drop(&mut self.saved_intr_status);
        }
        // IF trace: record IF restoration (may go IF=0→0 or IF=0→1).
        #[cfg(target_arch = "x86_64")]
        {
            let if_now = crate::arch::interrupts_enabled();
            if_trace::record(if_trace::IfEvent::LockRelease, self.lock_addr as u32, if_now);
        }
    }
}

/// A lock guard that does NOT restore interrupt status on drop.
///
/// Created by [`SpinLock::lock_no_irq`].
pub struct SpinLockGuardNoIrq<'a, T: ?Sized> {
    inner: ManuallyDrop<spin::mutex::SpinMutexGuard<'a, T>>,
    lock_addr: usize,
}

impl<'a, T: ?Sized> Drop for SpinLockGuardNoIrq<'a, T> {
    fn drop(&mut self) {
        lockdep::on_release(self.lock_addr);
        unsafe {
            ManuallyDrop::drop(&mut self.inner);
        }
    }
}

impl<'a, T: ?Sized> Deref for SpinLockGuardNoIrq<'a, T> {
    type Target = T;
    #[inline(always)]
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<'a, T: ?Sized> DerefMut for SpinLockGuardNoIrq<'a, T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

/// A lock guard that keeps interrupts enabled but disables preemption.
///
/// Created by [`SpinLock::lock_preempt`].
pub struct SpinLockGuardPreempt<'a, T: ?Sized> {
    inner: ManuallyDrop<spin::mutex::SpinMutexGuard<'a, T>>,
    lock_addr: usize,
}

impl<'a, T: ?Sized> Drop for SpinLockGuardPreempt<'a, T> {
    fn drop(&mut self) {
        lockdep::on_release(self.lock_addr);
        unsafe {
            ManuallyDrop::drop(&mut self.inner);
        }
        crate::arch::preempt_enable();
    }
}

impl<'a, T: ?Sized> Deref for SpinLockGuardPreempt<'a, T> {
    type Target = T;
    #[inline(always)]
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<'a, T: ?Sized> DerefMut for SpinLockGuardPreempt<'a, T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<'a, T: ?Sized> Deref for SpinLockGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<'a, T: ?Sized> DerefMut for SpinLockGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}
