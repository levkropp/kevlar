// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! A deferred job queue.
//!
//! When you want to run some time-consuming work, please consider using this
//! mechanism.
use alloc::boxed::Box;
use alloc::vec::Vec;
use kevlar_api::sync::SpinLock;

pub trait JobCallback = FnOnce() + Send + 'static;
static GLOBAL_QUEUE: SpinLock<Vec<Box<dyn JobCallback>>> = SpinLock::new(Vec::new());

pub struct DeferredJob {
    #[allow(unused)]
    name: &'static str,
}

impl DeferredJob {
    pub const fn new(name: &'static str) -> DeferredJob {
        DeferredJob { name }
    }

    /// Enqueues a job. `callback` will be automatically run sometime later.
    ///
    /// # Caveats
    ///
    /// `callback` MUST NOT sleep since it's can be run in an interrupt context!
    pub fn run_later<F: JobCallback>(&self, callback: F) {
        GLOBAL_QUEUE.lock().push(Box::new(callback));
    }
}

/// Run pending deferred jobs.
pub fn run_deferred_jobs() {
    loop {
        let callback = {
            let mut queue = GLOBAL_QUEUE.lock();
            if queue.is_empty() {
                break;
            }
            queue.remove(0)
        };
        callback();
    }
}
