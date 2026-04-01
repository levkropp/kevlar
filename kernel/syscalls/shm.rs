// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! SysV shared memory: shmget, shmat, shmdt, shmctl.
//!
//! Reference: FreeBSD kern/sysv_shm.c, POSIX shm_open(3).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicI32, Ordering};
use kevlar_platform::address::{PAddr, UserVAddr};
use kevlar_platform::arch::PAGE_SIZE;
use kevlar_platform::page_allocator::{alloc_page, free_pages, AllocPageFlags};
use kevlar_platform::spinlock::SpinLock;

use crate::ctypes::*;
use crate::mm::vm::VmAreaType;
use crate::prelude::*;
use crate::process::current_process;
use crate::result::{Errno, Result};
use crate::syscalls::SyscallHandler;

// ─── Constants ──────────────────────────────────────────────────────────────

const IPC_CREAT: i32 = 0o1000;
const IPC_EXCL: i32 = 0o2000;
const IPC_RMID: i32 = 0;
const IPC_STAT: i32 = 2;
#[allow(dead_code)]
const IPC_SET: i32 = 1;
const IPC_PRIVATE: i32 = 0;

#[allow(dead_code)]
const SHM_RDONLY: i32 = 0o10000;
const SHM_RND: i32 = 0o20000;

// ─── Segment storage ────────────────────────────────────────────────────────

struct ShmSegment {
    key: i32,
    size: usize,
    pages: Vec<PAddr>,
    nattach: u32,
    marked_for_removal: bool,
}

static NEXT_SHMID: AtomicI32 = AtomicI32::new(1);
static SHM_SEGMENTS: SpinLock<BTreeMap<i32, ShmSegment>> = SpinLock::new(BTreeMap::new());

// ─── Syscalls ───────────────────────────────────────────────────────────────

impl<'a> SyscallHandler<'a> {
    pub fn sys_shmget(&self, key: c_int, size: usize, shmflg: c_int) -> Result<isize> {
        let size = kevlar_utils::alignment::align_up(size, PAGE_SIZE);
        if size == 0 || size > 256 * 1024 * 1024 {
            return Err(Errno::EINVAL.into());
        }

        let mut segments = SHM_SEGMENTS.lock();

        // Look for existing segment
        if key != IPC_PRIVATE {
            for (&id, seg) in segments.iter() {
                if seg.key == key {
                    if shmflg & IPC_CREAT != 0 && shmflg & IPC_EXCL != 0 {
                        return Err(Errno::EEXIST.into());
                    }
                    return Ok(id as isize);
                }
            }
            if shmflg & IPC_CREAT == 0 {
                return Err(Errno::ENOENT.into());
            }
        }

        // Allocate pre-zeroed pages
        let num_pages = size / PAGE_SIZE;
        let mut pages = Vec::with_capacity(num_pages);
        for _ in 0..num_pages {
            match alloc_page(AllocPageFlags::USER) {
                Ok(paddr) => pages.push(paddr),
                Err(_) => {
                    for p in &pages { free_pages(*p, 1); }
                    return Err(Errno::ENOMEM.into());
                }
            }
        }

        let shmid = NEXT_SHMID.fetch_add(1, Ordering::Relaxed);
        segments.insert(shmid, ShmSegment {
            key,
            size,
            pages,
            nattach: 0,
            marked_for_removal: false,
        });

        Ok(shmid as isize)
    }

    pub fn sys_shmat(&self, shmid: c_int, shmaddr: usize, shmflg: c_int) -> Result<isize> {
        let mut segments = SHM_SEGMENTS.lock();
        let seg = segments.get_mut(&shmid)
            .ok_or(Errno::EINVAL)?;

        let size = seg.size;
        let pages: Vec<PAddr> = seg.pages.clone();
        seg.nattach += 1;
        drop(segments);

        let current = current_process();
        let vm_ref = current.vm();
        let mut vm = vm_ref.as_ref().unwrap().lock_preempt();

        let mapped_addr = if shmaddr != 0 {
            let addr_val = if shmflg & SHM_RND != 0 {
                shmaddr & !(PAGE_SIZE - 1)
            } else {
                shmaddr
            };
            UserVAddr::new(addr_val).ok_or(Errno::EINVAL)?
        } else {
            vm.alloc_vaddr_range(size)?
        };

        let prot = crate::ctypes::MMapProt::PROT_READ | crate::ctypes::MMapProt::PROT_WRITE;
        vm.add_vm_area_with_prot(mapped_addr, size, VmAreaType::Anonymous, prot, true)?;

        // Pre-map the shared physical pages (writable)
        for (i, paddr) in pages.iter().enumerate() {
            let page_addr = mapped_addr.add(i * PAGE_SIZE);
            kevlar_platform::page_refcount::page_ref_inc(*paddr);
            vm.page_table_mut().map_user_page_with_prot(page_addr, *paddr, 2);
        }

        Ok(mapped_addr.value() as isize)
    }

    pub fn sys_shmdt(&self, shmaddr: usize) -> Result<isize> {
        let addr = UserVAddr::new(shmaddr).ok_or(Errno::EINVAL)?;

        let current = current_process();
        let vm_ref = current.vm();
        let mut vm = vm_ref.as_ref().unwrap().lock_preempt();

        // Find the VMA and remove it
        if let Some(size) = vm.vma_len_at(addr) {
            vm.remove_vma_range(addr, size)?;
        }

        // Decrement attach count
        let mut segments = SHM_SEGMENTS.lock();
        // We don't track which segment was at which address, so decrement
        // the first segment with nattach > 0 (simplification).
        for seg in segments.values_mut() {
            if seg.nattach > 0 {
                seg.nattach -= 1;
                break;
            }
        }
        segments.retain(|_, seg| !seg.marked_for_removal || seg.nattach > 0);

        Ok(0)
    }

    pub fn sys_shmctl(&self, shmid: c_int, cmd: c_int, buf: usize) -> Result<isize> {
        match cmd {
            IPC_RMID => {
                let mut segments = SHM_SEGMENTS.lock();
                if let Some(seg) = segments.get_mut(&shmid) {
                    if seg.nattach == 0 {
                        for paddr in &seg.pages {
                            free_pages(*paddr, 1);
                        }
                        segments.remove(&shmid);
                    } else {
                        seg.marked_for_removal = true;
                    }
                }
                Ok(0)
            }
            IPC_STAT => {
                let segments = SHM_SEGMENTS.lock();
                let seg = segments.get(&shmid).ok_or(Errno::EINVAL)?;
                // struct shmid_ds: 112 bytes on x86_64
                let mut ds = [0u8; 112];
                // shm_segsz at offset 48
                ds[48..56].copy_from_slice(&(seg.size as u64).to_le_bytes());
                // shm_nattch at offset 88
                ds[88..96].copy_from_slice(&(seg.nattach as u64).to_le_bytes());
                if buf != 0 {
                    UserVAddr::new_nonnull(buf)?.write_bytes(&ds)?;
                }
                Ok(0)
            }
            _ => Ok(0),
        }
    }
}
