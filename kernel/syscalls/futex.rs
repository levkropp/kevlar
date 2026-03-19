// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Futex implementation: WAIT, WAKE, CMP_REQUEUE, WAKE_OP, WAIT_BITSET.
//! Provenance: Own (Linux futex(2) man page).
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use crate::process::wait_queue::WaitQueue;
use crate::result::{Errno, Result};
use crate::syscalls::SyscallHandler;
use kevlar_platform::spinlock::SpinLock;

const FUTEX_WAIT: i32 = 0;
const FUTEX_WAKE: i32 = 1;
const FUTEX_CMP_REQUEUE: i32 = 4;
const FUTEX_WAKE_OP: i32 = 5;
const FUTEX_WAIT_BITSET: i32 = 9;
const FUTEX_WAKE_BITSET: i32 = 10;
const FUTEX_PRIVATE_FLAG: i32 = 128;
const FUTEX_CLOCK_REALTIME: i32 = 256;
const FUTEX_CMD_MASK: i32 = !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);

static FUTEX_QUEUES: SpinLock<Option<BTreeMap<usize, &'static WaitQueue>>> =
    SpinLock::new(None);

fn get_or_create_queue(addr: usize) -> &'static WaitQueue {
    let mut map = FUTEX_QUEUES.lock();
    let map = map.get_or_insert_with(BTreeMap::new);
    if let Some(q) = map.get(&addr) {
        return *q;
    }
    let q: &'static WaitQueue = Box::leak(Box::new(WaitQueue::new()));
    map.insert(addr, q);
    q
}

fn get_queue(addr: usize) -> Option<&'static WaitQueue> {
    let map = FUTEX_QUEUES.lock();
    map.as_ref().and_then(|m| m.get(&addr).copied())
}

// ── FUTEX_WAKE_OP encoding ──────────────────────────────────────────

/// Decode the val3 encoding for FUTEX_WAKE_OP.
/// Returns (op, cmp, oparg, cmparg).
fn decode_wake_op(val3: u32) -> (u32, u32, u32, u32) {
    let op = (val3 >> 28) & 0xF;
    let cmp = (val3 >> 24) & 0xF;
    let oparg = (val3 >> 12) & 0xFFF;
    let cmparg = val3 & 0xFFF;
    (op, cmp, oparg, cmparg)
}

/// Apply the FUTEX_WAKE_OP operation. Returns the old value.
fn apply_wake_op(addr: kevlar_platform::address::UserVAddr, op: u32, oparg: u32) -> Result<u32> {
    let old: u32 = addr.read()?;
    let new = match op {
        0 => oparg,              // FUTEX_OP_SET
        1 => old.wrapping_add(oparg), // FUTEX_OP_ADD
        2 => old | oparg,       // FUTEX_OP_OR
        3 => old & !oparg,      // FUTEX_OP_ANDN
        4 => old ^ oparg,       // FUTEX_OP_XOR
        _ => return Err(Errno::ENOSYS.into()),
    };
    addr.write(&new)?;
    Ok(old)
}

/// Evaluate the FUTEX_WAKE_OP comparison.
fn wake_op_cmp(cmp: u32, old: u32, cmparg: u32) -> bool {
    match cmp {
        0 => old == cmparg,      // FUTEX_OP_CMP_EQ
        1 => old != cmparg,      // FUTEX_OP_CMP_NE
        2 => old < cmparg,       // FUTEX_OP_CMP_LT
        3 => old <= cmparg,      // FUTEX_OP_CMP_LE
        4 => old > cmparg,       // FUTEX_OP_CMP_GT
        5 => old >= cmparg,      // FUTEX_OP_CMP_GE
        _ => false,
    }
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_futex(
        &mut self,
        uaddr: usize,
        op: i32,
        val: u32,
        timeout_or_val2: usize,
        uaddr2: usize,
        val3: u32,
    ) -> Result<isize> {
        let cmd = op & FUTEX_CMD_MASK;

        match cmd {
            FUTEX_WAIT | FUTEX_WAIT_BITSET => {
                // WAIT_BITSET with bitset=0 is invalid per Linux.
                if cmd == FUTEX_WAIT_BITSET && val3 == 0 {
                    return Err(Errno::EINVAL.into());
                }

                let user_addr = kevlar_platform::address::UserVAddr::new_nonnull(uaddr)?;
                let current_val: u32 = user_addr.read()?;
                if current_val != val {
                    return Err(Errno::EAGAIN.into());
                }

                let queue = get_or_create_queue(uaddr);
                queue.sleep_signalable_until(|| {
                    let v: u32 = user_addr.read().unwrap_or(0);
                    if v != val {
                        Ok(Some(0isize))
                    } else {
                        Ok(None)
                    }
                })
            }
            FUTEX_WAKE | FUTEX_WAKE_BITSET => {
                if let Some(queue) = get_queue(uaddr) {
                    Ok(queue.wake_n(val) as isize)
                } else {
                    Ok(0)
                }
            }
            FUTEX_CMP_REQUEUE => {
                let user_addr1 = kevlar_platform::address::UserVAddr::new_nonnull(uaddr)?;

                // Read current value and compare to val3.
                let current_val: u32 = user_addr1.read()?;
                if current_val != val3 {
                    return Err(Errno::EAGAIN.into());
                }

                let queue1 = get_or_create_queue(uaddr);
                let queue2 = get_or_create_queue(uaddr2);

                // Wake up to `val` waiters on uaddr1.
                let woken = queue1.wake_n(val);
                // Requeue up to `timeout_or_val2` (= val2) waiters from uaddr1 to uaddr2.
                let requeued = queue1.requeue_to(queue2, timeout_or_val2);
                Ok((woken as usize + requeued) as isize)
            }
            FUTEX_WAKE_OP => {
                let addr2 = kevlar_platform::address::UserVAddr::new_nonnull(uaddr2)?;
                let (wake_op, wake_cmp, oparg, cmparg) = decode_wake_op(val3);

                // Atomically read old value at uaddr2, apply operation, write new value.
                let old = apply_wake_op(addr2, wake_op, oparg)?;

                // Wake up to `val` waiters on uaddr1.
                let mut total = 0u32;
                if let Some(queue1) = get_queue(uaddr) {
                    total += queue1.wake_n(val);
                }

                // If old value passes comparison, wake up to val2 waiters on uaddr2.
                if wake_op_cmp(wake_cmp, old, cmparg) {
                    if let Some(queue2) = get_queue(uaddr2) {
                        total += queue2.wake_n(timeout_or_val2 as u32);
                    }
                }

                Ok(total as isize)
            }
            _ => {
                debug_warn!("futex: unsupported op {:#x}", op);
                Err(Errno::ENOSYS.into())
            }
        }
    }
}

/// Wake up to `n` waiters sleeping on the given kernel-virtual address.
/// Called from process exit for CLONE_CHILD_CLEARTID.
pub fn futex_wake_addr(addr: usize, n: u32) {
    if let Some(queue) = get_queue(addr) {
        if n == u32::MAX {
            queue.wake_all();
        } else {
            queue.wake_n(n);
        }
    }
}
