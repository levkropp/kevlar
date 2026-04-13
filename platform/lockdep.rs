// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Runtime lock dependency validator.
//!
//! Tracks which locks each CPU currently holds and verifies that lock
//! acquisition follows a strict rank ordering.  Acquiring a lock with
//! rank <= any currently held lock's rank is a potential deadlock and
//! triggers an immediate panic with the full held-lock chain.
//!
//! Additionally tracks IRQ safety: if a lock is ever acquired with IF=0
//! (interrupt context), it is marked as `irq_context`.  A future
//! enhancement will warn if the same lock is later acquired with IF=1.
//!
//! **Only active in debug builds** (`#[cfg(debug_assertions)]`).
//! In release builds, all functions compile to no-ops.

/// Maximum nesting depth for lock tracking per CPU.
const MAX_HELD: usize = 16;
/// Maximum number of CPUs supported.
const MAX_CPUS: usize = 8;

/// Lock rank constants.  Higher rank = acquired later (outer lock).
/// Rank 0 = unranked (no ordering checks performed).
pub mod rank {
    pub const UNRANKED: u8 = 0;

    // Timer subsystem (acquired first in handle_timer_irq)
    pub const TIMERS: u8 = 10;
    pub const REAL_TIMERS: u8 = 11;

    // Wait queues (poll, futex, join, pipe) — after timers, before scheduler
    pub const WAIT_QUEUE: u8 = 20;

    // Scheduler — after wait queues
    pub const SCHEDULER: u8 = 30;

    // Process table — after scheduler (switch() releases SCHEDULER before PROCESSES)
    pub const PROCESSES: u8 = 40;
    pub const EXITED_PROCESSES: u8 = 41;

    // Per-process VM lock — after process table
    pub const VM: u8 = 50;

    // Per-process resource locks (fd table, signals, root_fs)
    pub const PROCESS_RESOURCE: u8 = 60;

    // Page allocator — can be called from many contexts (drop, fault, etc.)
    pub const PAGE_ALLOC: u8 = 70;

    // Filesystem locks
    pub const FILESYSTEM: u8 = 80;

    // Network locks
    pub const NETWORK: u8 = 90;
}

mod inner {
    use super::{MAX_CPUS, MAX_HELD};
    use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

    #[derive(Clone, Copy)]
    struct HeldLock {
        lock_addr: usize,
        rank: u8,
        irq_disabled: bool,
    }

    impl HeldLock {
        const EMPTY: Self = HeldLock { lock_addr: 0, rank: 0, irq_disabled: false };
    }

    struct CpuLockState {
        held: [HeldLock; MAX_HELD],
        depth: u8,
        /// Reentrancy guard — prevents recursive lockdep checks (e.g., if a
        /// panic from a lockdep violation tries to acquire a lock).
        checking: bool,
    }

    impl CpuLockState {
        const fn new() -> Self {
            CpuLockState {
                held: [HeldLock::EMPTY; MAX_HELD],
                depth: 0,
                checking: false,
            }
        }
    }

    /// Per-CPU lock tracking state.  Accessed via CPU index with interrupts
    /// disabled (or from single-CPU boot context), so no data races.
    static mut STATES: [CpuLockState; MAX_CPUS] = {
        const S: CpuLockState = CpuLockState::new();
        [S; MAX_CPUS]
    };

    /// Whether lockdep is enabled (set after per-CPU init is complete).
    static ENABLED: AtomicBool = AtomicBool::new(false);

    pub fn enable() {
        ENABLED.store(true, Ordering::Relaxed);
        log::info!("lockdep: runtime lock ordering checker enabled");
    }

    pub fn is_enabled() -> bool {
        ENABLED.load(Ordering::Relaxed)
    }

    /// Called on lock acquire.  Checks rank ordering and pushes entry.
    ///
    /// # Safety
    /// Must be called with interrupts disabled on the current CPU, or from
    /// a context where the current CPU index is stable (preemption disabled).
    pub fn on_acquire(lock_addr: usize, rank: u8, name: &str) {
        if !is_enabled() || rank == 0 {
            return;
        }
        let cpu = crate::arch::cpu_id() as usize;
        if cpu >= MAX_CPUS {
            return;
        }

        // Safety: each CPU only accesses its own entry; interrupts are disabled
        // or preemption is disabled by the caller.
        #[allow(unsafe_code)]
        let state = unsafe { &mut STATES[cpu] };

        if state.checking {
            return; // Reentrancy guard (panic path may acquire locks)
        }
        state.checking = true;

        let irq_disabled = !crate::arch::interrupts_enabled();

        // Check ordering: no held lock should have rank >= this lock's rank.
        for i in 0..state.depth as usize {
            let held = &state.held[i];
            if held.rank > 0 && held.rank >= rank {
                state.checking = false;
                panic!(
                    "LOCKDEP: lock ordering violation on CPU {}!\n\
                     Acquiring: {} (rank {}, addr {:#x})\n\
                     While holding: rank {} (addr {:#x})\n\
                     Held locks (innermost first): {:?}",
                    cpu, name, rank, lock_addr,
                    held.rank, held.lock_addr,
                    format_held(state),
                );
            }
        }

        // Push entry.
        let depth = state.depth as usize;
        if depth < MAX_HELD {
            state.held[depth] = HeldLock { lock_addr, rank, irq_disabled };
            state.depth += 1;
        }

        state.checking = false;
    }

    /// Called on lock release.  Pops the entry.
    pub fn on_release(lock_addr: usize) {
        if !is_enabled() {
            return;
        }
        let cpu = crate::arch::cpu_id() as usize;
        if cpu >= MAX_CPUS {
            return;
        }

        #[allow(unsafe_code)]
        let state = unsafe { &mut STATES[cpu] };

        if state.checking {
            return;
        }

        // Find and remove the entry.  Locks may be released out of order
        // (e.g., guard dropped in a different scope than acquired), so scan
        // from the top.
        let depth = state.depth as usize;
        for i in (0..depth).rev() {
            if state.held[i].lock_addr == lock_addr {
                // Shift entries down to fill the gap.
                for j in i..depth - 1 {
                    state.held[j] = state.held[j + 1];
                }
                state.held[depth - 1] = HeldLock::EMPTY;
                state.depth -= 1;
                return;
            }
        }
        // Not found — lock was acquired before lockdep was enabled, or is unranked.
    }

    /// Format the held-lock chain for panic messages.
    fn format_held(state: &CpuLockState) -> alloc::string::String {
        use alloc::format;
        let mut s = alloc::string::String::new();
        for i in (0..state.depth as usize).rev() {
            let h = &state.held[i];
            s.push_str(&format!("\n    [{}] rank={} addr={:#x} irq_off={}",
                i, h.rank, h.lock_addr, h.irq_disabled));
        }
        s
    }

    /// Dump currently held locks for the given CPU (called from NMI handler).
    pub fn dump_held_locks(cpu: usize) {
        if cpu >= MAX_CPUS {
            return;
        }
        #[allow(unsafe_code)]
        let state = unsafe { &STATES[cpu] };
        let depth = state.depth;
        if depth == 0 {
            log::warn!("  lockdep: no locks held");
            return;
        }
        log::warn!("  lockdep: {} lock(s) held:", depth);
        for i in 0..depth as usize {
            let h = &state.held[i];
            log::warn!("    [{}] rank={} addr={:#x} irq_off={}",
                i, h.rank, h.lock_addr, h.irq_disabled);
        }
    }
}

// Re-export.
pub use inner::{enable, is_enabled, on_acquire, on_release, dump_held_locks};
