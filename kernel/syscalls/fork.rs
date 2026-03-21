// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use crate::{
    process::{current_process, signal::SigSet, GHOST_FORK_ENABLED, Process, VFORK_WAIT_QUEUE},
    result::Result,
    syscalls::SyscallHandler,
};
use core::sync::atomic::Ordering;

impl<'a> SyscallHandler<'a> {
    pub fn sys_fork(&mut self) -> Result<isize> {
        let child = Process::fork(current_process(), self.frame)?;
        let child_pid = child.pid().as_i32() as isize;

        if GHOST_FORK_ENABLED.load(Ordering::Relaxed) {
            let current = current_process();
            // Block all signals during ghost-fork wait. Signals remain queued
            // and are delivered when fork() returns (via try_delivering_signal).
            let saved_mask = current.sigset_load();
            current.sigset_store(SigSet::ALL);

            let ncpus = kevlar_platform::arch::num_online_cpus();
            if ncpus > 1 {
                // SMP: spin-wait. The child runs on another CPU so spinning
                // doesn't block it. ~0 overhead vs sleep-based wait (~5µs).
                while !child.ghost_fork_done.load(Ordering::Acquire) {
                    core::hint::spin_loop();
                }
            } else {
                // UP: must sleep to let the child run on this CPU.
                while !child.ghost_fork_done.load(Ordering::Acquire) {
                    let _ = VFORK_WAIT_QUEUE.sleep_signalable_until(|| {
                        if child.ghost_fork_done.load(Ordering::Acquire) {
                            Ok(Some(()))
                        } else {
                            Ok(None)
                        }
                    });
                }
            }

            current.sigset_store(saved_mask);
            // Flush TLB to pick up restored writable PTEs from the ghost child.
            if let Some(vm_ref) = current.vm().as_ref() {
                vm_ref.lock().page_table().flush_tlb_all();
            }
        }

        Ok(child_pid)
    }
}
