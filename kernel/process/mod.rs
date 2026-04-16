// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use alloc::sync::Arc;

use kevlar_platform::{address::UserVAddr, spinlock::SpinLock};

use kevlar_utils::lazy::Lazy;
use kevlar_utils::once::Once;

mod cmdline;
mod elf;
mod init_stack;
#[allow(clippy::module_inception)]
mod process;
pub mod process_group;
mod scheduler;
pub mod signal;
mod switch;
pub mod wait_queue;

pub use process::{gc_exited_processes, list_pids, process_count, read_process_stats, PId, Process, ProcessState, EXITED_PROCESSES, VFORK_WAIT_QUEUE, GHOST_FORK_ENABLED, PREFAULT_TEMPLATE_ENABLED, DIRECT_MAP_ENABLED};
pub use scheduler::SchedulerPolicy;
pub use switch::switch;
pub use wait_queue::WaitQueue;

/// Returns true if no other processes are in any scheduler run queue.
/// Lock-free check via atomic counter — used by sched_yield fast path.
pub fn scheduler_is_empty() -> bool {
    scheduler::runqueue_len() == 0
}

use self::scheduler::Scheduler;

cpu_local! {
    static ref CURRENT: Lazy<Arc<Process>> = Lazy::new();
}

cpu_local! {
    // TODO: Should be pub(super)
    pub static ref IDLE_THREAD: Lazy<Arc<Process>> = Lazy::new();
}

static SCHEDULER: Once<SpinLock<Scheduler>> = Once::new();
pub static JOIN_WAIT_QUEUE: Once<WaitQueue> = Once::new();

pub fn current_process() -> &'static Arc<Process> {
    CURRENT.get()
}

/// Returns the current process if initialized, else None.
/// Safe to call during early boot before any process exists.
pub fn current_process_option() -> Option<&'static Arc<Process>> {
    CURRENT.get().try_get()
}

/// Returns the current PID if the process subsystem is initialized, else 0.
/// Used by ktrace to safely record events during early boot before any
/// process exists.
pub fn try_current_pid() -> i32 {
    // CURRENT is a cpu_local Lazy<Arc<Process>>. `.get()` returns
    // `&Lazy<Arc<Process>>`, then `.try_get()` checks if the Lazy is set.
    match CURRENT.get().try_get() {
        Some(p) => p.pid().as_i32(),
        None => 0,
    }
}

/// Task #25 diagnostic: snapshot each run queue (length + up to
/// `max` PIDs) by locking each queue briefly.  Returned as an owned
/// Vec so the caller can print it without holding any lock.
pub fn dump_scheduler_state(max: usize)
    -> alloc::vec::Vec<(usize, alloc::vec::Vec<PId>)>
{
    SCHEDULER.lock().snapshot(max)
}

pub fn init() {
    JOIN_WAIT_QUEUE.init(WaitQueue::new);
    VFORK_WAIT_QUEUE.init(WaitQueue::new);
    SCHEDULER.init(|| SpinLock::new_ranked(
        Scheduler::new(),
        kevlar_platform::lockdep::rank::SCHEDULER,
        "SCHEDULER",
    ));
    let idle_thread = Process::new_idle_thread().unwrap();
    IDLE_THREAD.as_mut().set(idle_thread.clone());
    CURRENT.as_mut().set(idle_thread);
}

/// Per-AP initialization: create the idle thread and set CURRENT.
/// Called from `ap_kernel_entry` after the BSP has completed `init()`.
pub fn init_ap() {
    let idle_thread = Process::new_idle_thread().unwrap();
    IDLE_THREAD.as_mut().set(idle_thread.clone());
    CURRENT.as_mut().set(idle_thread);
}
