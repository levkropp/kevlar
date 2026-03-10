// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Minimal ACPI MADT parser used during SMP boot to discover AP APIC IDs.
//!
//! Scans the BIOS extended area (0xE0000–0xFFFFF) for the RSDP, then follows
//! RSDP → RSDT → MADT (signature "APIC") to extract Processor Local APIC
//! entries.  Only ACPI 1.0 RSDT (32-bit pointers) is used; XSDT is a
//! fallback for ACPI 2.0+ systems.
//!
//! No heap allocation is used so this is safe to call during early boot.

use crate::address::PAddr;
use core::ptr;

pub const MAX_CPUS: usize = 16;

// ── ACPI structure layouts ────────────────────────────────────────────

/// RSDP (Root System Descriptor Pointer).
#[repr(C, packed)]
struct Rsdp {
    signature:    [u8; 8],   // "RSD PTR " (note trailing space)
    checksum:     u8,
    oem_id:       [u8; 6],
    revision:     u8,        // 0 = ACPI 1.0, 2 = ACPI 2.0+
    rsdt_address: u32,       // physical address of RSDT
    // ACPI 2.0+ fields follow (length, xsdt_address, …) — we only use rsdt above.
}

/// Generic ACPI SDT header (36 bytes).
#[repr(C, packed)]
struct SdtHeader {
    signature:        [u8; 4],
    length:           u32,
    revision:         u8,
    checksum:         u8,
    oem_id:           [u8; 6],
    oem_table_id:     [u8; 8],
    oem_revision:     u32,
    creator_id:       u32,
    creator_revision: u32,
}

/// Interrupt controller structure header (Type 0 = Processor Local APIC).
#[repr(C, packed)]
struct IcHeader {
    type_:  u8,
    length: u8,
}

/// Type-0 MADT entry: Processor Local APIC.
#[repr(C, packed)]
struct LocalApicEntry {
    type_:     u8,  // 0
    length:    u8,  // 8
    acpi_id:   u8,
    apic_id:   u8,
    flags:     u32, // bit 0 = Processor Enabled
}

const LAPIC_ENABLED: u32 = 1;
const SDT_HDR_SIZE: usize = core::mem::size_of::<SdtHeader>(); // 36

// ── Public API ───────────────────────────────────────────────────────

/// Scan ACPI MADT and return all enabled Processor Local APIC IDs.
/// Returns `(ids_array, count)`.  The caller should exclude the BSP's own
/// APIC ID from the returned list.
pub fn find_lapic_ids() -> ([u8; MAX_CPUS], usize) {
    let mut ids = [0u8; MAX_CPUS];
    let mut count = 0usize;

    let Some(rsdp_paddr) = find_rsdp() else {
        warn!("acpi: RSDP not found — assuming single CPU");
        return (ids, count);
    };

    let rsdt_paddr = unsafe {
        let rsdp = rsdp_paddr.as_ptr::<Rsdp>();
        ptr::read_unaligned(ptr::addr_of!((*rsdp).rsdt_address)) as usize
    };

    let Some(madt_paddr) = find_table_in_rsdt(rsdt_paddr, *b"APIC") else {
        warn!("acpi: MADT not found in RSDT");
        return (ids, count);
    };

    scan_madt(madt_paddr.value(), &mut ids, &mut count);
    info!("acpi: found {} Local APIC(s)", count);
    (ids, count)
}

// ── Internal helpers ─────────────────────────────────────────────────

/// Scan 0xE0000–0xFFFFF (BIOS extended area) for the "RSD PTR " signature.
fn find_rsdp() -> Option<PAddr> {
    const SCAN_START: usize = 0xE_0000;
    const SCAN_END:   usize = 0x10_0000;
    const RSDP_ALIGN: usize = 16;

    let mut paddr = SCAN_START;
    while paddr + 8 <= SCAN_END {
        let ptr = PAddr::new(paddr).as_ptr::<u8>();
        let sig = unsafe { core::slice::from_raw_parts(ptr, 8) };
        if sig == b"RSD PTR " {
            // Verify the RSDP v1 checksum (first 20 bytes must sum to 0).
            let bytes = unsafe { core::slice::from_raw_parts(ptr, 20) };
            let sum: u8 = bytes.iter().fold(0u8, |a, &b| a.wrapping_add(b));
            if sum == 0 {
                info!("acpi: RSDP at {:#x}", paddr);
                return Some(PAddr::new(paddr));
            }
        }
        paddr += RSDP_ALIGN;
    }
    None
}

/// Walk RSDT entries (32-bit physical pointers after the header) looking
/// for a table whose signature matches `sig`.
fn find_table_in_rsdt(rsdt_paddr: usize, sig: [u8; 4]) -> Option<PAddr> {
    let hdr_ptr = PAddr::new(rsdt_paddr).as_ptr::<SdtHeader>();
    let table_len = unsafe { ptr::read_unaligned(ptr::addr_of!((*hdr_ptr).length)) } as usize;

    if table_len < SDT_HDR_SIZE {
        return None;
    }

    // Entries begin after the header; each is a u32 physical address.
    let entry_count = (table_len - SDT_HDR_SIZE) / 4;
    let entries_start = rsdt_paddr + SDT_HDR_SIZE;

    for i in 0..entry_count {
        let entry_ptr = PAddr::new(entries_start + i * 4).as_ptr::<u32>();
        let child_paddr = unsafe { ptr::read_unaligned(entry_ptr) } as usize;
        if child_paddr == 0 {
            continue;
        }

        let child_hdr = PAddr::new(child_paddr).as_ptr::<SdtHeader>();
        let child_sig = unsafe {
            ptr::read_unaligned(ptr::addr_of!((*child_hdr).signature))
        };
        if child_sig == sig {
            return Some(PAddr::new(child_paddr));
        }
    }
    None
}

/// Parse MADT interrupt controller structures and collect Local APIC IDs.
fn scan_madt(madt_paddr: usize, ids: &mut [u8; MAX_CPUS], count: &mut usize) {
    let hdr_ptr = PAddr::new(madt_paddr).as_ptr::<SdtHeader>();
    let table_len =
        unsafe { ptr::read_unaligned(ptr::addr_of!((*hdr_ptr).length)) } as usize;

    // Skip SDT header (36 bytes) + local APIC address (4) + flags (4) = 44 bytes.
    const MADT_FIXED: usize = SDT_HDR_SIZE + 4 + 4;
    if table_len <= MADT_FIXED {
        return;
    }

    let mut offset = MADT_FIXED;
    while offset + 2 <= table_len {
        let ic_ptr = PAddr::new(madt_paddr + offset).as_ptr::<IcHeader>();
        let (ic_type, ic_len) = unsafe {
            (
                ptr::read_unaligned(ptr::addr_of!((*ic_ptr).type_)),
                ptr::read_unaligned(ptr::addr_of!((*ic_ptr).length)) as usize,
            )
        };

        if ic_len < 2 || offset + ic_len > table_len {
            break;
        }

        // Type 0: Processor Local APIC
        if ic_type == 0 && ic_len >= 8 {
            let entry_ptr = PAddr::new(madt_paddr + offset).as_ptr::<LocalApicEntry>();
            let (apic_id, flags) = unsafe {
                (
                    ptr::read_unaligned(ptr::addr_of!((*entry_ptr).apic_id)),
                    ptr::read_unaligned(ptr::addr_of!((*entry_ptr).flags)),
                )
            };

            if flags & LAPIC_ENABLED != 0 && *count < MAX_CPUS {
                ids[*count] = apic_id;
                *count += 1;
            }
        }

        offset += ic_len;
    }
}
