// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
// Own implementation based on Linux man pages.
use alloc::sync::Arc;
use crate::fs::inode::FileLike;
use crate::mm::vm::VmAreaType;
use crate::prelude::*;
use crate::process::current_process;
use crate::syscalls::SyscallHandler;
use crate::user_buffer::UserBuffer;
use kevlar_platform::{
    address::{PAddr, UserVAddr},
    arch::{PAGE_SIZE, HUGE_PAGE_SIZE},
    page_allocator::{free_pages, free_huge_page_and_zero},
};
use kevlar_utils::alignment::is_aligned;

/// Info about a MAP_SHARED file page that needs writeback before free.
struct SharedWriteback {
    file: Arc<dyn FileLike>,
    file_offset: usize,
    paddr: PAddr,
}

impl<'a> SyscallHandler<'a> {
    pub fn sys_munmap(&mut self, addr: UserVAddr, len: usize) -> Result<isize> {
        if !is_aligned(addr.value(), PAGE_SIZE) {
            return Err(Errno::EINVAL.into());
        }

        if len == 0 {
            return Err(Errno::EINVAL.into());
        }

        let len = kevlar_utils::alignment::align_up(len, PAGE_SIZE);

        let current = current_process();
        kevlar_platform::flight_recorder::record(
            kevlar_platform::flight_recorder::kind::MUNMAP,
            current.pid().as_i32() as u32,
            addr.value() as u64,
            len as u64,
        );
        let vm_ref = current.vm();
        let mut vm = vm_ref.as_ref().unwrap().lock_preempt();

        // Collect shared file VMAs that need writeback BEFORE removing them.
        let mut writebacks: alloc::vec::Vec<SharedWriteback> = alloc::vec::Vec::new();
        let end_value = addr.value() + len;

        // Scan VMAs in the unmap range for MAP_SHARED file mappings.
        for vma in vm.vm_areas_ref().iter() {
            let vma_start = vma.start().value();
            let vma_end = vma_start + vma.len();

            // Skip non-overlapping VMAs
            if vma_end <= addr.value() || vma_start >= end_value {
                continue;
            }

            // Only care about shared file-backed VMAs
            if !vma.is_shared() {
                continue;
            }
            let (file, file_base_offset) = match vma.area_type() {
                VmAreaType::File { file, offset, .. } => (file.clone(), *offset),
                _ => continue,
            };

            // For each page in the overlap region, schedule writeback
            let overlap_start = core::cmp::max(addr.value(), vma_start);
            let overlap_end = core::cmp::min(end_value, vma_end);
            let mut cursor = overlap_start;
            while cursor < overlap_end {
                let page_addr = UserVAddr::new(cursor).unwrap();
                // Check if this page is mapped (has a physical page)
                if let Some(paddr) = vm.page_table().lookup_paddr(page_addr) {
                    let file_offset = file_base_offset + (cursor - vma_start);
                    writebacks.push(SharedWriteback {
                        file: file.clone(),
                        file_offset,
                        paddr,
                    });
                }
                cursor += PAGE_SIZE;
            }
        }

        // Remove VMAs in the range (splits at boundaries).
        vm.remove_vma_range(addr, len)?;

        // Walk the page table: clear PTEs and collect physical pages.
        let mut to_free: alloc::vec::Vec<(PAddr, usize)> = alloc::vec::Vec::new();
        let mut cursor = addr.value();
        while cursor < end_value {
            let page_addr = UserVAddr::new(cursor).unwrap();

            // Check for 2MB huge page at this address.
            if is_aligned(cursor, HUGE_PAGE_SIZE) && cursor + HUGE_PAGE_SIZE <= end_value {
                if let Some(hp_paddr) = vm.page_table_mut().unmap_huge_user_page(page_addr) {
                    vm.page_table().flush_tlb_local(page_addr);
                    let mut all_sole_owner = true;
                    for sub_i in 0..512usize {
                        let sub = PAddr::new(hp_paddr.value() + sub_i * PAGE_SIZE);
                        if kevlar_platform::page_refcount::page_ref_count(sub) != 1 {
                            all_sole_owner = false;
                            break;
                        }
                    }
                    if all_sole_owner {
                        to_free.push((hp_paddr, 512));
                    } else {
                        for sub_i in 0..512usize {
                            to_free.push((
                                PAddr::new(hp_paddr.value() + sub_i * PAGE_SIZE),
                                1,
                            ));
                        }
                    }
                    cursor += HUGE_PAGE_SIZE;
                    continue;
                }
            }

            if vm.page_table().is_huge_mapped(page_addr).is_some() {
                vm.page_table_mut().split_huge_page(page_addr);
                vm.page_table().flush_tlb_local(page_addr);
            }

            if let Some(paddr) = vm.page_table_mut().unmap_user_page(page_addr) {
                vm.page_table().flush_tlb_local(page_addr);
                to_free.push((paddr, 1));
            }
            cursor += PAGE_SIZE;
        }

        if !to_free.is_empty() {
            vm.page_table().flush_tlb_remote();
        }

        // Drop VM lock before doing file I/O (writeback + page free).
        drop(vm);

        // Write back MAP_SHARED dirty pages to files.
        for wb in &writebacks {
            // Read page data from physical memory via the straight map.
            let vaddr = wb.paddr.as_vaddr();
            #[allow(unsafe_code)]
            let page_data: &[u8] = unsafe {
                core::slice::from_raw_parts(vaddr.as_ptr::<u8>(), PAGE_SIZE)
            };
            // Write to the file at the correct offset using kernel-slice UserBuffer.
            let opts = crate::fs::opened_file::OpenOptions::new(false, false);
            let _ = wb.file.write(
                wb.file_offset,
                UserBuffer::from(page_data),
                &opts,
            );
        }

        // Now safe to free all unmapped physical pages.
        for (paddr, num) in to_free {
            if num == 512 {
                for sub_i in 0..512usize {
                    let sub = PAddr::new(paddr.value() + sub_i * PAGE_SIZE);
                    kevlar_platform::page_refcount::page_ref_dec(sub);
                }
                free_huge_page_and_zero(paddr);
            } else if kevlar_platform::page_refcount::page_ref_dec(paddr) {
                free_pages(paddr, num);
            }
        }

        Ok(0)
    }
}
