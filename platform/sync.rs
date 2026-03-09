// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Safe wrappers for synchronization primitives that require unsafe.

use alloc::sync::Arc;

/// Decrement the strong reference count of an Arc without running the
/// destructor. This is useful during context switches where we need to
/// "forget" an Arc without triggering Drop (since the thread may not
/// return to the point after the switch).
///
/// # Panics
/// Debug-asserts that the strong count is > 1, ensuring the Arc won't
/// be deallocated by this operation.
pub fn arc_leak_one_ref<T>(arc: &Arc<T>) {
    debug_assert!(Arc::strong_count(arc) > 1);
    unsafe {
        Arc::decrement_strong_count(Arc::as_ptr(arc));
    }
}
