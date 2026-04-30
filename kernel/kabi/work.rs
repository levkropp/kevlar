// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux `work_struct` + workqueue shims for K2 modules.
//!
//! Modules see `struct work_struct` as:
//!
//! ```c
//! struct work_struct {
//!     void *_kevlar_inner;
//!     void (*func)(struct work_struct *);
//! };
//! ```
//!
//! K2 has a single global FIFO worker kthread (`kabi_workqueue`).
//! `schedule_work()` enqueues; the worker drains and invokes
//! `work->func(work)` one at a time, sleepable.  Linux's full
//! workqueue infrastructure (per-cpu queues, priorities,
//! `delayed_work`, `flush_workqueue`) defers to K3+.

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};

use kevlar_platform::spinlock::SpinLock;
use kevlar_utils::once::Once;

use crate::ksym;
use crate::process::wait_queue::WaitQueue;
use crate::process::Process;

static WORKER_INIT: AtomicBool = AtomicBool::new(false);

#[repr(C)]
pub struct WorkStructShim {
    pub inner: *mut WorkInner,
    pub func: Option<extern "C" fn(*mut WorkStructShim)>,
}

pub struct WorkInner {
    /// True if currently in the queue or running.  False when idle.
    pending: AtomicBool,
    /// Wait queue used by `flush_work` to block until the work
    /// transitions from running back to idle.
    flush_wq: WaitQueue,
}

impl WorkInner {
    fn new() -> Self {
        Self {
            pending: AtomicBool::new(false),
            flush_wq: WaitQueue::new(),
        }
    }
}

struct WorkerQueue {
    items: VecDeque<*mut WorkStructShim>,
}

// SAFETY: WorkStructShim pointers are only touched while the
// queue's spinlock is held; we send them to the worker thread.
#[allow(unsafe_code)]
unsafe impl Send for WorkerQueue {}

static WORKER_QUEUE: SpinLock<WorkerQueue> = SpinLock::new(WorkerQueue {
    items: VecDeque::new(),
});
static WORKER_WAKE: Once<WaitQueue> = Once::new();

/// Spawned exactly once at boot from `kabi::init()`.
static WORKER_TASK: Once<Arc<Process>> = Once::new();

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn kabi_init_work(
    w: *mut WorkStructShim,
    func: extern "C" fn(*mut WorkStructShim),
) {
    if w.is_null() {
        return;
    }
    let inner = alloc::boxed::Box::into_raw(alloc::boxed::Box::new(WorkInner::new()));
    unsafe {
        (*w).inner = inner;
        (*w).func = Some(func);
    }
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn schedule_work(w: *mut WorkStructShim) -> i32 {
    if w.is_null() {
        return 0;
    }
    let inner_ptr = unsafe { (*w).inner };
    if inner_ptr.is_null() {
        return 0;
    }
    let inner = unsafe { &*inner_ptr };
    // If already pending, no-op (matches Linux semantics).
    if inner.pending.swap(true, Ordering::AcqRel) {
        return 0;
    }
    WORKER_QUEUE.lock().items.push_back(w);
    if WORKER_INIT.load(Ordering::Acquire) {
        (&*WORKER_WAKE).wake_one();
    }
    1
}

/// Block until `w` is no longer queued or running.
///
/// Phase 12 (ext4 arc): noop.  ext4.ko's `INIT_WORK` initializes
/// the work_struct using Linux's struct layout, not our
/// `WorkStructShim` shape — so `(*w).inner` is garbage Linux
/// fields.  For RO-mount paths nothing actually runs work, so
/// flush is a noop.  Real impl needed when something queues +
/// awaits work in the kABI path.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn flush_work(_w: *mut WorkStructShim) -> i32 { 0 }

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn cancel_work_sync(w: *mut WorkStructShim) -> i32 {
    if w.is_null() {
        return 0;
    }
    // Try to remove from queue.
    let removed = {
        let mut q = WORKER_QUEUE.lock();
        let pos = q.items.iter().position(|&p| p == w);
        match pos {
            Some(i) => {
                q.items.remove(i);
                true
            }
            None => false,
        }
    };
    if removed {
        let inner_ptr = unsafe { (*w).inner };
        if !inner_ptr.is_null() {
            let inner = unsafe { &*inner_ptr };
            inner.pending.store(false, Ordering::Release);
            inner.flush_wq.wake_all();
        }
        return 1;
    }
    flush_work(w);
    0
}

ksym!(kabi_init_work);
ksym!(schedule_work);
ksym!(flush_work);
ksym!(cancel_work_sync);

/// Worker kthread entry.  Loops forever: pop work_struct from
/// queue, invoke `func(work)`, mark it idle, wake any flushers.
#[allow(unsafe_code)]
extern "C" fn worker_thread_entry() {
    log::info!("kabi: workqueue worker started (pid={})",
        crate::process::current_process().pid().as_i32());
    let wake_wq: &WaitQueue = &*WORKER_WAKE;
    loop {
        let work_ptr = {
            // sleep_signalable_until returns Some when there's work.
            let result = wake_wq.sleep_signalable_until(|| {
                let mut q = WORKER_QUEUE.lock();
                Ok(q.items.pop_front())
            });
            match result {
                Ok(p) => p,
                Err(_) => continue, // spurious signal — retry
            }
        };

        let func = unsafe { (*work_ptr).func };
        if let Some(f) = func {
            f(work_ptr);
        }

        // Mark idle + wake flushers AFTER func returns.
        let inner_ptr = unsafe { (*work_ptr).inner };
        if !inner_ptr.is_null() {
            let inner = unsafe { &*inner_ptr };
            inner.pending.store(false, Ordering::Release);
            inner.flush_wq.wake_all();
        }
    }
}

/// Spawn the singleton worker kthread.  Call exactly once at boot
/// after the scheduler is up.
pub fn init() {
    if WORKER_INIT.load(Ordering::Acquire) {
        return;
    }
    WORKER_WAKE.init(WaitQueue::new);
    let task = Process::new_kthread_with_entry("kabi_wq", worker_thread_entry)
        .expect("kabi: failed to spawn workqueue thread");
    WORKER_TASK.init(|| task);
    WORKER_INIT.store(true, Ordering::Release);
}
