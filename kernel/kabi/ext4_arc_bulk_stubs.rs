// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![allow(unsafe_code, non_upper_case_globals, non_camel_case_types)]
//! ext4-arc Phase 9 bulk stubs (auto-generated from UNDEF list).
//!
//! 261 symbols ext4.ko references that aren't yet covered by
//! kernel ksym!() exports, runtime exports from mbcache/jbd2,
//! or ext4_arc_stubs.rs.  Each is a no-op or null-returning
//! placeholder so ext4.ko can complete relocation pass and
//! init_module can call into the few that are actually used at
//! init time (mostly kmem_cache_create / register_filesystem).
//! Phase 10/11 will replace the ones called during mount.
use core::ffi::c_void;
use crate::ksym;

/// Small non-null heap allocation.  Phase 10: many ext4_init paths
/// `cbz x0, error` on the result of an "alloc/create" stub; returning
/// a 64-byte zeroed buffer instead of NULL clears that gate.  The
/// returned object isn't a real Linux struct, but during init most
/// callers just store the pointer and don't deref the contents.
fn fake_alloc() -> *mut c_void {
    super::alloc::kmalloc(64, super::alloc::__GFP_ZERO)
}

#[unsafe(no_mangle)] pub extern "C" fn __arch_copy_from_user(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn __blk_crypto_submit_bio(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn __bread_gfp(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn __dquot_alloc_space(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn __dquot_free_space(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn __dquot_transfer(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn __filemap_set_wb_err(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn __find_get_block(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn __flush_workqueue(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn __folio_batch_release(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn __folio_start_writeback(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn __get_random_u32_below(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn __mark_inode_dirty(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub static __per_cpu_offset: u64 = 0;
#[unsafe(no_mangle)] pub extern "C" fn __percpu_counter_sum(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn __percpu_down_read(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn __percpu_init_rwsem(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn __remove_inode_hash(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn __seq_puts(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn __task_pid_nr_ns(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn __xa_insert(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn _find_next_zero_bit(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn _raw_write_trylock(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn arch_timer_read_counter(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn bdev_file_open_by_dev(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { fake_alloc() }
#[unsafe(no_mangle)] pub extern "C" fn bdev_fput(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn bdev_freeze(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn bdev_thaw(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn bio_associate_blkg_from_css(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn blk_status_to_errno(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn block_commit_write(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn block_dirty_folio(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn block_invalidate_folio(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn block_is_partially_uptodate(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn block_page_mkwrite(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn block_read_full_folio(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn block_write_end(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn buffer_migrate_folio(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn buffer_migrate_folio_norefs(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn clear_nlink(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn crc16(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn create_empty_buffers(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn d_alloc(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { fake_alloc() }
#[unsafe(no_mangle)] pub extern "C" fn d_drop(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn d_find_any_alias(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn d_instantiate(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn d_instantiate_new(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { fake_alloc() }
#[unsafe(no_mangle)] pub extern "C" fn d_mark_dontcache(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn d_path(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn d_tmpfile(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dget_parent(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn down_read_trylock(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn down_write_trylock(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dput(
    dentry: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void {
    log::info!("kabi-trace: dput(dentry=0x{:x})", dentry);
    core::ptr::null_mut()
}
#[unsafe(no_mangle)] pub extern "C" fn dqget(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dqput(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_acquire(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_alloc(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { fake_alloc() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_alloc_inode(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { fake_alloc() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_claim_space_nodirty(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_commit(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_commit_info(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_destroy(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_disable(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_drop(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_file_open(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_free_inode(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_get_dqblk(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_get_next_dqblk(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_get_next_id(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_get_state(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_initialize(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_initialize_needed(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_load_quota_inode(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_mark_dquot_dirty(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_quota_off(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_quota_on(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_quota_on_mount(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_quota_sync(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_quotactl_sysfile_ops(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_reclaim_space_nodirty(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_release(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_resume(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_set_dqblk(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_set_dqinfo(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_transfer(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dquot_writeback_dquots(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn drop_nlink(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn dump_stack(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
// end_buffer_read_sync — Phase 12 v6: real impl in ext4_arc_stubs.rs.
#[unsafe(no_mangle)] pub extern "C" fn errseq_set(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn fdget(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn fiemap_fill_next_extent(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn fiemap_prep(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn file_check_and_advance_wb_err(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn file_modified(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn file_path(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn file_update_time(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn file_write_and_wait_range(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn fileattr_fill_flags(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn filemap_dirty_folio(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn filemap_fault(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn filemap_flush(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn filemap_flush_range(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn filemap_get_folios(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn filemap_get_folios_tag(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn filemap_map_pages(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn filemap_release_folio(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn filemap_write_and_wait_range(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn find_inode_by_ino_rcu(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn finish_open(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { fake_alloc() }
#[unsafe(no_mangle)] pub extern "C" fn folio_clear_dirty_for_io(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn folio_end_writeback(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn folio_mark_dirty(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn folio_mkclean(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn folio_redirty_for_writepage(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn folio_wait_stable(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn folio_wait_writeback(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn folio_zero_new_buffers(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn free_percpu(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn from_kgid(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn from_kgid_munged(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn from_kprojid(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn from_kuid(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn from_kuid_munged(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn from_vfsgid(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn from_vfsuid(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn fs_holder_ops(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn fs_lookup_param(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn fs_overflowgid(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn fs_overflowuid(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn fs_param_is_blockdev(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn fs_param_is_gid(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn fs_param_is_s32(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn fs_param_is_u32(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn fs_param_is_uid(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn fserror_report(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn generate_random_uuid(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn generic_atomic_write_valid(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn generic_buffers_fsync_noflush(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn generic_check_addressable(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn generic_encode_ino32_fh(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn generic_error_remove_folio(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn generic_fh_to_dentry(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn generic_fh_to_parent(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn generic_file_llseek_size(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
/// Phase 13: real `generic_file_read_iter`.  ext4_file_read_iter
/// dispatches buffered (non-DIRECT) reads to this; we route them
/// straight into our `filemap_read` impl which handles the
/// page-cache loop + a_ops->read_folio dispatch.  Direct-IO is not
/// yet supported.
#[unsafe(no_mangle)]
pub extern "C" fn generic_file_read_iter(
    iocb: *mut c_void, iter: *mut c_void,
) -> isize {
    super::filemap::filemap_read(iocb, iter, 0)
}
#[unsafe(no_mangle)] pub extern "C" fn generic_fill_statx_atomic_writes(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn generic_perform_write(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn generic_set_sb_d_ops(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn generic_write_checks(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn get_inode_acl(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn get_random_u16(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn get_random_u32(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
// get_tree_bdev — Phase 11: real impl in kabi/fs_synth.rs.
// iget_locked — Phase 12 v3: dispatch through sb->s_op->alloc_inode
// so per-fs allocators (ext4_alloc_inode → struct ext4_inode_info
// with vfs_inode at offset +0x110) produce inodes shaped to the
// caller's expectations.  ext4's check_igot_inode reads
// `[inode, -192]`, which only points at valid memory if vfs_inode
// is embedded inside ext4_inode_info.
//
//   sb_arg   = first arg (x0) = struct super_block *
//   ino_arg  = second arg (x1) = inode number (unsigned long)
//
// `struct super_operations.alloc_inode` is the FIRST function pointer
// (offset 0) per linux-7.0 include/linux/fs/super_types.h:83.
#[unsafe(no_mangle)]
pub extern "C" fn iget_locked(
    sb: *mut c_void, ino: u64,
    _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void {
    use super::struct_layouts as fl;
    if sb.is_null() { return core::ptr::null_mut(); }

    // Try sb->s_op->alloc_inode(sb) first.
    let s_op = unsafe {
        *(sb.cast::<u8>().add(fl::SB_S_OP_OFF) as *const usize)
    };
    let inode = if s_op != 0 {
        let alloc_inode_ptr = unsafe { *(s_op as *const usize) };
        if alloc_inode_ptr != 0 {
            let raw = super::loader::call_with_scs_1(
                alloc_inode_ptr as *const (), sb as usize,
            ) as *mut c_void;
            log::info!("kabi: iget_locked: s_op->alloc_inode(sb={:p}) → {:p}",
                       sb, raw);
            if raw.is_null() {
                return core::ptr::null_mut();
            }
            raw
        } else {
            // alloc_inode unset; fall back to plain inode kmalloc.
            super::alloc::kmalloc(fl::INODE_SIZE, super::alloc::__GFP_ZERO)
        }
    } else {
        super::alloc::kmalloc(fl::INODE_SIZE, super::alloc::__GFP_ZERO)
    };
    if inode.is_null() { return inode; }

    unsafe {
        // Populate vfs_inode fields ext4_iget reads before
        // ext4_get_inode_loc fills the rest from disk.
        *(inode.cast::<u8>().add(fl::INODE_I_SB_OFF)
            as *mut *mut c_void) = sb;
        *(inode.cast::<u8>().add(fl::INODE_I_INO_OFF) as *mut u64) = ino;
        *(inode.cast::<u8>().add(fl::INODE_I_BLKBITS_OFF) as *mut u8) = 12;
        // Mark I_NEW so caller (ext4_iget) knows to read on-disk
        // inode + populate fields, rather than treating as cached.
        const I_NEW: u32 = 1 << 0;
        *(inode.cast::<u8>().add(144) as *mut u32) = I_NEW;
        // Default i_nlink = 1 so check_igot_inode's "i_nlink == 0
        // → special inode unallocated" sanity check doesn't reject
        // before ext4 populates it from disk.
        *(inode.cast::<u8>().add(72) as *mut u32) = 1;
    }

    // Phase 13: allocate a per-inode address_space and point
    // inode->i_mapping at it.  Real Linux's `inode_init_always`
    // (called from `alloc_inode` between sb->s_op->alloc_inode and
    // return) sets `inode->i_mapping = &inode->i_data`.  We don't
    // call inode_init_always; without this, ext4_set_aops writes to
    // `inode->i_mapping->a_ops` = NULL+104 = fault.  Filling
    // i_mapping with a heap-allocated address_space gives ext4 a
    // valid target.  host=inode so read_cache_folio sees the
    // inode pointer when looking up KabiInodeMeta or a_ops.
    let per_inode_mapping = super::alloc::kzalloc(
        fl::AS_SIZE, super::alloc::__GFP_ZERO,
    );
    if !per_inode_mapping.is_null() {
        unsafe {
            *(per_inode_mapping.cast::<u8>().add(fl::AS_HOST_OFF)
                as *mut *mut c_void) = inode;
            *(inode.cast::<u8>().add(fl::INODE_I_MAPPING_OFF)
                as *mut *mut c_void) = per_inode_mapping;
        }
    }
    inode
}
#[unsafe(no_mangle)] pub extern "C" fn igrab(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn ihold(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn in_group_p(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn inc_nlink(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub static init_uts_ns: u64 = 0;
#[unsafe(no_mangle)] pub extern "C" fn inode_dio_wait(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn inode_init_owner(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn inode_io_list_del(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn inode_maybe_inc_iversion(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn inode_needs_sync(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn inode_newsize_ok(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn inode_owner_or_capable(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn inode_query_iversion(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn inode_set_ctime_current(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn inode_set_flags(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn insert_inode_locked(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn invalidate_bdev(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn invalidate_inode_buffers(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn io_schedule_timeout(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn iocb_bio_iopoll(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn iomap_swapfile_activate(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn iov_iter_alignment(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn is_bad_inode(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn iter_file_splice_write(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn kfree_link(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
// kobject_create_and_add — Phase 10: moved to kabi/kobject.rs to
// return a real (non-null) kobject so ext4_init_sysfs proceeds.
#[unsafe(no_mangle)] pub extern "C" fn kstrtouint(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn kthread_should_stop(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn kthread_stop(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn ktime_get_real_seconds(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn kvfree_call_rcu(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn list_sort(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn lock_two_nondirectories(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn make_bad_inode(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn make_kprojid(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn make_vfsgid(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn make_vfsuid(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn mark_buffer_dirty_inode(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn memcpy_and_pad(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn mempool_alloc_slab(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn mempool_create_node_noprof(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { fake_alloc() }
#[unsafe(no_mangle)] pub extern "C" fn mempool_destroy(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn mempool_free(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn mempool_free_slab(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn memweight(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn mnt_drop_write_file(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn mnt_want_write_file(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn mod_timer(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn nop_mnt_idmap(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn nsecs_to_jiffies(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn pagecache_isize_extended(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn panic(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn path_put(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn pcpu_alloc_noprof(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { fake_alloc() }
#[unsafe(no_mangle)] pub extern "C" fn percpu_down_write(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn percpu_free_rwsem(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn percpu_up_write(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn posix_acl_alloc(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { fake_alloc() }
#[unsafe(no_mangle)] pub extern "C" fn posix_acl_chmod(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn posix_acl_create(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn posix_acl_update_mode(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn preempt_schedule_notrace(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn print_hex_dump(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn proc_create_seq_private(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { fake_alloc() }
#[unsafe(no_mangle)] pub extern "C" fn proc_create_single_data(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { fake_alloc() }
// `rb_next` is now real — see `kernel/kabi/rbtree.rs`.
#[unsafe(no_mangle)] pub extern "C" fn rcuwait_wake_up(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn release_dentry_name_snapshot(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn remove_proc_subtree(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
// sb_min_blocksize — Phase 11: ext4_fill_super calls this with min
// size 1024 to set the initial sb->s_blocksize before reading the
// on-disk superblock.  Returns the chosen blocksize on success, 0
// on failure.  Just delegate to sb_set_blocksize with the requested
// minimum.
#[unsafe(no_mangle)] pub extern "C" fn sb_min_blocksize(
    sb: *mut c_void, size: i32,
) -> i32 {
    super::block::sb_set_blocksize(sb, size)
}
#[unsafe(no_mangle)] pub extern "C" fn schedule_timeout_interruptible(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn schedule_timeout_uninterruptible(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn security_inode_init_security(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn seq_escape_mem(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn seq_putc(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
// set_blocksize — Phase 11: bdev-side blocksize setter.  Called by
// ext4_fill_super before sb_bread reads the real on-disk superblock
// at the chosen block size.  Returns 0 on success.
#[unsafe(no_mangle)] pub extern "C" fn set_blocksize(
    _bdev: *mut c_void, _size: i32,
) -> i32 { 0 }
#[unsafe(no_mangle)] pub extern "C" fn set_cached_acl(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn set_task_ioprio(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn setattr_copy(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn setattr_prepare(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn simple_inode_init_ts(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { fake_alloc() }
#[unsafe(no_mangle)] pub extern "C" fn sort(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn strchr(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn strsep(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn sync_dirty_buffer(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn sync_filesystem(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn sync_inode_metadata(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn sync_mapping_buffers(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn sysfs_notify(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn system_dfl_wq(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub static system_state: u64 = 0;
#[unsafe(no_mangle)] pub extern "C" fn tag_pages_for_writeback(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn take_dentry_name_snapshot(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn timer_shutdown_sync(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn touch_atime(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn truncate_inode_pages(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn truncate_pagecache(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn truncate_pagecache_range(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn try_to_writeback_inodes_sb(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn unlock_two_nondirectories(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn vfs_fsync_range(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn wbc_account_cgroup_owner(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn writeback_iter(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn xa_destroy(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }
#[unsafe(no_mangle)] pub extern "C" fn xa_erase(
    _: usize, _: usize, _: usize, _: usize, _: usize, _: usize,
) -> *mut c_void { core::ptr::null_mut() }

// ── ksym!() declarations ──────────────────────────────────────
ksym!(__arch_copy_from_user);
ksym!(__blk_crypto_submit_bio);
ksym!(__bread_gfp);
ksym!(__dquot_alloc_space);
ksym!(__dquot_free_space);
ksym!(__dquot_transfer);
ksym!(__filemap_set_wb_err);
ksym!(__find_get_block);
ksym!(__flush_workqueue);
ksym!(__folio_batch_release);
ksym!(__folio_start_writeback);
ksym!(__get_random_u32_below);
ksym!(__mark_inode_dirty);
crate::ksym_static!(__per_cpu_offset);
ksym!(__percpu_counter_sum);
ksym!(__percpu_down_read);
ksym!(__percpu_init_rwsem);
ksym!(__remove_inode_hash);
ksym!(__seq_puts);
ksym!(__task_pid_nr_ns);
ksym!(__xa_insert);
ksym!(_find_next_zero_bit);
ksym!(_raw_write_trylock);
ksym!(arch_timer_read_counter);
ksym!(bdev_file_open_by_dev);
ksym!(bdev_fput);
ksym!(bdev_freeze);
ksym!(bdev_thaw);
ksym!(bio_associate_blkg_from_css);
ksym!(blk_status_to_errno);
ksym!(block_commit_write);
ksym!(block_dirty_folio);
ksym!(block_invalidate_folio);
ksym!(block_is_partially_uptodate);
ksym!(block_page_mkwrite);
ksym!(block_read_full_folio);
ksym!(block_write_end);
ksym!(buffer_migrate_folio);
ksym!(buffer_migrate_folio_norefs);
ksym!(clear_nlink);
ksym!(crc16);
ksym!(create_empty_buffers);
ksym!(d_alloc);
ksym!(d_drop);
ksym!(d_find_any_alias);
ksym!(d_instantiate);
ksym!(d_instantiate_new);
ksym!(d_mark_dontcache);
ksym!(d_path);
ksym!(d_tmpfile);
ksym!(dget_parent);
ksym!(down_read_trylock);
ksym!(down_write_trylock);
ksym!(dput);
ksym!(dqget);
ksym!(dqput);
ksym!(dquot_acquire);
ksym!(dquot_alloc);
ksym!(dquot_alloc_inode);
ksym!(dquot_claim_space_nodirty);
ksym!(dquot_commit);
ksym!(dquot_commit_info);
ksym!(dquot_destroy);
ksym!(dquot_disable);
ksym!(dquot_drop);
ksym!(dquot_file_open);
ksym!(dquot_free_inode);
ksym!(dquot_get_dqblk);
ksym!(dquot_get_next_dqblk);
ksym!(dquot_get_next_id);
ksym!(dquot_get_state);
ksym!(dquot_initialize);
ksym!(dquot_initialize_needed);
ksym!(dquot_load_quota_inode);
ksym!(dquot_mark_dquot_dirty);
ksym!(dquot_quota_off);
ksym!(dquot_quota_on);
ksym!(dquot_quota_on_mount);
ksym!(dquot_quota_sync);
ksym!(dquot_quotactl_sysfile_ops);
ksym!(dquot_reclaim_space_nodirty);
ksym!(dquot_release);
ksym!(dquot_resume);
ksym!(dquot_set_dqblk);
ksym!(dquot_set_dqinfo);
ksym!(dquot_transfer);
ksym!(dquot_writeback_dquots);
ksym!(drop_nlink);
ksym!(dump_stack);
// ksym!(end_buffer_read_sync) — registered in ext4_arc_stubs.rs
ksym!(errseq_set);
ksym!(fdget);
ksym!(fiemap_fill_next_extent);
ksym!(fiemap_prep);
ksym!(file_check_and_advance_wb_err);
ksym!(file_modified);
ksym!(file_path);
ksym!(file_update_time);
ksym!(file_write_and_wait_range);
ksym!(fileattr_fill_flags);
ksym!(filemap_dirty_folio);
ksym!(filemap_fault);
ksym!(filemap_flush);
ksym!(filemap_flush_range);
ksym!(filemap_get_folios);
ksym!(filemap_get_folios_tag);
ksym!(filemap_map_pages);
ksym!(filemap_release_folio);
ksym!(filemap_write_and_wait_range);
ksym!(find_inode_by_ino_rcu);
ksym!(finish_open);
ksym!(folio_clear_dirty_for_io);
ksym!(folio_end_writeback);
ksym!(folio_mark_dirty);
ksym!(folio_mkclean);
ksym!(folio_redirty_for_writepage);
ksym!(folio_wait_stable);
ksym!(folio_wait_writeback);
ksym!(folio_zero_new_buffers);
ksym!(free_percpu);
ksym!(from_kgid);
ksym!(from_kgid_munged);
ksym!(from_kprojid);
ksym!(from_kuid);
ksym!(from_kuid_munged);
ksym!(from_vfsgid);
ksym!(from_vfsuid);
ksym!(fs_holder_ops);
ksym!(fs_lookup_param);
ksym!(fs_overflowgid);
ksym!(fs_overflowuid);
ksym!(fs_param_is_blockdev);
ksym!(fs_param_is_gid);
ksym!(fs_param_is_s32);
ksym!(fs_param_is_u32);
ksym!(fs_param_is_uid);
ksym!(fserror_report);
ksym!(generate_random_uuid);
ksym!(generic_atomic_write_valid);
ksym!(generic_buffers_fsync_noflush);
ksym!(generic_check_addressable);
ksym!(generic_encode_ino32_fh);
ksym!(generic_error_remove_folio);
ksym!(generic_fh_to_dentry);
ksym!(generic_fh_to_parent);
ksym!(generic_file_llseek_size);
ksym!(generic_file_read_iter);
ksym!(generic_fill_statx_atomic_writes);
ksym!(generic_perform_write);
ksym!(generic_set_sb_d_ops);
ksym!(generic_write_checks);
ksym!(get_inode_acl);
ksym!(get_random_u16);
ksym!(get_random_u32);
// ksym!(get_tree_bdev) — registered in kabi/fs_synth.rs
ksym!(iget_locked);
ksym!(igrab);
ksym!(ihold);
ksym!(in_group_p);
ksym!(inc_nlink);
crate::ksym_static!(init_uts_ns);
ksym!(inode_dio_wait);
ksym!(inode_init_owner);
ksym!(inode_io_list_del);
ksym!(inode_maybe_inc_iversion);
ksym!(inode_needs_sync);
ksym!(inode_newsize_ok);
ksym!(inode_owner_or_capable);
ksym!(inode_query_iversion);
ksym!(inode_set_ctime_current);
ksym!(inode_set_flags);
ksym!(insert_inode_locked);
ksym!(invalidate_bdev);
ksym!(invalidate_inode_buffers);
ksym!(io_schedule_timeout);
ksym!(iocb_bio_iopoll);
ksym!(iomap_swapfile_activate);
ksym!(iov_iter_alignment);
ksym!(is_bad_inode);
ksym!(iter_file_splice_write);
ksym!(kfree_link);
// ksym!(kobject_create_and_add) — registered in kabi/kobject.rs
ksym!(kstrtouint);
ksym!(kthread_should_stop);
ksym!(kthread_stop);
ksym!(ktime_get_real_seconds);
ksym!(kvfree_call_rcu);
ksym!(list_sort);
ksym!(lock_two_nondirectories);
ksym!(make_bad_inode);
ksym!(make_kprojid);
ksym!(make_vfsgid);
ksym!(make_vfsuid);
ksym!(mark_buffer_dirty_inode);
ksym!(memcpy_and_pad);
ksym!(mempool_alloc_slab);
ksym!(mempool_create_node_noprof);
ksym!(mempool_destroy);
ksym!(mempool_free);
ksym!(mempool_free_slab);
ksym!(memweight);
ksym!(mnt_drop_write_file);
ksym!(mnt_want_write_file);
ksym!(mod_timer);
ksym!(nop_mnt_idmap);
ksym!(nsecs_to_jiffies);
ksym!(pagecache_isize_extended);
ksym!(panic);
ksym!(path_put);
ksym!(pcpu_alloc_noprof);
ksym!(percpu_down_write);
ksym!(percpu_free_rwsem);
ksym!(percpu_up_write);
ksym!(posix_acl_alloc);
ksym!(posix_acl_chmod);
ksym!(posix_acl_create);
ksym!(posix_acl_update_mode);
ksym!(preempt_schedule_notrace);
ksym!(print_hex_dump);
ksym!(proc_create_seq_private);
ksym!(proc_create_single_data);
// rb_next is now real — exported from rbtree.rs.
ksym!(rcuwait_wake_up);
ksym!(release_dentry_name_snapshot);
ksym!(remove_proc_subtree);
ksym!(sb_min_blocksize);
ksym!(schedule_timeout_interruptible);
ksym!(schedule_timeout_uninterruptible);
ksym!(security_inode_init_security);
ksym!(seq_escape_mem);
ksym!(seq_putc);
ksym!(set_blocksize);
ksym!(set_cached_acl);
ksym!(set_task_ioprio);
ksym!(setattr_copy);
ksym!(setattr_prepare);
ksym!(simple_inode_init_ts);
ksym!(sort);
ksym!(strchr);
ksym!(strsep);
ksym!(sync_dirty_buffer);
ksym!(sync_filesystem);
ksym!(sync_inode_metadata);
ksym!(sync_mapping_buffers);
ksym!(sysfs_notify);
ksym!(system_dfl_wq);
crate::ksym_static!(system_state);
ksym!(tag_pages_for_writeback);
ksym!(take_dentry_name_snapshot);
ksym!(timer_shutdown_sync);
ksym!(touch_atime);
ksym!(truncate_inode_pages);
ksym!(truncate_pagecache);
ksym!(truncate_pagecache_range);
ksym!(try_to_writeback_inodes_sb);
ksym!(unlock_two_nondirectories);
ksym!(vfs_fsync_range);
ksym!(wbc_account_cgroup_owner);
ksym!(writeback_iter);
ksym!(xa_destroy);
ksym!(xa_erase);
