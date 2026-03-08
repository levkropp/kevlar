// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Minimal futex implementation for M2 (single-threaded dynamic linking).
//! Reference: FreeBSD sys/compat/linux/linux_futex.c (BSD-2-Clause).
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use crate::process::wait_queue::WaitQueue;
use crate::result::{Errno, Result};
use crate::syscalls::SyscallHandler;
use kevlar_runtime::spinlock::SpinLock;

const FUTEX_WAIT: i32 = 0;
const FUTEX_WAKE: i32 = 1;
const FUTEX_PRIVATE_FLAG: i32 = 128;
const FUTEX_CMD_MASK: i32 = !(FUTEX_PRIVATE_FLAG);

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

impl<'a> SyscallHandler<'a> {
    pub fn sys_futex(
        &mut self,
        uaddr: usize,
        op: i32,
        val: u32,
        _timeout: usize,
        _uaddr2: usize,
        _val3: u32,
    ) -> Result<isize> {
        let cmd = op & FUTEX_CMD_MASK;

        match cmd {
            FUTEX_WAIT => {
                // Read the current value at uaddr.
                let current_val = unsafe { *(uaddr as *const u32) };
                if current_val != val {
                    return Err(Errno::EAGAIN.into());
                }

                // Sleep until woken. For single-threaded M2, this path is
                // rarely hit, but we implement it correctly for correctness.
                let queue = get_or_create_queue(uaddr);
                queue.sleep_signalable_until(|| {
                    // Re-check the value; if it changed, wake up.
                    let v = unsafe { *(uaddr as *const u32) };
                    if v != val {
                        Ok(Some(0isize))
                    } else {
                        Ok(None)
                    }
                })
            }
            FUTEX_WAKE => {
                // Wake up to `val` waiters. In the single-threaded case,
                // there are typically no waiters.
                let guard = FUTEX_QUEUES.lock();
                if let Some(ref map) = *guard {
                    if let Some(queue) = map.get(&uaddr).copied() {
                        queue.wake_all();
                    }
                }
                Ok(0)
            }
            _ => {
                debug_warn!("futex: unsupported op {:#x}", op);
                Err(Errno::ENOSYS.into())
            }
        }
    }
}
