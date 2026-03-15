// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use core::arch::asm;
use core::mem::ManuallyDrop;
use core::ops::{Deref, DerefMut};

use crate::arch::SavedInterruptStatus;

pub struct SpinLock<T: ?Sized> {
    inner: spin::mutex::SpinMutex<T>,
}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> SpinLock<T> {
        SpinLock {
            inner: spin::mutex::SpinMutex::new(value),
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

impl<T: ?Sized> SpinLock<T> {
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        let saved_intr_status = SavedInterruptStatus::save();
        unsafe {
            #[cfg(target_arch = "x86_64")]
            asm!("cli");
            #[cfg(target_arch = "aarch64")]
            asm!("msr daifset, #2"); // Mask IRQs
        }

        let guard = self.inner.lock();

        SpinLockGuard {
            inner: ManuallyDrop::new(guard),
            saved_intr_status: ManuallyDrop::new(saved_intr_status),
        }
    }

    /// Acquires the lock **without** disabling interrupts (no cli/sti).
    ///
    /// Use for locks that are never accessed from interrupt context (e.g. fd
    /// tables, root_fs).  Saves the pushfq/cli/sti overhead on every
    /// acquire/release cycle.
    #[inline(always)]
    pub fn lock_no_irq(&self) -> SpinLockGuardNoIrq<'_, T> {
        let guard = self.inner.lock();

        SpinLockGuardNoIrq {
            inner: ManuallyDrop::new(guard),
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
        let guard = self.inner.lock();

        SpinLockGuardPreempt {
            inner: ManuallyDrop::new(guard),
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
}

impl<'a, T: ?Sized> Drop for SpinLockGuard<'a, T> {
    fn drop(&mut self) {
        unsafe {
            ManuallyDrop::drop(&mut self.inner);
            ManuallyDrop::drop(&mut self.saved_intr_status);
        }
    }
}

/// A lock guard that does NOT restore interrupt status on drop.
///
/// Created by [`SpinLock::lock_no_irq`].
pub struct SpinLockGuardNoIrq<'a, T: ?Sized> {
    inner: ManuallyDrop<spin::mutex::SpinMutexGuard<'a, T>>,
}

impl<'a, T: ?Sized> Drop for SpinLockGuardNoIrq<'a, T> {
    fn drop(&mut self) {
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
}

impl<'a, T: ?Sized> Drop for SpinLockGuardPreempt<'a, T> {
    fn drop(&mut self) {
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
