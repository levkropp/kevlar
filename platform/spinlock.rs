// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use cfg_if::cfg_if;
use core::arch::asm;
use core::mem::ManuallyDrop;
use core::ops::{Deref, DerefMut};

use crate::arch::SavedInterruptStatus;

#[cfg(debug_assertions)]
use crate::backtrace::CapturedBacktrace;
#[cfg(debug_assertions)]
use atomic_refcell::AtomicRefCell;

pub struct SpinLock<T: ?Sized> {
    #[cfg(debug_assertions)]
    locked_by: AtomicRefCell<Option<CapturedBacktrace>>,
    inner: spin::mutex::SpinMutex<T>,
}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> SpinLock<T> {
        SpinLock {
            inner: spin::mutex::SpinMutex::new(value),
            #[cfg(debug_assertions)]
            locked_by: AtomicRefCell::new(None),
        }
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
            #[cfg(debug_assertions)]
            locked_by: &self.locked_by,
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
            #[cfg(debug_assertions)]
            locked_by: &self.locked_by,
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
    #[cfg(debug_assertions)]
    locked_by: &'a AtomicRefCell<Option<CapturedBacktrace>>,
    saved_intr_status: ManuallyDrop<SavedInterruptStatus>,
}

impl<'a, T: ?Sized> Drop for SpinLockGuard<'a, T> {
    fn drop(&mut self) {
        unsafe {
            ManuallyDrop::drop(&mut self.inner);
        }

        cfg_if! {
            if #[cfg(debug_assertions)] {
                *self.locked_by.borrow_mut() = None;
            }
        }

        unsafe {
            ManuallyDrop::drop(&mut self.saved_intr_status);
        }
    }
}

/// A lock guard that does NOT restore interrupt status on drop.
///
/// Created by [`SpinLock::lock_no_irq`].
pub struct SpinLockGuardNoIrq<'a, T: ?Sized> {
    inner: ManuallyDrop<spin::mutex::SpinMutexGuard<'a, T>>,
    #[cfg(debug_assertions)]
    locked_by: &'a AtomicRefCell<Option<CapturedBacktrace>>,
}

impl<'a, T: ?Sized> Drop for SpinLockGuardNoIrq<'a, T> {
    fn drop(&mut self) {
        unsafe {
            ManuallyDrop::drop(&mut self.inner);
        }

        cfg_if! {
            if #[cfg(debug_assertions)] {
                *self.locked_by.borrow_mut() = None;
            }
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
