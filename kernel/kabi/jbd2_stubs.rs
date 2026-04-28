// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code)]
//! Linux jbd2 (journaling block device) stubs (K33 Phase 2).
//!
//! ext4 and ocfs2 use jbd2 for write-side journaling; even RO
//! mounts touch a few jbd2 entry points (journal_start to
//! validate, journal_get_undo_access for fast-commit replay).
//! K33 scope is RO mount of ext4 with `noload` on the cmdline,
//! which disables journal replay; the remaining jbd2 calls are
//! safely no-ops.
//!
//! v1 strategy:
//!
//!   * `journal_init_dev` / `journal_load` — return non-null
//!     "fake handle" pointer so the caller treats it as success.
//!   * `journal_destroy` — frees the fake handle (noop in v1
//!     since we statically allocate it).
//!   * `journal_start` / `journal_stop` /
//!     `journal_dirty_metadata` — no-op stubs.
//!
//! Symbols here only get linked when ext4.ko is loaded; erofs,
//! 9p, etc. don't depend on jbd2.

use core::ffi::{c_int, c_void};

use crate::ksym;

/// A fake `journal_t` we hand back to the fs.  ext4 stores some
/// per-handle state (transaction id, etc.) but if we return
/// success without actually starting transactions, nothing reads
/// past the initial validity check.  Reserve a page-sized region
/// of bss to be safe.
#[unsafe(no_mangle)]
static mut KABI_FAKE_JOURNAL: [u8; 4096] = [0u8; 4096];

#[unsafe(no_mangle)]
pub extern "C" fn jbd2_journal_init_dev(_bdev: *mut c_void,
                                        _fs_dev: *mut c_void,
                                        _start: u64, _len: c_int,
                                        _bsize: c_int) -> *mut c_void {
    log::warn!("kabi: jbd2_journal_init_dev (stub) — fake handle");
    let p = &raw const KABI_FAKE_JOURNAL as *const u8 as *mut c_void;
    p
}

#[unsafe(no_mangle)]
pub extern "C" fn jbd2_journal_load(_journal: *mut c_void) -> c_int {
    log::warn!("kabi: jbd2_journal_load (stub) — pretend success");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn jbd2_journal_destroy(_journal: *mut c_void) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn jbd2_journal_start(_journal: *mut c_void,
                                     _nblocks: c_int) -> *mut c_void {
    // Return a non-null sentinel so callers that check `IS_ERR(handle)`
    // treat this as success.
    let p = &raw const KABI_FAKE_JOURNAL as *const u8 as *mut c_void;
    p
}

#[unsafe(no_mangle)]
pub extern "C" fn jbd2_journal_stop(_handle: *mut c_void) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn jbd2_journal_dirty_metadata(_handle: *mut c_void,
                                              _bh: *mut c_void) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn jbd2_journal_get_write_access(_handle: *mut c_void,
                                                _bh: *mut c_void) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn jbd2_journal_set_features(_journal: *mut c_void,
                                            _compat: u32, _ro_compat: u32,
                                            _incompat: u32) -> c_int {
    0
}

ksym!(jbd2_journal_init_dev);
ksym!(jbd2_journal_load);
ksym!(jbd2_journal_destroy);
ksym!(jbd2_journal_start);
ksym!(jbd2_journal_stop);
ksym!(jbd2_journal_dirty_metadata);
ksym!(jbd2_journal_get_write_access);
ksym!(jbd2_journal_set_features);
