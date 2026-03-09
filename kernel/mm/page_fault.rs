// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use kevlar_utils::alignment::align_down;

use super::vm::VmAreaType;
use crate::{
    debug::{self, DebugEvent, DebugFilter},
    fs::opened_file::OpenOptions,
    process::{
        current_process,
        signal::{self, SIGSEGV},
        Process,
    },
};
use core::cmp::min;
use kevlar_platform::{
    address::UserVAddr,
    arch::{PageFaultReason, PAGE_SIZE},
    page_allocator::{alloc_page, alloc_page_batch, AllocPageFlags},
    page_ops::zero_page,
};
#[cfg(not(feature = "profile-fortress"))]
use kevlar_platform::page_ops::page_as_slice_mut;
#[cfg(feature = "profile-fortress")]
use kevlar_platform::page_ops::PageFrame;

pub fn handle_page_fault(unaligned_vaddr: Option<UserVAddr>, ip: usize, _reason: PageFaultReason) {
    // Usercopy fault debug check — only in debug builds to avoid hot-path overhead.
    #[cfg(debug_assertions)]
    #[cfg(target_arch = "x86_64")]
    if debug::is_enabled(DebugFilter::FAULT) || debug::is_enabled(DebugFilter::USERCOPY) {
        #[allow(unsafe_code)]
        unsafe extern "C" {
            fn usercopy1();
            fn usercopy1b();
            fn usercopy1c();
            fn usercopy1d();
            fn usercopy2();
            fn usercopy3();
        }
        let ip_val = ip as u64;
        let in_usercopy = ip_val == usercopy1 as u64
            || ip_val == usercopy1b as u64
            || ip_val == usercopy1c as u64
            || ip_val == usercopy1d as u64
            || ip_val == usercopy2 as u64
            || ip_val == usercopy3 as u64;
        if in_usercopy {
            let pid = current_process().pid().as_i32();
            let fault_addr = unaligned_vaddr.map(|v| v.value()).unwrap_or(0);
            debug::emit_usercopy_fault(pid, fault_addr, ip);
        }
    }

    let unaligned_vaddr = match unaligned_vaddr {
        Some(unaligned_vaddr) => unaligned_vaddr,
        None => {
            let pid = current_process().pid().as_i32();
            debug::emit(DebugFilter::FAULT, &DebugEvent::PageFault {
                pid,
                vaddr: 0,
                ip,
                reason: "null_pointer",
                resolved: false,
                vma_start: None,
                vma_end: None,
                vma_type: None,
            });
            debug_warn!(
                "null pointer access (pid={}, ip={:x}), killing the current process...",
                pid, ip
            );
            Process::exit_by_signal(signal::SIGSEGV);
        }
    };

    let current = current_process();
    let aligned_vaddr = match UserVAddr::new_nonnull(align_down(unaligned_vaddr.value(), PAGE_SIZE))
    {
        Ok(uaddr) => uaddr,
        _ => {
            debug_warn!(
                "invalid memory access at {} (ip={:x}), killing the current process...",
                unaligned_vaddr,
                ip
            );
            Process::exit_by_signal(SIGSEGV);
        }
    };

    // Allocate and zero the page BEFORE acquiring the VM lock.
    // This keeps the lock hold time minimal (just VMA lookup + PTE write).
    let paddr = alloc_page(AllocPageFlags::USER | AllocPageFlags::DIRTY_OK)
        .expect("failed to allocate an anonymous page");
    zero_page(paddr);

    // Look for the associated vma area.
    let vm_ref = current.vm();
    let mut vm = vm_ref.as_ref().unwrap().lock_no_irq();
    let vma = match vm.find_vma_cached(unaligned_vaddr) {
        Some(vma) => vma,
        None => {
            let pid = current.pid().as_i32();
            debug::emit(DebugFilter::FAULT, &DebugEvent::PageFault {
                pid,
                vaddr: unaligned_vaddr.value(),
                ip,
                reason: "no_vma",
                resolved: false,
                vma_start: None,
                vma_end: None,
                vma_type: None,
            });
            debug_warn!(
                "pid={}: no VMAs for address {} (ip={:x}, reason={:?}), killing the current process...",
                pid, unaligned_vaddr, ip, _reason
            );
            // Free the page we allocated since we're killing the process.
            kevlar_platform::page_allocator::free_pages(paddr, 1);
            drop(vm);
            drop(vm_ref);
            Process::exit_by_signal(SIGSEGV);
        }
    };

    match vma.area_type() {
        VmAreaType::Anonymous => { /* The page is already filled with zeros. Nothing to do. */ }
        VmAreaType::File {
            file,
            offset,
            file_size,
        } => {
            let offset_in_page;
            let offset_in_file;
            let copy_len;
            if aligned_vaddr < vma.start() {
                // The VMA starts partway through this page. Place file data
                // at the VMA start's offset within the page.
                offset_in_page = vma.start().value() % PAGE_SIZE;
                offset_in_file = *offset;
                copy_len = min(*file_size, PAGE_SIZE - offset_in_page);
            } else {
                let offset_in_vma = vma.offset_in_vma(aligned_vaddr);
                offset_in_page = 0;
                if offset_in_vma >= *file_size {
                    offset_in_file = 0;
                    copy_len = 0;
                } else {
                    offset_in_file = offset + offset_in_vma;
                    copy_len = min(*file_size - offset_in_vma, PAGE_SIZE);
                }
            }

            if copy_len > 0 {
                // Fortress: read file into a stack buffer, then copy to
                // the page frame without exposing a raw &mut [u8] to
                // physical memory.
                #[cfg(feature = "profile-fortress")]
                {
                    let mut tmp = [0u8; PAGE_SIZE];
                    let dst = &mut tmp[..copy_len];
                    file.read(
                        offset_in_file,
                        dst.into(),
                        &OpenOptions::readwrite(),
                    )
                    .expect("failed to read file");
                    let mut frame = PageFrame::new(paddr);
                    frame.write(offset_in_page, dst);
                }

                // Other profiles: write directly into the page via
                // page_as_slice_mut for zero-copy performance.
                #[cfg(not(feature = "profile-fortress"))]
                {
                    let buf = page_as_slice_mut(paddr);
                    file.read(
                        offset_in_file,
                        (&mut buf[offset_in_page..(offset_in_page + copy_len)]).into(),
                        &OpenOptions::readwrite(),
                    )
                    .expect("failed to read file");
                }
            }
        }
    }

    // Map the page in the page table, respecting VMA protection flags.
    let prot_flags = vma.prot().bits();
    let vma_end_value = vma.end().value();
    let is_anonymous = matches!(vma.area_type(), VmAreaType::Anonymous);

    // Emit successful fault resolution event (only in debug builds).
    #[cfg(debug_assertions)]
    if debug::is_enabled(DebugFilter::FAULT) {
        let vma_type_str = match vma.area_type() {
            VmAreaType::Anonymous => "anonymous",
            VmAreaType::File { .. } => "file",
        };
        debug::emit(DebugFilter::FAULT, &DebugEvent::PageFault {
            pid: current.pid().as_i32(),
            vaddr: unaligned_vaddr.value(),
            ip,
            reason: "demand_page",
            resolved: true,
            vma_start: Some(vma.start().value()),
            vma_end: Some(vma.end().value()),
            vma_type: Some(vma_type_str),
        });
    }

    vm.page_table_mut()
        .map_user_page_with_prot(aligned_vaddr, paddr, prot_flags);

    // Fault-around: for anonymous mappings, prefault adjacent pages to reduce
    // the number of page faults (and their associated exception + EPT overhead).
    // This mirrors Linux's fault_around_bytes behavior.
    // Batch-allocate pages to amortize allocator lock overhead.
    if is_anonymous {
        use kevlar_platform::address::PAddr;
        const FAULT_AROUND_PAGES: usize = 64;

        // Count how many pages we can prefault.
        let mut num_prefault = 0;
        for i in 1..FAULT_AROUND_PAGES {
            let next_value = aligned_vaddr.value() + i * PAGE_SIZE;
            if next_value >= vma_end_value {
                break;
            }
            if UserVAddr::new_nonnull(next_value).is_err() {
                break;
            }
            num_prefault += 1;
        }

        if num_prefault > 0 {
            let mut pages = [PAddr::new(0); FAULT_AROUND_PAGES];
            let allocated = alloc_page_batch(&mut pages, num_prefault);

            for i in 0..allocated {
                let next_value = aligned_vaddr.value() + (i + 1) * PAGE_SIZE;
                let next_addr = match UserVAddr::new_nonnull(next_value) {
                    Ok(a) => a,
                    Err(_) => {
                        // Free remaining pages.
                        for j in i..allocated {
                            kevlar_platform::page_allocator::free_pages(pages[j], 1);
                        }
                        break;
                    }
                };

                zero_page(pages[i]);
                if !vm.page_table_mut()
                    .try_map_user_page_with_prot(next_addr, pages[i], prot_flags)
                {
                    kevlar_platform::page_allocator::free_pages(pages[i], 1);
                }
            }
        }
    }
}
