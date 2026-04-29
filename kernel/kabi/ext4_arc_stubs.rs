// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code, non_upper_case_globals, non_camel_case_types)]
//! ext4-arc Phase 9 stubs.
//!
//! `mbcache.ko` → `jbd2.ko` → `ext4.ko` references ~115 kernel symbols
//! that aren't yet exposed via `ksym!()`.  This file collects no-op
//! stubs for the subset that an RO-mount path with `noload` (skip
//! journal replay) doesn't actually need to do anything.  Real impls
//! land as the bring-up loop hits each one.
//!
//! Style: one declaration per line where possible; group by Linux
//! subsystem to make later refactors painless.
use core::ffi::c_void;
use crate::ksym;

// ── Wait queue / scheduler primitives ──────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn __init_waitqueue_head(
    _wq: *mut c_void, _name: *const u8, _key: *mut c_void,
) {}

#[unsafe(no_mangle)]
pub extern "C" fn __wait_on_buffer(_bh: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn autoremove_wake_function(
    _wait: *mut c_void, _mode: u32, _sync: i32, _key: *mut c_void,
) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn prepare_to_wait(
    _q: *mut c_void, _wait: *mut c_void, _state: i32,
) {}

#[unsafe(no_mangle)]
pub extern "C" fn prepare_to_wait_exclusive(
    _q: *mut c_void, _wait: *mut c_void, _state: i32,
) {}

#[unsafe(no_mangle)]
pub extern "C" fn wake_bit_function(
    _wait: *mut c_void, _mode: u32, _sync: i32, _arg: *mut c_void,
) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn bit_waitqueue(_word: *mut c_void, _bit: i32) -> *mut c_void {
    static FAKE: u64 = 0;
    &raw const FAKE as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "C" fn bit_wait_io(_wait: *mut c_void, _mode: i32) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn out_of_line_wait_on_bit(
    _word: *mut c_void, _bit: i32, _action: *mut c_void, _mode: i32,
) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn __cond_resched_lock(_lock: *mut c_void) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn __refrigerator(_check_kthr_stop: i32) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn set_freezable() -> i32 { 0 }

#[unsafe(no_mangle)]
pub static freezer_active: u32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn freezing_slow_path(_p: *mut c_void) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn schedule_hrtimeout(
    _expires: *mut c_void, _mode: i32,
) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn kthread_create_on_node(
    _threadfn: *mut c_void, _data: *mut c_void, _node: i32,
    _namefmt: *const u8, _arg5: usize,
) -> *mut c_void {
    // Return ERR_PTR(-EAGAIN); jbd2 falls back gracefully.
    (-11isize) as *mut c_void
}

// ── Spin / rwlock primitives — noops because the kABI runs single-
//    threaded against fs ko's during RO mount ────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn _raw_read_lock(_lock: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn _raw_read_unlock(_lock: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn _raw_write_lock(_lock: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn _raw_write_unlock(_lock: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn mutex_is_locked(_lock: *mut c_void) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn mutex_lock_io(_lock: *mut c_void) {}

// ── Time / timers ──────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn add_timer(_timer: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn timer_delete_sync(_timer: *mut c_void) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn timer_init_key(
    _timer: *mut c_void, _func: *mut c_void, _flags: u32,
    _name: *const u8, _key: *mut c_void,
) {}

#[unsafe(no_mangle)]
pub static jiffies: u64 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn ktime_get() -> i64 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn ktime_get_coarse_real_ts64(ts: *mut c_void) {
    if !ts.is_null() {
        unsafe { core::ptr::write_bytes(ts as *mut u8, 0, 16); }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn round_jiffies_up(j: u64) -> u64 { j }

// ── Buffer-head + block layer (Phase 10 will land real impls) ─────

#[unsafe(no_mangle)]
pub extern "C" fn __bforget(_bh: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn __bh_read(
    _bh: *mut c_void, _op_flags: u32, _wait: i32,
) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn __bh_read_batch(
    _nr: i32, _bhs: *mut *mut c_void, _op_flags: u32, _wait: i32,
) {}

#[unsafe(no_mangle)]
pub extern "C" fn __brelse(_bh: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn __lock_buffer(_bh: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn __find_get_block_nonatomic(
    _bdev: *mut c_void, _block: u64, _size: u32,
) -> *mut c_void { core::ptr::null_mut() }

#[unsafe(no_mangle)]
pub extern "C" fn alloc_buffer_head(_gfp: u32) -> *mut c_void {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn bdev_getblk(
    _bdev: *mut c_void, _block: u64, _size: u32, _gfp: u32,
) -> *mut c_void { core::ptr::null_mut() }

#[unsafe(no_mangle)]
pub extern "C" fn bh_uptodate_or_lock(_bh: *mut c_void) -> i32 { 1 }

#[unsafe(no_mangle)]
pub extern "C" fn blk_finish_plug(_plug: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn blk_start_plug(_plug: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn blkdev_issue_discard(
    _bdev: *mut c_void, _sector: u64, _nr_sects: u64, _gfp: u32,
) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn blkdev_issue_flush(_bdev: *mut c_void) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn blkdev_issue_zeroout(
    _bdev: *mut c_void, _sector: u64, _nr_sects: u64,
    _gfp: u32, _flags: u32,
) -> i32 { 0 }

#[unsafe(no_mangle)]
pub static blockdev_superblock: u64 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn bmap(_inode: *mut c_void, _block: *mut u64) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn end_buffer_write_sync(_bh: *mut c_void, _uptodate: i32) {}

#[unsafe(no_mangle)]
pub extern "C" fn folio_set_bh(
    _bh: *mut c_void, _folio: *mut c_void, _offset: u64,
) {}

#[unsafe(no_mangle)]
pub extern "C" fn free_buffer_head(_bh: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn mark_buffer_dirty(_bh: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn submit_bh(_op: u32, _bh: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn sync_blockdev(_bdev: *mut c_void) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn try_to_free_buffers(_folio: *mut c_void) -> bool { false }

#[unsafe(no_mangle)]
pub extern "C" fn unlock_buffer(_bh: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn write_dirty_buffer(
    _bh: *mut c_void, _op_flags: u32,
) -> i32 { 0 }

// ── Page management (alloc/free pages) ─────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn free_pages(_addr: u64, _order: u32) {}

#[unsafe(no_mangle)]
pub extern "C" fn get_free_pages_noprof(
    _gfp: u32, _order: u32,
) -> u64 { 0 }

// ── Per-CPU counters ───────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn __percpu_counter_init_many(
    _fbc: *mut c_void, _amount: i64, _gfp: u32,
    _nr_counters: u32, _key: *mut c_void,
) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn percpu_counter_add_batch(
    _fbc: *mut c_void, _amount: i64, _batch: i32,
) {}

#[unsafe(no_mangle)]
pub static percpu_counter_batch: i32 = 32;

#[unsafe(no_mangle)]
pub extern "C" fn percpu_counter_destroy_many(
    _fbc: *mut c_void, _nr_counters: u32,
) {}

// ── Procfs (jbd2 creates /proc/fs/jbd2/<dev>) ──────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn proc_create_data(
    _name: *const u8, _mode: u32, _parent: *mut c_void,
    _proc_ops: *mut c_void, _data: *mut c_void,
) -> *mut c_void { core::ptr::null_mut() }

#[unsafe(no_mangle)]
pub extern "C" fn proc_mkdir(
    _name: *const u8, _parent: *mut c_void,
) -> *mut c_void { core::ptr::null_mut() }

#[unsafe(no_mangle)]
pub extern "C" fn remove_proc_entry(_name: *const u8, _parent: *mut c_void) {}

// ── seq_file (used by procfs entries; we never read them) ─────────

#[unsafe(no_mangle)]
pub extern "C" fn seq_lseek(
    _f: *mut c_void, _off: i64, _whence: i32,
) -> i64 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn seq_open(
    _f: *mut c_void, _ops: *mut c_void,
) -> i32 { -38 } // -ENOSYS

#[unsafe(no_mangle)]
pub extern "C" fn seq_read(
    _f: *mut c_void, _buf: *mut u8, _size: usize, _ppos: *mut i64,
) -> isize { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn seq_release(_inode: *mut c_void, _f: *mut c_void) -> i32 { 0 }

// ── filemap (writeback/wait paths — RO mount never writes) ────────

#[unsafe(no_mangle)]
pub extern "C" fn filemap_fdatawait_range_keep_errors(
    _mapping: *mut c_void, _start: i64, _end: i64,
) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn filemap_fdatawrite_range(
    _mapping: *mut c_void, _start: i64, _end: i64,
) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn truncate_inode_pages_range(
    _mapping: *mut c_void, _lstart: i64, _lend: i64,
) {}

// ── Misc primitives ────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn ___ratelimit(
    _rs: *mut c_void, _func: *const u8,
) -> i32 { 1 }

/// CRC-32 (big-endian, ANSI X3.66) used by jbd2 for journal block
/// checksums.  Stub: return 0 — works only because RO+`noload`
/// skips journal replay.
#[unsafe(no_mangle)]
pub extern "C" fn crc32_be(_seed: u32, _buf: *const u8, _len: usize) -> u32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn errseq_check(
    _eseq: *mut c_void, _since: u32,
) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn errseq_check_and_advance(
    _eseq: *mut c_void, _since: *mut u32,
) -> i32 { 0 }

#[unsafe(no_mangle)]
pub extern "C" fn strreplace(s: *mut u8, _old: i32, _new: i32) -> *mut u8 { s }

// ── ksym!() declarations ───────────────────────────────────────────

ksym!(__init_waitqueue_head);
ksym!(__wait_on_buffer);
ksym!(autoremove_wake_function);
ksym!(prepare_to_wait);
ksym!(prepare_to_wait_exclusive);
ksym!(wake_bit_function);
ksym!(bit_waitqueue);
ksym!(bit_wait_io);
ksym!(out_of_line_wait_on_bit);
ksym!(__cond_resched_lock);
ksym!(__refrigerator);
ksym!(set_freezable);
crate::ksym_static!(freezer_active);
ksym!(freezing_slow_path);
ksym!(schedule_hrtimeout);
ksym!(kthread_create_on_node);
ksym!(_raw_read_lock);
ksym!(_raw_read_unlock);
ksym!(_raw_write_lock);
ksym!(_raw_write_unlock);
ksym!(mutex_is_locked);
ksym!(mutex_lock_io);
ksym!(add_timer);
ksym!(timer_delete_sync);
ksym!(timer_init_key);
crate::ksym_static!(jiffies);
ksym!(ktime_get);
ksym!(ktime_get_coarse_real_ts64);
ksym!(round_jiffies_up);
ksym!(__bforget);
ksym!(__bh_read);
ksym!(__bh_read_batch);
ksym!(__brelse);
ksym!(__lock_buffer);
ksym!(__find_get_block_nonatomic);
ksym!(alloc_buffer_head);
ksym!(bdev_getblk);
ksym!(bh_uptodate_or_lock);
ksym!(blk_finish_plug);
ksym!(blk_start_plug);
ksym!(blkdev_issue_discard);
ksym!(blkdev_issue_flush);
ksym!(blkdev_issue_zeroout);
crate::ksym_static!(blockdev_superblock);
ksym!(bmap);
ksym!(end_buffer_write_sync);
ksym!(folio_set_bh);
ksym!(free_buffer_head);
ksym!(mark_buffer_dirty);
ksym!(submit_bh);
ksym!(sync_blockdev);
ksym!(try_to_free_buffers);
ksym!(unlock_buffer);
ksym!(write_dirty_buffer);
ksym!(free_pages);
ksym!(get_free_pages_noprof);
ksym!(__percpu_counter_init_many);
ksym!(percpu_counter_add_batch);
crate::ksym_static!(percpu_counter_batch);
ksym!(percpu_counter_destroy_many);
ksym!(proc_create_data);
ksym!(proc_mkdir);
ksym!(remove_proc_entry);
ksym!(seq_lseek);
ksym!(seq_open);
ksym!(seq_read);
ksym!(seq_release);
ksym!(filemap_fdatawait_range_keep_errors);
ksym!(filemap_fdatawrite_range);
ksym!(truncate_inode_pages_range);
ksym!(___ratelimit);
ksym!(crc32_be);
ksym!(errseq_check);
ksym!(errseq_check_and_advance);
ksym!(strreplace);
