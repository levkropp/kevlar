// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Breadcrumb logging for `.ko` instrumentation.
//!
//! Designed for the patcher pipeline in `tools/docker-kabi-modules/`:
//! when a Linux module gets patched with diagnostic calls (e.g.,
//! before each `goto failed_mount` in `ext4_fill_super`), each call
//! site emits a fixed-shape breadcrumb (`line`, `target_id`, `err`)
//! that the kernel records both in `log::info!` and in a small
//! ring buffer that downstream code can dump.
//!
//! The motivation for a dedicated helper (rather than reusing
//! `printk` directly) is two-fold:
//!
//!   1.  **Predictability.**  `printk` handles a wide format-string
//!       grammar with variadic args; subtle interactions between
//!       the grammar parser, our `VaList`, and ext4's `pr_err`
//!       wrapper have surfaced as misleading layout-sensitivity
//!       bugs in the past (Phase 12 v7).  Breadcrumb takes three
//!       fixed `i32` args and does no parsing.
//!
//!   2.  **Shape invariance.**  The breadcrumb call is encoded as a
//!       single `bl kabi_breadcrumb` followed by 3 register loads,
//!       so injecting many of them shifts code by a fixed amount
//!       per site and avoids surprises in nearby `.altinstructions`
//!       and `__bug_table` slot layout.
//!
//! Use from a patched `.ko`:
//!
//! ```c
//! /* Linux side — the patcher injects this verbatim. */
//! extern void kabi_breadcrumb(int line, int target_id, int err);
//! kabi_breadcrumb(__LINE__, KABI_GOTO_FAILED_MOUNT4A, err);
//! goto failed_mount4a;
//! ```
//!
//! And read the ring back from kABI Rust:
//!
//! ```rust
//! kabi::breadcrumb::dump();  /* → log::info!("kabi-bc: ...") per entry */
//! kabi::breadcrumb::clear();
//! ```

use core::sync::atomic::{AtomicUsize, Ordering};

use kevlar_platform::spinlock::SpinLock;

use crate::ksym;

#[derive(Clone, Copy, Default)]
pub struct Crumb {
    pub line: i32,
    pub target_id: i32,
    pub err: i32,
}

const RING_SIZE: usize = 256;

static RING: SpinLock<[Crumb; RING_SIZE]> =
    SpinLock::new([Crumb { line: 0, target_id: 0, err: 0 }; RING_SIZE]);
static HEAD: AtomicUsize = AtomicUsize::new(0);

/// Patched `.ko` callers invoke this at every diagnostic site.
/// Fixed 3-arg signature keeps the call shape predictable.
#[unsafe(no_mangle)]
pub extern "C" fn kabi_breadcrumb(line: i32, target_id: i32, err: i32) {
    let pos = HEAD.fetch_add(1, Ordering::AcqRel);
    let mut ring = RING.lock();
    ring[pos % RING_SIZE] = Crumb { line, target_id, err };
    drop(ring);
    log::info!(
        "kabi-bc: line={} target_id={} err={}", line, target_id, err,
    );
}

ksym!(kabi_breadcrumb);

/// Dump the current ring buffer to the log.  Useful right after
/// `fill_super` returns — captures the entire breadcrumb sequence
/// in chronological order.
pub fn dump() {
    let head = HEAD.load(Ordering::Acquire);
    let ring = RING.lock();
    let n = head.min(RING_SIZE);
    log::info!("kabi-bc: dumping {} breadcrumb(s) (head={}):", n, head);
    if head <= RING_SIZE {
        for i in 0..n {
            let c = ring[i];
            log::info!(
                "  [{}] line={} target_id={} err={}",
                i, c.line, c.target_id, c.err,
            );
        }
    } else {
        // Wrapped — print in chronological order from oldest entry.
        let start = head % RING_SIZE;
        for i in 0..RING_SIZE {
            let idx = (start + i) % RING_SIZE;
            let c = ring[idx];
            log::info!(
                "  [{}] line={} target_id={} err={}",
                head - RING_SIZE + i, c.line, c.target_id, c.err,
            );
        }
    }
}

/// Reset the ring (call before a new mount attempt).
pub fn clear() {
    HEAD.store(0, Ordering::Release);
    let mut ring = RING.lock();
    for c in ring.iter_mut() {
        *c = Crumb::default();
    }
}
