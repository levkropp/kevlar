// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use kevlar_runtime::{
    address::UserVAddr,
    arch::PAGE_SIZE,
    page_allocator::free_pages,
};
use kevlar_utils::alignment::is_aligned;

use crate::{
    ctypes::*, fs::opened_file::Fd, mm::vm::VmAreaType, prelude::*, process::current_process,
    syscalls::SyscallHandler,
};

impl<'a> SyscallHandler<'a> {
    pub fn sys_mmap(
        &mut self,
        addr_hint: Option<UserVAddr>,
        len: c_size,
        prot: MMapProt,
        flags: MMapFlags,
        fd: Fd,
        offset: c_off,
    ) -> Result<isize> {
        if !is_aligned(len as usize, PAGE_SIZE) {
            return Err(Errno::EINVAL.into());
        }

        if !is_aligned(offset as usize, PAGE_SIZE) {
            return Err(Errno::EINVAL.into());
        }

        let area_type = if flags.contains(MMapFlags::MAP_ANONYMOUS) {
            VmAreaType::Anonymous
        } else {
            let opened_file = current_process()
                .opened_files()
                .lock()
                .get(fd)?
                .clone();
            let file = opened_file.as_file()?.clone();

            // Compute the actual file size remaining from offset, so the page
            // fault handler knows where file data ends and zero-fill begins
            // (important for BSS-like regions where p_filesz < p_memsz).
            let file_size = opened_file.inode().stat()
                .map(|st| {
                    let remaining = (st.size.0 as usize).saturating_sub(offset as usize);
                    core::cmp::min(len as usize, remaining)
                })
                .unwrap_or(len as usize);

            VmAreaType::File {
                file,
                offset: offset as usize,
                file_size,
            }
        };

        // Determine the virtual address space to map.
        let current = current_process();
        let vm_ref = current.vm();
        let mut vm = vm_ref.as_ref().unwrap().lock();
        let mapped_uaddr = if flags.contains(MMapFlags::MAP_FIXED) {
            match addr_hint {
                Some(addr) => {
                    if !is_aligned(addr.value(), PAGE_SIZE) {
                        return Err(Errno::EINVAL.into());
                    }

                    // MAP_FIXED: unmap any existing mappings in the range first.
                    if !vm.is_free_vaddr_range(addr, len as usize) {
                        vm.remove_vma_range(addr, len as usize)?;
                        let num_pages = len as usize / PAGE_SIZE;
                        for i in 0..num_pages {
                            let page_addr = addr.add(i * PAGE_SIZE);
                            if let Some(paddr) = vm.page_table_mut().unmap_user_page(page_addr) {
                                free_pages(paddr, 1);
                                vm.page_table().flush_tlb(page_addr);
                            }
                        }
                    }

                    addr
                }
                None => return Err(Errno::EINVAL.into()),
            }
        } else {
            match addr_hint {
                Some(addr) if vm.is_free_vaddr_range(addr, len as usize) => addr,
                _ => vm.alloc_vaddr_range(len as usize)?,
            }
        };

        vm.add_vm_area_with_prot(mapped_uaddr, len as usize, area_type, prot)?;
        Ok(mapped_uaddr.value() as isize)
    }
}
