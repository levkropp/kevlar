// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! msync(2) — synchronize a file mapping with the file.
//!
//! Provenance: Own (Linux msync(2) man page).
use crate::fs::opened_file::OpenOptions;
use crate::prelude::*;
use crate::{process::current_process, syscalls::SyscallHandler};
use kevlar_platform::address::UserVAddr;
use kevlar_platform::arch::PAGE_SIZE;
use kevlar_utils::alignment::is_aligned;

use crate::mm::vm::VmAreaType;

const MS_ASYNC: i32 = 1;
const MS_INVALIDATE: i32 = 2;
const MS_SYNC: i32 = 4;

impl<'a> SyscallHandler<'a> {
    pub fn sys_msync(&mut self, addr: UserVAddr, len: usize, flags: i32) -> Result<isize> {
        if !is_aligned(addr.value(), PAGE_SIZE) {
            return Err(Errno::EINVAL.into());
        }
        if len == 0 {
            return Ok(0);
        }
        // MS_SYNC and MS_ASYNC are mutually exclusive.
        if flags & MS_SYNC != 0 && flags & MS_ASYNC != 0 {
            return Err(Errno::EINVAL.into());
        }

        let len = kevlar_utils::alignment::align_up(len, PAGE_SIZE);
        let end_value = addr.value() + len;

        let current = current_process();
        let vm_ref = current.vm();
        let vm = vm_ref.as_ref().unwrap().lock_preempt();

        // Verify the range is fully mapped (Linux returns ENOMEM otherwise).
        // We just check that at least one VMA covers the start address.
        let has_vma = vm.vm_areas_ref().iter().any(|vma| {
            let vs = vma.start().value();
            let ve = vs + vma.len();
            addr.value() >= vs && addr.value() < ve
        });
        if !has_vma {
            return Err(Errno::ENOMEM.into());
        }

        // For MAP_PRIVATE mappings, msync is a no-op (writes are private).
        // For MAP_SHARED file-backed mappings, write dirty pages back to file.
        // We collect writeback info under the VM lock, then release and do I/O.
        struct Writeback {
            file: Arc<dyn crate::fs::inode::FileLike>,
            file_offset: usize,
            paddr: kevlar_platform::address::PAddr,
        }
        let mut writebacks: Vec<Writeback> = Vec::new();

        for vma in vm.vm_areas_ref().iter() {
            let vma_start = vma.start().value();
            let vma_end = vma_start + vma.len();

            if vma_end <= addr.value() || vma_start >= end_value {
                continue;
            }
            if !vma.is_shared() {
                continue;
            }
            let (file, file_base_offset) = match vma.area_type() {
                VmAreaType::File { file, offset, .. } => (file.clone(), *offset),
                _ => continue,
            };

            let overlap_start = core::cmp::max(addr.value(), vma_start);
            let overlap_end = core::cmp::min(end_value, vma_end);
            let mut cursor = overlap_start;
            while cursor < overlap_end {
                if let Some(paddr) = vm.page_table().lookup_paddr(
                    UserVAddr::new(cursor).unwrap(),
                ) {
                    writebacks.push(Writeback {
                        file: file.clone(),
                        file_offset: file_base_offset + (cursor - vma_start),
                        paddr,
                    });
                }
                cursor += PAGE_SIZE;
            }
        }

        drop(vm);

        // Write back pages outside the VM lock.
        for wb in &writebacks {
            #[cfg(not(feature = "profile-fortress"))]
            {
                let buf = kevlar_platform::page_ops::page_as_slice(wb.paddr);
                let _ = wb.file.write(
                    wb.file_offset,
                    buf.into(),
                    &OpenOptions::readwrite(),
                );
            }
            #[cfg(feature = "profile-fortress")]
            {
                let mut tmp = [0u8; PAGE_SIZE];
                let frame = kevlar_platform::page_ops::PageFrame::new(wb.paddr);
                frame.read(0, &mut tmp);
                let _ = wb.file.write(
                    wb.file_offset,
                    (&tmp[..]).into(),
                    &OpenOptions::readwrite(),
                );
            }
        }

        Ok(0)
    }
}
