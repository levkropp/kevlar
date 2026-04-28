// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Minimal device tree blob (DTB) parser for QEMU virt machine.
//! Extracts memory regions and command line from the DTB.
use crate::address::PAddr;
use crate::bootinfo::{BootInfo, RamArea, VirtioMmioDevice};
use arrayvec::{ArrayString, ArrayVec};
use core::cmp::max;

const DTB_MAGIC: u32 = 0xd00dfeed;

/// DTB header (big-endian).
#[repr(C)]
struct DtbHeader {
    magic: u32,
    totalsize: u32,
    off_dt_struct: u32,
    off_dt_strings: u32,
    off_mem_rsvmap: u32,
    version: u32,
    last_comp_version: u32,
    boot_cpuid_phys: u32,
    size_dt_strings: u32,
    size_dt_struct: u32,
}

fn be32(val: u32) -> u32 {
    u32::from_be(val)
}

fn be64_from_cells(high: u32, low: u32) -> u64 {
    ((u32::from_be(high) as u64) << 32) | (u32::from_be(low) as u64)
}

unsafe extern "C" {
    static __kernel_image_end: u8;
}

/// Parse a device tree blob and extract boot information.
/// Falls back to hardcoded QEMU virt defaults if DTB is invalid.
pub unsafe fn parse(dtb_paddr: PAddr) -> BootInfo {
    let dtb_paddr = if dtb_paddr.value() != 0 {
        dtb_paddr
    } else {
        // QEMU doesn't pass DTB in x0 for ELF kernels.  Use defaults.
        // Init path is patched via KEVLAR_INIT slot (compare-contracts.py).
        println!("DTB not found, using defaults");
        return default_boot_info();
    };

    let header = &*(dtb_paddr.as_vaddr().as_ptr::<DtbHeader>());

    if be32(header.magic) != DTB_MAGIC {
        warn!("DTB magic mismatch ({:#x}), using hardcoded QEMU virt defaults", be32(header.magic));
        return default_boot_info();
    }

    // For now, use a simplified approach: scan the struct block for
    // memory nodes and the chosen/bootargs property.
    let struct_base = dtb_paddr.as_vaddr().add(be32(header.off_dt_struct) as usize);
    let strings_base = dtb_paddr.as_vaddr().add(be32(header.off_dt_strings) as usize);
    let struct_size = be32(header.size_dt_struct) as usize;

    let mut ram_areas = ArrayVec::new();
    let mut cpu_mpdirs: ArrayVec<u64, 8> = ArrayVec::new();
    let mut virtio_mmio_devices: ArrayVec<VirtioMmioDevice, 32> = ArrayVec::new();
    let mut cmdline_str: Option<&str> = None;
    let mut in_memory = false;
    let mut in_chosen = false;
    let mut depth: i32 = 0;
    let mut cpus_depth: i32 = -1;
    let mut in_cpu_node = false;
    // virtio_mmio@<addr> nodes are top-level children of /.  Within one
    // we collect the MMIO base address from `reg` and the SPI IRQ
    // number from `interrupts` (QEMU virt uses <type=0 SPI, spi_num,
    // flags=1 level>).  Kevlar's GIC `enable_irq` takes the GIC INTID,
    // which is SPI + 32 for SPIs.
    let mut in_virtio_mmio = false;
    let mut vmmio_base: Option<usize> = None;
    let mut vmmio_irq: Option<u8> = None;
    // QEMU `-machine virt` exposes a single fw-cfg@<addr> node — used
    // by `exts/ramfb` for QEMU's ramfb scan-out device.
    let mut in_fw_cfg = false;
    let mut fw_cfg_base: Option<PAddr> = None;

    let mut offset = 0usize;
    while offset + 4 <= struct_size {
        let token = be32(*(struct_base.add(offset).as_ptr::<u32>()));
        offset += 4;

        match token {
            1 => {
                // FDT_BEGIN_NODE: followed by null-terminated name string.
                let name_ptr = struct_base.add(offset).as_ptr::<u8>();
                let name = read_cstr(name_ptr);
                let name_len = name.len() + 1; // include null
                offset += (name_len + 3) & !3; // align to 4

                depth += 1;
                // Top-level nodes are at depth 2 (root node is depth 1).
                if depth == 2 && name.starts_with("memory") {
                    in_memory = true;
                } else if depth == 2 && name == "chosen" {
                    in_chosen = true;
                } else if depth == 2 && name == "cpus" {
                    cpus_depth = depth;
                } else if cpus_depth >= 0 && depth == cpus_depth + 1 && name.starts_with("cpu@") {
                    in_cpu_node = true;
                } else if depth == 2 && name.starts_with("virtio_mmio@") {
                    in_virtio_mmio = true;
                    vmmio_base = None;
                    vmmio_irq = None;
                } else if depth == 2 && name.starts_with("fw-cfg@") {
                    in_fw_cfg = true;
                }
            }
            2 => {
                // FDT_END_NODE
                if in_cpu_node {
                    in_cpu_node = false;
                } else if cpus_depth >= 0 && depth == cpus_depth {
                    cpus_depth = -1;
                } else if in_virtio_mmio {
                    // Commit the virtio-mmio slot only if it had both reg
                    // and interrupts — otherwise it's an empty placeholder.
                    if let (Some(base), Some(irq)) = (vmmio_base, vmmio_irq) {
                        if !virtio_mmio_devices.is_full() {
                            let _ = virtio_mmio_devices.try_push(VirtioMmioDevice {
                                mmio_base: PAddr::new(base),
                                irq,
                            });
                        }
                    }
                    in_virtio_mmio = false;
                    vmmio_base = None;
                    vmmio_irq = None;
                } else if in_fw_cfg {
                    in_fw_cfg = false;
                } else {
                    in_memory = false;
                    in_chosen = false;
                }
                depth -= 1;
            }
            3 => {
                // FDT_PROP: u32 len, u32 nameoff, then data.
                if offset + 8 > struct_size {
                    break;
                }
                let prop_len = be32(*(struct_base.add(offset).as_ptr::<u32>())) as usize;
                let name_off = be32(*(struct_base.add(offset + 4).as_ptr::<u32>())) as usize;
                offset += 8;

                let prop_name = read_cstr(strings_base.add(name_off).as_ptr::<u8>());
                let prop_data = struct_base.add(offset).as_ptr::<u8>();

                if in_memory && prop_name == "reg" && prop_len >= 16 {
                    // Memory node: #address-cells=2, #size-cells=2.
                    let base_hi = *(prop_data as *const u32);
                    let base_lo = *(prop_data.add(4) as *const u32);
                    let size_hi = *(prop_data.add(8) as *const u32);
                    let size_lo = *(prop_data.add(12) as *const u32);
                    let base = be64_from_cells(base_hi, base_lo) as usize;
                    let size = be64_from_cells(size_hi, size_lo) as usize;

                    let image_end = &__kernel_image_end as *const _ as usize;
                    let usable_base = max(base, image_end);
                    if usable_base < base + size {
                        ram_areas.push(RamArea {
                            base: PAddr::new(usable_base),
                            len: base + size - usable_base,
                        });
                    }
                }

                if in_chosen && prop_name == "bootargs" && prop_len > 1 {
                    let s = core::slice::from_raw_parts(prop_data, prop_len.saturating_sub(1));
                    if let Ok(cs) = core::str::from_utf8(s) {
                        cmdline_str = Some(cs);
                    }
                }

                // CPU node: reg = MPIDR (1 cell = 4 bytes on QEMU virt).
                if in_cpu_node && prop_name == "reg" && prop_len >= 4 && !cpu_mpdirs.is_full() {
                    let mpidr = u32::from_be(*(prop_data as *const u32)) as u64;
                    let _ = cpu_mpdirs.try_push(mpidr);
                }

                // virtio_mmio@ADDR: reg = <addr_hi addr_lo size_hi size_lo>
                // (#address-cells=2, #size-cells=2 at root), interrupts =
                // <type spi_num flags> where type=0 means SPI.
                if in_virtio_mmio && prop_name == "reg" && prop_len >= 16 {
                    let base_hi = *(prop_data as *const u32);
                    let base_lo = *(prop_data.add(4) as *const u32);
                    vmmio_base = Some(be64_from_cells(base_hi, base_lo) as usize);
                }
                if in_virtio_mmio && prop_name == "interrupts" && prop_len >= 12 {
                    let irq_type = be32(*(prop_data as *const u32));
                    let spi_num = be32(*(prop_data.add(4) as *const u32));
                    // SPI (type=0) → GIC INTID = SPI + 32.  Ignore PPI etc.
                    if irq_type == 0 {
                        let intid = (spi_num + 32) as u8;
                        vmmio_irq = Some(intid);
                    }
                }

                // fw-cfg@ADDR: reg = <addr_hi addr_lo size_hi size_lo>
                // Only the base matters for ramfb's port-mode access.
                if in_fw_cfg && prop_name == "reg" && prop_len >= 16 {
                    let base_hi = *(prop_data as *const u32);
                    let base_lo = *(prop_data.add(4) as *const u32);
                    fw_cfg_base = Some(PAddr::new(be64_from_cells(base_hi, base_lo) as usize));
                }

                offset += (prop_len + 3) & !3; // align to 4
            }
            9 => {
                // FDT_END
                break;
            }
            4 => {
                // FDT_NOP
            }
            _ => break,
        }
    }

    if ram_areas.is_empty() {
        return default_boot_info();
    }

    let cmdline_raw = cmdline_str.unwrap_or("");
    let cmdline = parse_cmdline(cmdline_raw);
    // Capture the raw cmdline so /proc/cmdline shows what userspace
    // was actually given (including custom flags like kbox-phase=N
    // that we don't structurally parse).
    let mut raw_cmdline = ArrayString::<512>::new();
    let _ = raw_cmdline.try_push_str(cmdline_raw);
    // Merge DTB-discovered virtio-mmio devices with any passed via cmdline
    // (`virtio_mmio.device=SIZE@ADDR:IRQ`).  DTB entries come first, as the
    // hypervisor-authoritative source; cmdline entries act as overrides.
    let mut merged_vmmio = virtio_mmio_devices;
    for dev in cmdline.virtio_mmio_devices.iter() {
        if !merged_vmmio.is_full() {
            let _ = merged_vmmio.try_push(VirtioMmioDevice {
                mmio_base: dev.mmio_base,
                irq: dev.irq,
            });
        }
    }
    println!("virtio-mmio: discovered {} device(s) from DTB", merged_vmmio.len());
    if let Some(p) = fw_cfg_base {
        println!("fw-cfg: discovered MMIO base {:#x} from DTB", p.value());
    }
    BootInfo {
        ram_areas,
        pci_enabled: cmdline.pci_enabled,
        pci_allowlist: cmdline.pci_allowlist,
        virtio_mmio_devices: merged_vmmio,
        fw_cfg_base,
        log_filter: cmdline.log_filter,
        use_second_serialport: false,
        dhcp_enabled: cmdline.dhcp_enabled,
        ip4: cmdline.ip4,
        gateway_ip4: cmdline.gateway_ip4,
        cpu_mpdirs,
        init_path: cmdline.init_path,
        debug_filter: cmdline.debug_filter,
        strace_pid: cmdline.strace_pid,
        strace_comm: cmdline.strace_comm,
        epoll_trace_fd: cmdline.epoll_trace_fd,
        raw_cmdline,
    }
}

/// Default boot info for QEMU virt: 1GB RAM starting at 0x40000000.
/// Matches the `-m 1024` flag used in compare-contracts.py and run-qemu.py.
pub fn default_boot_info() -> BootInfo {
    let image_end = unsafe { &__kernel_image_end as *const _ as usize };
    let ram_base = max(0x40000000, image_end);
    let ram_end = 0x40000000 + 0x40000000; // 1GB

    let mut ram_areas = ArrayVec::new();
    ram_areas.push(RamArea {
        base: PAddr::new(ram_base),
        len: ram_end - ram_base,
    });

    // Skip virtio-mmio probing in default mode — each probe is ~1.5s under
    // TCG emulation (32 probes = ~48s, exceeds test timeout).  The kernel
    // can live without virtio when running contract tests via initramfs.
    // Real-hardware or DTB-based boots will discover devices via the DTB.
    let virtio_mmio_devices = ArrayVec::new();

    BootInfo {
        ram_areas,
        pci_enabled: false,
        pci_allowlist: ArrayVec::new(),
        virtio_mmio_devices,
        fw_cfg_base: None,
        log_filter: ArrayString::new(),
        use_second_serialport: false,
        dhcp_enabled: true,
        ip4: None,
        gateway_ip4: None,
        cpu_mpdirs: ArrayVec::new(),
        init_path: None,
        debug_filter: ArrayString::new(),
        strace_pid: None,
        strace_comm: None,
        epoll_trace_fd: None,
        raw_cmdline: ArrayString::new(),
    }
}

/// Scan for a DTB (magic 0xd00dfeed).  QEMU virt places the DTB below the
/// RAM base (0x40000000) for ELF kernels.  The boot page table maps all of
/// 0–4GB so reads below RAM are safe (device memory, AttrIndx=0).
unsafe fn scan_for_dtb() -> Option<PAddr> {
    // Scan downward from 0x40000000 — QEMU places DTB below RAM base.
    let mut addr = 0x4000_0000usize;
    while addr >= 0x1000 {
        addr -= 0x1000;
        let vaddr = addr + super::KERNEL_BASE_ADDR;
        let val = unsafe { core::ptr::read_volatile(vaddr as *const u32) };
        if be32(val) == DTB_MAGIC {
            return Some(PAddr::new(addr));
        }
        // Don't go below 0x08000000 (deep in peripheral territory).
        if addr <= 0x0800_0000 {
            break;
        }
    }
    // Also scan first 128MB of RAM (safe even with -m 256).
    addr = 0x4000_0000;
    let end = 0x4800_0000usize;
    while addr < end {
        let vaddr = addr + super::KERNEL_BASE_ADDR;
        let val = unsafe { core::ptr::read_volatile(vaddr as *const u32) };
        if be32(val) == DTB_MAGIC {
            return Some(PAddr::new(addr));
        }
        addr += 0x1000;
    }
    None
}

unsafe fn read_cstr<'a>(ptr: *const u8) -> &'a str {
    let mut len = 0;
    while *ptr.add(len) != 0 {
        len += 1;
        if len > 256 {
            break;
        }
    }
    core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr, len))
}

struct ParsedCmdline {
    pci_enabled: bool,
    pci_allowlist: ArrayVec<crate::bootinfo::AllowedPciDevice, 4>,
    virtio_mmio_devices: ArrayVec<crate::bootinfo::VirtioMmioDevice, 32>,
    log_filter: ArrayString<64>,
    dhcp_enabled: bool,
    ip4: Option<ArrayString<18>>,
    gateway_ip4: Option<ArrayString<15>>,
    init_path: Option<ArrayString<128>>,
    debug_filter: ArrayString<64>,
    strace_pid: Option<i32>,
    strace_comm: Option<ArrayString<16>>,
    epoll_trace_fd: Option<i32>,
}

fn parse_cmdline(s: &str) -> ParsedCmdline {
    info!("cmdline: {}", if s.is_empty() { "(empty)" } else { s });

    let mut result = ParsedCmdline {
        pci_enabled: false, // No PCI on QEMU virt ARM64.
        pci_allowlist: ArrayVec::new(),
        virtio_mmio_devices: ArrayVec::new(),
        log_filter: ArrayString::new(),
        dhcp_enabled: true,
        ip4: None,
        gateway_ip4: None,
        init_path: None,
        debug_filter: ArrayString::new(),
        strace_pid: None,
        strace_comm: None,
        epoll_trace_fd: None,
    };

    for config in s.split(' ') {
        if config.is_empty() {
            continue;
        }
        let mut words = config.splitn(2, '=');
        match (words.next(), words.next()) {
            (Some("log"), Some(value)) => {
                let _ = result.log_filter.try_push_str(value);
            }
            (Some("virtio_mmio.device"), Some(value)) => {
                if let Some((_size, rest)) = value.split_once("@0x") {
                    if let Some((addr, irq)) = rest.split_once(':') {
                        if let (Ok(addr), Ok(irq)) = (usize::from_str_radix(addr, 16), irq.parse()) {
                            result.virtio_mmio_devices.push(crate::bootinfo::VirtioMmioDevice {
                                mmio_base: PAddr::new(addr),
                                irq,
                            });
                        }
                    }
                }
            }
            (Some("dhcp"), Some("off")) => {
                result.dhcp_enabled = false;
            }
            (Some("init"), Some(value)) => {
                info!("bootinfo: init path = \"{}\"", value);
                let mut s = ArrayString::new();
                if s.try_push_str(value).is_ok() {
                    result.init_path = Some(s);
                }
            }
            (Some("debug"), Some(value)) => {
                let _ = result.debug_filter.try_push_str(value);
            }
            (Some("strace-pid"), Some(value)) => {
                if let Ok(pid) = value.parse() {
                    info!("bootinfo: strace-pid = {}", pid);
                    result.strace_pid = Some(pid);
                }
            }
            (Some("strace-comm"), Some(value)) => {
                let mut s = ArrayString::new();
                if s.try_push_str(value).is_ok() {
                    info!("bootinfo: strace-comm = {}", value);
                    result.strace_comm = Some(s);
                }
            }
            (Some("epoll-trace-fd"), Some(value)) => {
                if let Ok(fd) = value.parse() {
                    info!("bootinfo: epoll-trace-fd = {}", fd);
                    result.epoll_trace_fd = Some(fd);
                }
            }
            _ => {}
        }
    }

    result
}
