// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Debug page table tracing for investigating page mapping issues.

use crate::address::{PAddr, UserVAddr};

/// Dump the PTE chain for a virtual address. Logs PML4E → PDPTE → PDE → PTE
/// and reads the physical page content if the PTE is present.
#[allow(unsafe_code)]
pub fn dump_pte(pml4: PAddr, vaddr: usize, pid: i32) {
    let pml4_idx = (vaddr >> 39) & 0x1FF;
    let pdpt_idx = (vaddr >> 30) & 0x1FF;
    let pd_idx = (vaddr >> 21) & 0x1FF;
    let pt_idx = (vaddr >> 12) & 0x1FF;

    unsafe {
        let pml4e = *(pml4.as_ptr::<u64>().add(pml4_idx));
        if pml4e & 1 == 0 {
            warn!("PAGETRACE: pid={} vaddr={:#x} PML4E not present", pid, vaddr);
            return;
        }

        let pdpt_pa = PAddr::new((pml4e & 0x000F_FFFF_FFFF_F000) as usize);
        let pdpte = *(pdpt_pa.as_ptr::<u64>().add(pdpt_idx));
        if pdpte & 1 == 0 {
            warn!("PAGETRACE: pid={} vaddr={:#x} PDPTE not present", pid, vaddr);
            return;
        }

        let pd_pa = PAddr::new((pdpte & 0x000F_FFFF_FFFF_F000) as usize);
        let pde = *(pd_pa.as_ptr::<u64>().add(pd_idx));
        if pde & 1 == 0 {
            warn!("PAGETRACE: pid={} vaddr={:#x} PDE not present", pid, vaddr);
            return;
        }

        if pde & 0x80 != 0 {
            // 2MB huge page
            let base = (pde & 0x000F_FFFF_FFE0_0000) as usize;
            let offset = vaddr & 0x1FFFFF;
            warn!("PAGETRACE: pid={} vaddr={:#x} HUGE_PAGE base={:#x}+{:#x}", pid, vaddr, base, offset);
            return;
        }

        let pt_pa = PAddr::new((pde & 0x000F_FFFF_FFFF_F000) as usize);
        let pte = *(pt_pa.as_ptr::<u64>().add(pt_idx));
        let phys = (pte & 0x000F_FFFF_FFFF_F000) as usize;
        let present = pte & 1;
        let writable = (pte >> 1) & 1;
        let user = (pte >> 2) & 1;

        warn!("PAGETRACE: pid={} vaddr={:#x} pte={:#x} phys={:#x} P={} W={} U={}",
            pid, vaddr, pte, phys, present, writable, user);

        if present != 0 && phys != 0 {
            let pa = PAddr::new(phys);
            let q0 = *(pa.as_ptr::<u64>().add(0));
            let q2 = *(pa.as_ptr::<u64>().add(2)); // offset 0x10 (for busybox .data at 0xc6010)
            let q_130 = *(pa.as_ptr::<u64>().add(0x130 / 8));
            warn!("PAGETRACE: pid={} q[0]={:#x} q[@10]={:#x} q[@130]={:#x}", pid, q0, q2, q_130);
        }
    }
}

/// Read a user-space qword by walking the page table (works even if
/// the current CR3 doesn't map the address, e.g. from another process).
#[allow(unsafe_code)]
fn read_user_qword(pml4: PAddr, vaddr: usize) -> Option<u64> {
    let pml4_idx = (vaddr >> 39) & 0x1FF;
    let pdpt_idx = (vaddr >> 30) & 0x1FF;
    let pd_idx = (vaddr >> 21) & 0x1FF;
    let pt_idx = (vaddr >> 12) & 0x1FF;
    let offset = vaddr & 0xFFF;

    unsafe {
        let pml4e = *(pml4.as_ptr::<u64>().add(pml4_idx));
        if pml4e & 1 == 0 { return None; }
        let pdpt_pa = PAddr::new((pml4e & 0x000F_FFFF_FFFF_F000) as usize);
        let pdpte = *(pdpt_pa.as_ptr::<u64>().add(pdpt_idx));
        if pdpte & 1 == 0 { return None; }
        let pd_pa = PAddr::new((pdpte & 0x000F_FFFF_FFFF_F000) as usize);
        let pde = *(pd_pa.as_ptr::<u64>().add(pd_idx));
        if pde & 1 == 0 { return None; }
        if pde & 0x80 != 0 {
            // Huge page
            let base = (pde & 0x000F_FFFF_FFE0_0000) as usize;
            let phys = base + (vaddr & 0x1FFFFF);
            return Some(*(PAddr::new(phys).as_ptr::<u8>().add(0) as *const u64));
        }
        let pt_pa = PAddr::new((pde & 0x000F_FFFF_FFFF_F000) as usize);
        let pte = *(pt_pa.as_ptr::<u64>().add(pt_idx));
        if pte & 1 == 0 { return None; }
        let phys = ((pte & 0x000F_FFFF_FFFF_F000) as usize) + offset;
        Some(*(PAddr::new(phys).as_ptr::<u64>()))
    }
}

/// Dump 8 qwords from the user stack, resolving via the page table.
#[allow(unsafe_code)]
pub fn dump_stack(rsp: usize, pid: i32, pml4: PAddr) {
    let mut line = alloc::string::String::new();
    use core::fmt::Write;
    let _ = write!(line, "STACK pid={}: ", pid);
    for i in 0..8usize {
        let addr = rsp + i * 8;
        if let Some(val) = read_user_qword(pml4, addr) {
            let _ = write!(line, "[+{}]={:#x} ", i * 8, val);
        } else {
            let _ = write!(line, "[+{}]=UNMAPPED ", i * 8);
        }
    }
    warn!("{}", line);
}
