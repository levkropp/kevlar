// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Debug page table tracing for investigating page mapping issues.

use crate::address::PAddr;

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
            let q_130 = *(pa.as_ptr::<u64>().add(0x130 / 8));
            warn!("PAGETRACE: pid={} q[0]={:#x} q[@130]={:#x}", pid, q0, q_130);
        }
    }
}
