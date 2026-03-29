// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use kevlar_utils::alignment::align_down;

use super::vm::VmAreaType;
use crate::{
    debug::{self, DebugEvent, DebugFilter},
    fs::opened_file::OpenOptions,
    process::{
        current_process,
        signal::{self, SigAction, SIGKILL, SIGSEGV},
        Process,
    },
};

/// Deliver SIGSEGV to the current process. If the default action is Terminate
/// (no user handler installed), kill the process immediately instead of
/// queuing the signal. This prevents infinite fault loops where the faulting
/// instruction is retried after the page fault handler returns.
/// Use for unrecoverable faults (invalid address, no VMA).
fn deliver_sigsegv_fatal() {
    let current = current_process();
    let pid = current.pid().as_i32();
    let action = current.signals().lock_no_irq().get_action(SIGSEGV);
    if matches!(action, SigAction::Terminate) {
        Process::exit_by_signal(SIGSEGV);
    } else {
        warn!("SIGSEGV->handler pid={}", pid);
        current.send_signal(SIGSEGV);
    }
}
use core::cmp::min;
use core::sync::atomic::{AtomicU64, Ordering};
use hashbrown::HashMap;
use kevlar_platform::{
    address::{PAddr, UserVAddr},
    arch::{PageFaultReason, PAGE_SIZE, HUGE_PAGE_SIZE},
    page_allocator::{alloc_page, alloc_page_batch, alloc_huge_page, alloc_huge_page_prezeroed, AllocPageFlags},
    page_ops::{zero_page, zero_huge_page},
    spinlock::SpinLock,
};
#[cfg(not(feature = "profile-fortress"))]
use kevlar_platform::page_ops::page_as_slice_mut;
#[cfg(feature = "profile-fortress")]
use kevlar_platform::page_ops::PageFrame;

use crate::process::signal::Signal;

// --- Page fault profiling counters ---
pub static PAGE_FAULT_COUNT: AtomicU64 = AtomicU64::new(0);
pub static PAGE_FAULT_CYCLES: AtomicU64 = AtomicU64::new(0);

// --- Initramfs page cache ---
// Key: (file_data_ptr, page_index_in_file) → cached physical page.
// Only used for immutable files (initramfs). Entries never need invalidation.
pub static PAGE_CACHE: SpinLock<Option<HashMap<(usize, usize), PAddr>>> = SpinLock::new(None);

/// Monotonically increasing generation counter for the page cache.
/// Incremented on every page_cache_insert. Used by the prefault template
/// to detect when the cache has grown and the template is stale.
pub static PAGE_CACHE_GEN: AtomicU64 = AtomicU64::new(0);

/// Cache of 2MB huge pages assembled from PAGE_CACHE entries during exec prefaulting.
/// Key: (file_data_ptr, huge_page_index) where index = huge_vaddr / HUGE_PAGE_SIZE.
/// Value: (PAddr, [u64; 8] bitmap) — base physical address + which sub-pages have content.
/// The bitmap tracks which of the 512 sub-pages were populated (from cache or file).
/// Only populated sub-pages should be mapped; others should be demand-faulted.
pub static HUGE_PAGE_CACHE: SpinLock<Option<HashMap<(usize, usize), (PAddr, [u64; 8])>>> = SpinLock::new(None);

pub fn page_cache_lookup(file_ptr: usize, page_index: usize) -> Option<PAddr> {
    let cache = PAGE_CACHE.lock_no_irq();
    cache.as_ref().and_then(|map| map.get(&(file_ptr, page_index)).copied())
}

fn page_cache_insert(file_ptr: usize, page_index: usize, paddr: PAddr) {
    let mut cache = PAGE_CACHE.lock_no_irq();
    let map = cache.get_or_insert_with(HashMap::new);
    map.insert((file_ptr, page_index), paddr);
    PAGE_CACHE_GEN.fetch_add(1, Ordering::Relaxed);
}

pub fn huge_page_cache_lookup(file_ptr: usize, huge_index: usize) -> Option<(PAddr, [u64; 8])> {
    let cache = HUGE_PAGE_CACHE.lock_no_irq();
    cache.as_ref().and_then(|map| map.get(&(file_ptr, huge_index)).copied())
}

pub fn huge_page_cache_insert(file_ptr: usize, huge_index: usize, paddr: PAddr, bitmap: [u64; 8]) {
    let mut cache = HUGE_PAGE_CACHE.lock_no_irq();
    let map = cache.get_or_insert_with(HashMap::new);
    map.insert((file_ptr, huge_index), (paddr, bitmap));
}

/// Emit a CrashReport debug event and then kill the process.
///
/// Collects the per-process syscall trace, VMA map, and fsbase before
/// calling `Process::exit_by_signal`. This gives us rich crash diagnostics
/// with zero runtime cost when no crash occurs.
fn emit_crash_and_exit(signal: Signal, fault_addr: usize, ip: usize) -> ! {
    let current = current_process();
    let pid = current.pid().as_i32();
    let cmdline = current.cmdline();
    let signal_name = debug::signal_name(signal);

    // Per-process syscall trace.
    let trace = current.dump_trace();
    let mut sc_tuples: [(u16, i32, u32, u32); 32] = [(0, 0, 0, 0); 32];
    let sc_count = core::cmp::min(trace.len(), 32);
    for (i, e) in trace.iter().enumerate().take(sc_count) {
        sc_tuples[i] = (e.nr, e.result, e.arg0, e.arg1);
    }

    // VMA map (up to 64 entries). Skip if the VM lock is already held
    // to avoid deadlock (best-effort in crash path).
    let mut vma_buf: [(usize, usize, &str); 64] = [(0, 0, ""); 64];
    let mut vma_count = 0;
    if let Some(vm_arc) = current.vm().as_ref() {
        if !vm_arc.is_locked() {
            let vm_guard = vm_arc.lock_no_irq();
            for vma in vm_guard.vm_areas().iter() {
                if vma_count >= 64 {
                    break;
                }
                let vt = match vma.area_type() {
                    VmAreaType::Anonymous => "anon",
                    VmAreaType::File { .. } => "file",
                };
                vma_buf[vma_count] = (vma.start().value(), vma.end().value(), vt);
                vma_count += 1;
            }
        }
    }

    let fsbase = current.arch().fsbase() as usize;

    // Read stashed registers from the interrupt handler.
    let cpu = kevlar_platform::arch::cpu_id() as usize;
    let regs = kevlar_platform::crash_regs::take(cpu);
    let r = regs.unwrap_or(kevlar_platform::crash_regs::CrashRegs {
        rax: 0, rbx: 0, rcx: 0, rdx: 0,
        rsi: 0, rdi: 0, rbp: 0, rsp: 0,
        r8: 0, r9: 0, r10: 0, r11: 0,
        r12: 0, r13: 0, r14: 0, r15: 0,
        rip: 0, rflags: 0, fault_addr: 0,
    });

    debug::emit(DebugFilter::FAULT, &DebugEvent::CrashReport {
        pid,
        signal: signal as i32,
        signal_name,
        cmdline: cmdline.as_str(),
        fault_addr,
        ip,
        fsbase,
        rax: r.rax, rbx: r.rbx, rcx: r.rcx, rdx: r.rdx,
        rsi: r.rsi, rdi: r.rdi, rbp: r.rbp, rsp: r.rsp,
        r8: r.r8, r9: r.r9, r10: r.r10, r11: r.r11,
        r12: r.r12, r13: r.r13, r14: r.r14, r15: r.r15,
        rflags: r.rflags,
        syscalls: &sc_tuples[..sc_count],
        vmas: &vma_buf[..vma_count],
    });

    drop(cmdline);
    Process::exit_by_signal(signal);
}

pub fn handle_page_fault(unaligned_vaddr: Option<UserVAddr>, ip: usize, _reason: PageFaultReason) {
    // ktrace: record page fault event with address and IP.
    #[cfg(feature = "ktrace-mm")]
    {
        let addr = unaligned_vaddr.map_or(0usize, |v| v.value());
        crate::debug::ktrace::trace(
            crate::debug::ktrace::event::PAGE_FAULT,
            addr as u32,
            (addr >> 32) as u32,
            ip as u32,
            (ip >> 32) as u32,
            _reason.bits() as u32,
        );
    }

    // Hierarchical tracer: record page fault with faulting address.
    let _htrace_guard = crate::debug::htrace::enter_guard(
        crate::debug::htrace::id::PAGE_FAULT,
        unaligned_vaddr.map_or(0, |v| v.value() as u32),
    );

    // Profile page fault latency when the syscall profiler is enabled.
    let pf_start = if crate::debug::profiler::is_enabled() {
        kevlar_platform::arch::read_clock_counter()
    } else {
        0
    };

    handle_page_fault_inner(unaligned_vaddr, ip, _reason);

    if pf_start != 0 {
        let elapsed = kevlar_platform::arch::read_clock_counter().saturating_sub(pf_start);
        PAGE_FAULT_COUNT.fetch_add(1, Ordering::Relaxed);
        PAGE_FAULT_CYCLES.fetch_add(elapsed, Ordering::Relaxed);
    }
}

fn handle_page_fault_inner(unaligned_vaddr: Option<UserVAddr>, ip: usize, _reason: PageFaultReason) {
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
            let fsbase = current_process().arch().fsbase();
            warn!(
                "SIGSEGV: null pointer access (pid={}, ip={:#x}, fsbase={:#x})",
                pid, ip, fsbase
            );
            // Full register dump for crash investigation.
            #[cfg(target_arch = "x86_64")]
            {
                let cpu = kevlar_platform::arch::cpu_id() as usize;
                if let Some(r) = kevlar_platform::crash_regs::take(cpu) {
                    warn!("  RAX={:#x} RBX={:#x} RCX={:#x} RDX={:#x}",
                        r.rax, r.rbx, r.rcx, r.rdx);
                    warn!("  RSI={:#x} RDI={:#x} RBP={:#x} RSP={:#x}",
                        r.rsi, r.rdi, r.rbp, r.rsp);
                    warn!("  R8={:#x}  R9={:#x}  R10={:#x} R11={:#x}",
                        r.r8, r.r9, r.r10, r.r11);
                    warn!("  R12={:#x} R13={:#x} R14={:#x} R15={:#x}",
                        r.r12, r.r13, r.r14, r.r15);
                    warn!("  RIP={:#x} RFLAGS={:#x} fault_addr={:#x}",
                        r.rip, r.rflags, r.fault_addr);
                }
            }
            deliver_sigsegv_fatal();
            return;
        }
    };

    let current = current_process();
    let aligned_vaddr = match UserVAddr::new_nonnull(align_down(unaligned_vaddr.value(), PAGE_SIZE))
    {
        Ok(uaddr) => uaddr,
        _ => {
            let pid = current_process().pid().as_i32();
            warn!(
                "SIGSEGV: invalid address {:#x} (pid={}, ip={:#x})",
                unaligned_vaddr.value(), pid, ip
            );
            // Dump registers and syscall trace for crash investigation.
            if pid > 2 {
                let cpu = kevlar_platform::arch::cpu_id() as usize;
                if let Some(r) = kevlar_platform::crash_regs::take(cpu) {
                    warn!("  RDI={:#x} RSI={:#x} RBP={:#x} RSP={:#x}",
                        r.rdi, r.rsi, r.rbp, r.rsp);
                }
                // Dump last syscalls for this process
                let trace = current.dump_trace();
                if !trace.is_empty() {
                    warn!("  last {} syscalls:", trace.len());
                    for e in trace.iter().rev().take(8) {
                        warn!("    nr={} result={} a0={:#x} a1={:#x}", e.nr, e.result, e.arg0, e.arg1);
                    }
                }
                // Dump VMAs near the crash address
                if let Some(vm_ref) = current.vm().as_ref() {
                    let vm = vm_ref.lock();
                    let vmas = vm.vm_areas_ref();
                    warn!("  VMAs total: {}", vmas.len());
                    // Show VMAs containing or near the crash IP
                    let crash_ip_page = ip & !0xFFF;
                    for vma in vmas.iter() {
                        let s = vma.start().value();
                        let e = vma.end().value();
                        // Show if contains the crash IP or fault address
                        if (s <= ip && ip < e) || (s <= unaligned_vaddr.value() && unaligned_vaddr.value() < e) {
                            warn!("  * VMA {:#x}-{:#x} prot={:#x} CONTAINS target", s, e, vma.prot().bits());
                        }
                    }
                    // Show first text (prot=5) VMAs
                    let mut text_count = 0;
                    for vma in vmas.iter() {
                        if vma.prot().bits() == 0x5 {
                            warn!("  text VMA: {:#x}-{:#x}", vma.start().value(), vma.end().value());
                            text_count += 1;
                            if text_count >= 5 { break; }
                        }
                    }
                }
            }
            deliver_sigsegv_fatal();
            return;
        }
    };

    // Allocate a zeroed page BEFORE acquiring the VM lock.
    // This keeps the lock hold time minimal (just VMA lookup + PTE write).
    // alloc_page without DIRTY_OK serves pre-zeroed pages from the pool
    // (~5ns) or falls back to alloc+memset (~1-2µs).
    let mut paddr = match alloc_page(AllocPageFlags::USER) {
        Ok(p) => p,
        Err(_) => {
            debug_warn!(
                "pid={}: OOM during page fault at {} (ip={:x}), killing process",
                current.pid().as_i32(), unaligned_vaddr, ip
            );
            Process::exit_by_signal(SIGKILL);
        }
    };

    // Look for the associated vma area.
    let vm_ref = current.vm();
    let mut vm = vm_ref.as_ref().unwrap().lock_no_irq();


    let vma = match vm.find_vma_cached(unaligned_vaddr) {
        Some(vma) => vma,
        None => {
            let pid = current.pid().as_i32();
            // Always dump VMAs on SIGSEGV for dlopen/Alpine investigation.
            {
                warn!("PAGE FAULT NO VMA: pid={} addr={:#x} ip={:#x} reason={:?}",
                      pid, unaligned_vaddr.value(), ip, _reason);
                let vma_count = vm.vm_areas().len();
                // Find the highest and nearest VMAs to the fault address
                let fault_val = unaligned_vaddr.value();
                let mut nearest_below: Option<(usize, usize)> = None; // (idx, end)
                let mut nearest_above: Option<(usize, usize)> = None; // (idx, start)
                for (i, vma) in vm.vm_areas().iter().enumerate() {
                    let vs = vma.start().value();
                    let ve = vs + vma.len();
                    if ve <= fault_val {
                        if nearest_below.is_none() || ve > nearest_below.unwrap().1 {
                            nearest_below = Some((i, ve));
                        }
                    }
                    if vs > fault_val {
                        if nearest_above.is_none() || vs < nearest_above.unwrap().1 {
                            nearest_above = Some((i, vs));
                        }
                    }
                }
                warn!("  VMA count={} fault={:#x}", vma_count, fault_val);
                if let Some((idx, end)) = nearest_below {
                    let vma = &vm.vm_areas()[idx];
                    warn!("  nearest_below[{}]: [{:#x}-{:#x}] gap={:#x}",
                          idx, vma.start().value(), end, fault_val - end);
                }
                if let Some((idx, start)) = nearest_above {
                    let vma = &vm.vm_areas()[idx];
                    warn!("  nearest_above[{}]: [{:#x}-{:#x}] gap={:#x}",
                          idx, start, start + vma.len(), start - fault_val);
                }
                // Show the last 15 VMAs (highest addresses) with file offsets for dlopen debug
                let start_idx = if vma_count > 15 { vma_count - 15 } else { 0 };
                for (i, vma) in vm.vm_areas().iter().enumerate().skip(start_idx) {
                    match vma.area_type() {
                        VmAreaType::Anonymous => {
                            warn!("  VMA[{}]: [{:#x}-{:#x}] prot={:#x} anon",
                                  i, vma.start().value(), vma.end().value(), vma.prot().bits());
                        }
                        VmAreaType::File { file, offset, file_size } => {
                            let fp = alloc::sync::Arc::as_ptr(file) as *const () as usize;
                            warn!("  VMA[{}]: [{:#x}-{:#x}] prot={:#x} file off={:#x} fsz={:#x} fp={:#x}",
                                  i, vma.start().value(), vma.end().value(), vma.prot().bits(),
                                  offset, file_size, fp);
                        }
                    }
                }
            }
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
            warn!(
                "SIGSEGV: no VMA for address {:#x} (pid={}, ip={:#x}, reason={:?})",
                unaligned_vaddr.value(), pid, ip, _reason
            );
            // Dump registers for crash investigation.
            if pid > 2 {
                let cpu = kevlar_platform::arch::cpu_id() as usize;
                if let Some(r) = kevlar_platform::crash_regs::take(cpu) {
                    warn!("  RDI={:#x} RSI={:#x} RBP={:#x} RSP={:#x} RAX={:#x}",
                        r.rdi, r.rsi, r.rbp, r.rsp, r.rax);
                }
            }
            // Free the page we allocated since we won't map it.
            kevlar_platform::page_allocator::free_pages(paddr, 1);
            drop(vm);
            drop(vm_ref);
            // Deliver SIGSEGV via signal path so userspace handlers can catch it.
            // If no handler is installed, the default action terminates the process.
            deliver_sigsegv_fatal();
            return;
        }
    };

    // Page cache state — set inside the File match arm, used after for refcount init.
    let mut cache_hit = false;
    let mut cache_shared = false; // true when paddr is a shared cached page
    let mut is_cacheable = false;
    let mut offset_in_file = 0;
    let mut cache_file_ptr: usize = 0; // Arc data ptr for cache key

    match vma.area_type() {
        VmAreaType::Anonymous => { /* Zero-filled by zero_page above. */ }
        VmAreaType::File {
            file,
            offset,
            file_size,
        } => {
            let offset_in_page;
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

            // --- Page cache for immutable files (initramfs) ---
            let vma_readonly = vma.prot().bits() & 2 == 0;
            is_cacheable = file.is_content_immutable()
                && offset_in_page == 0
                && copy_len > 0
                && vma_readonly;

            if is_cacheable {
                cache_file_ptr = alloc::sync::Arc::as_ptr(file) as *const () as usize;
                let page_index = offset_in_file / PAGE_SIZE;

                if let Some(cached_paddr) = page_cache_lookup(cache_file_ptr, page_index) {
                    cache_hit = true;
                    let vma_writable = vma.prot().bits() & 2 != 0;
                    if !vma_writable {
                        // Read-only VMA: share the cached physical page directly.
                        kevlar_platform::page_allocator::free_pages(paddr, 1);
                        kevlar_platform::page_refcount::page_ref_inc(cached_paddr);
                        paddr = cached_paddr;
                        cache_shared = true;
                    } else {
                        // Writable VMA: copy cached content to fresh page (CoW-style).
                        #[cfg(not(feature = "profile-fortress"))]
                        {
                            let src = kevlar_platform::page_ops::page_as_slice(cached_paddr);
                            let dst = page_as_slice_mut(paddr);
                            dst.copy_from_slice(src);
                        }
                        #[cfg(feature = "profile-fortress")]
                        {
                            let mut tmp = [0u8; PAGE_SIZE];
                            let src_frame = PageFrame::new(cached_paddr);
                            src_frame.read(0, &mut tmp);
                            let mut dst_frame = PageFrame::new(paddr);
                            dst_frame.write(0, &tmp);
                        }
                    }
                }
            }

            // --- Direct physical mapping (Experiment 3) ---
            let mut direct_mapped = false;
            if !cache_hit && copy_len == PAGE_SIZE && is_cacheable
                && crate::process::DIRECT_MAP_ENABLED.load(core::sync::atomic::Ordering::Relaxed) {
                if let Some(data_base) = file.data_vaddr() {
                    let page_vaddr = data_base + offset_in_file;
                    if page_vaddr % PAGE_SIZE == 0 {
                        let direct_paddr = kevlar_platform::address::VAddr::new(page_vaddr).as_paddr();
                        kevlar_platform::page_refcount::page_ref_init_kernel_image(direct_paddr);
                        kevlar_platform::page_allocator::free_pages(paddr, 1);
                        paddr = direct_paddr;
                        cache_shared = true; // Prevent page_ref_init later.
                        let page_index = offset_in_file / PAGE_SIZE;
                        page_cache_insert(cache_file_ptr, page_index, direct_paddr);
                        direct_mapped = true;
                    }
                }
            }

            if !cache_hit && !direct_mapped && copy_len > 0 {
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

    // If the VMA has PROT_NONE, any access is illegal — deliver SIGSEGV.
    // This covers both demand-paged PROT_NONE VMAs and pages that were
    // mprotect'd to PROT_NONE (PTE has paddr but no PRESENT bit).
    // We send the signal (not exit_by_signal) so that a user-installed
    // SIGSEGV handler can catch the fault.  The interrupt return path
    // (x64_check_signal_on_irq_return) will deliver the signal and
    // redirect RIP to the handler trampoline before IRET.
    let prot_flags = vma.prot().bits();
    if prot_flags == 0 {
        kevlar_platform::page_allocator::free_pages(paddr, 1);
        drop(vm);
        drop(vm_ref);
        warn!(
            "SIGSEGV: PROT_NONE access at {:#x} (pid={}, ip={:#x})",
            unaligned_vaddr.value(), current.pid().as_i32(), ip
        );
        // Use send_signal (not fatal) so user SIGSEGV handlers can catch
        // PROT_NONE faults (e.g., guard pages, mprotect tests).
        current.send_signal(SIGSEGV);
        return;
    }

    // Map the page in the page table, respecting VMA protection flags.
    let vma_start_value = vma.start().value();
    let vma_end_value = vma.end().value();
    let is_anonymous = matches!(vma.area_type(), VmAreaType::Anonymous);
    let vma_is_shared = vma.is_shared();

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

    // --- Huge page fast path (2MB transparent huge pages) ---
    if is_anonymous {
        let huge_base = align_down(aligned_vaddr.value(), HUGE_PAGE_SIZE);
        let huge_end = huge_base + HUGE_PAGE_SIZE;

        if huge_base >= vma_start_value && huge_end <= vma_end_value {
            let huge_vaddr = UserVAddr::new_nonnull(huge_base).unwrap();
            if vm.page_table().is_pde_empty(huge_vaddr) {
                // Fast path: try pre-zeroed pool (zero-cost zeroing).
                let huge_opt = alloc_huge_page_prezeroed().or_else(|| {
                    alloc_huge_page().ok().map(|p| { zero_huge_page(p); p })
                });
                if let Some(huge_paddr) = huge_opt {
                    kevlar_platform::page_refcount::page_ref_init_huge(huge_paddr);
                    vm.page_table_mut().map_huge_user_page(
                        huge_vaddr,
                        huge_paddr,
                        prot_flags,
                    );
                    kevlar_platform::page_allocator::free_pages(paddr, 1);
                    return;
                }
            }
        }
    }

    // If the faulting address is within a 2MB huge page, split it into
    // 512 × 4KB PTEs first, then fall through to the normal 4KB path.
    // This handles both CoW write faults and permission upgrades on huge pages.
    if let Some(_pde_val) = vm.page_table().is_huge_mapped(aligned_vaddr) {
        vm.page_table_mut().split_huge_page(aligned_vaddr);
        vm.page_table().flush_tlb_local(aligned_vaddr);
        // Fall through — the page is now mapped as a 4KB PTE.
    }

    // CRITICAL: Use try_map to detect if the page is already mapped.
    if !vm.page_table_mut()
        .try_map_user_page_with_prot(aligned_vaddr, paddr, prot_flags)
    {
        // ktrace: record that try_map found an existing PTE.
        #[cfg(feature = "ktrace-mm")]
        {
            let va = aligned_vaddr.value();
            let existing_pa = vm.page_table().lookup_paddr(aligned_vaddr)
                .map(|p| p.value()).unwrap_or(0);
            crate::debug::ktrace::trace(
                crate::debug::ktrace::event::PTE_MAP_EXISTING,
                va as u32, (va >> 32) as u32,
                _reason.bits() as u32,
                existing_pa as u32, (existing_pa >> 32) as u32,
            );
        }
        // Page already mapped — we won't use paddr for a new mapping.
        // Page already mapped. Free the fresh page we allocated.
        kevlar_platform::page_allocator::free_pages(paddr, 1);

        // Check for CoW: if the fault was a write to a present page,
        // and the VMA allows writing, this is a CoW page that needs copying.
        let is_cow_write = _reason.contains(PageFaultReason::PRESENT)
            && _reason.contains(PageFaultReason::CAUSED_BY_WRITE)
            && (prot_flags & 2 != 0); // VMA has PROT_WRITE

        if is_cow_write {
            // Get the old physical page from the existing PTE.
            if let Some(old_paddr) = vm.page_table().lookup_paddr(aligned_vaddr) {
                if vma_is_shared {
                    // MAP_SHARED: restore writable without copying.
                    vm.page_table_mut().update_page_flags(aligned_vaddr, prot_flags);
                    vm.page_table().flush_tlb_local(aligned_vaddr);
                    return;
                }
                let is_ghost = vm.is_ghost_forked;
                let refcount = kevlar_platform::page_refcount::page_ref_count(old_paddr);
                // Ghost-forked VMs always CoW-copy: refcounts weren't incremented
                // during fork, so refcount=1 even though the page is shared with
                // the blocked parent. Normal fork: copy only if refcount > 1.
                if refcount > 1 || is_ghost {
                    // Shared page: allocate new page, copy content, map writable.
                    let new_paddr = match alloc_page(AllocPageFlags::USER | AllocPageFlags::DIRTY_OK) {
                        Ok(p) => p,
                        Err(_) => {
                            debug_warn!("OOM during CoW fault");
                            Process::exit_by_signal(SIGKILL);
                        }
                    };
                    #[cfg(not(feature = "profile-fortress"))]
                    {
                        let src = kevlar_platform::page_ops::page_as_slice(old_paddr);
                        let dst = kevlar_platform::page_ops::page_as_slice_mut(new_paddr);
                        dst.copy_from_slice(src);
                    }
                    #[cfg(feature = "profile-fortress")]
                    {
                        let mut tmp = [0u8; PAGE_SIZE];
                        let src_frame = kevlar_platform::page_ops::PageFrame::new(old_paddr);
                        src_frame.read(0, &mut tmp);
                        let mut dst_frame = kevlar_platform::page_ops::PageFrame::new(new_paddr);
                        dst_frame.write(0, &tmp);
                    }
                    kevlar_platform::page_refcount::page_ref_init(new_paddr);
                    if !is_ghost {
                        // Normal fork: decrement old page's refcount.
                        if kevlar_platform::page_refcount::page_ref_dec(old_paddr) {
                            kevlar_platform::page_allocator::free_pages(old_paddr, 1);
                        }
                    }
                    // Ghost fork: DON'T decrement — refcount was never incremented.
                    vm.page_table_mut().map_user_page_with_prot(aligned_vaddr, new_paddr, prot_flags);
                    vm.page_table().flush_tlb_local(aligned_vaddr);
                    return;
                }
                // refcount == 1 and not ghost: sole owner, just make writable.
            }
        }

        // Write fault on a page the VMA doesn't allow writing.
        // For MAP_PRIVATE file-backed pages: do COW (copy page, remap writable).
        // This is needed for the dynamic linker's RELR relocation processing:
        // musl maps the entire library with PROT_READ (reservation), then
        // applies RELR relocations that write to the .data segment pages.
        // Some RELR bitmaps may overflow into adjacent read-only pages.
        // On Linux, the dynamic linker's writes to MAP_PRIVATE read-only
        // pages succeed via the COW mechanism.
        if _reason.contains(PageFaultReason::CAUSED_BY_WRITE) && (prot_flags & 2 == 0) {
            if !vma_is_shared && !is_anonymous {
                // MAP_PRIVATE file-backed: COW-copy the page and make it writable.
                // This handles musl's RELR relocations writing to read-only
                // file-mapped pages. Anonymous pages with PROT_READ should SIGSEGV.
                if let Some(old_paddr) = vm.page_table().lookup_paddr(aligned_vaddr) {
                    let new_paddr = match alloc_page(AllocPageFlags::USER | AllocPageFlags::DIRTY_OK) {
                        Ok(p) => p,
                        Err(_) => {
                            drop(vm); drop(vm_ref);
                            Process::exit_by_signal(SIGKILL);
                        }
                    };
                    #[cfg(not(feature = "profile-fortress"))]
                    {
                        let src = kevlar_platform::page_ops::page_as_slice(old_paddr);
                        let dst = kevlar_platform::page_ops::page_as_slice_mut(new_paddr);
                        dst.copy_from_slice(src);
                    }
                    kevlar_platform::page_refcount::page_ref_init(new_paddr);
                    // Unmap old page, map new writable copy
                    vm.page_table_mut().unmap_user_page(aligned_vaddr);
                    vm.page_table_mut().map_user_page_with_prot(
                        aligned_vaddr, new_paddr, prot_flags | 2);
                    vm.page_table().flush_tlb_local(aligned_vaddr);
                    if kevlar_platform::page_refcount::page_ref_dec(old_paddr) {
                        kevlar_platform::page_allocator::free_pages(old_paddr, 1);
                    }
                    return;
                }
            }
            // MAP_SHARED or no existing page: genuine SIGSEGV
            drop(vm);
            drop(vm_ref);
            deliver_sigsegv_fatal();
            return;
        }

        // Not a CoW fault (or sole owner): just update the PTE flags.
        vm.page_table_mut().update_page_flags(aligned_vaddr, prot_flags);
        vm.page_table().flush_tlb_local(aligned_vaddr);

        return;
    }

    // Flush TLB after writing a new PTE.
    //
    // On real ARM64 hardware a new demand-paged mapping (no prior TLB entry)
    // does not require TLBI — the hardware page-table walker sees the PTE
    // after DSB.  But QEMU TCG caches a "fault" TLB entry when the initial
    // access misses (before the PTE is written), and that stale entry persists
    // through ERET unless explicitly invalidated.
    //
    // `tlbi vale1` (flush_tlb_local) was tried but proved insufficient:
    // QEMU TCG's tlb_flush_page() does not reliably clear fault-type entries.
    // `tlbi vmalle1` (flush_tlb_all) flushes the entire softmmu TLB and
    // reliably clears all stale entries including fault/not-present ones.
    //
    // Linux always calls flush_tlb_page() / update_mmu_cache() after writing
    // a demand-paged PTE; we must do the same for QEMU correctness.
    vm.page_table().flush_tlb_all();

    // ktrace: record successful PTE mapping + page content sample.
    #[cfg(feature = "ktrace-mm")]
    {
        let va = aligned_vaddr.value();
        let pa = paddr.value();
        crate::debug::ktrace::trace(
            crate::debug::ktrace::event::PTE_MAP,
            va as u32, (va >> 32) as u32,
            pa as u32, (pa >> 32) as u32,
            prot_flags as u32,
        );
        // Sample page content right after PTE install.
        let b = kevlar_platform::page_ops::page_as_slice(paddr);
        let word = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        crate::debug::ktrace::trace(
            crate::debug::ktrace::event::PAGE_CONTENT,
            pa as u32, (pa >> 32) as u32,
            word, 0 /* context: 0=after_map */, 0,
        );
    }

    // Initialize the page's reference count to 1 (sole owner).
    // CoW fork will increment to 2 when sharing the page.
    // Skip for shared cached pages — their refcount is already managed.
    if !cache_shared {
        kevlar_platform::page_refcount::page_ref_init(paddr);

        // Insert into page cache on miss. Must happen AFTER page_ref_init so
        // the process mapping has refcount 1, then the cache bumps it to 2.
        if is_cacheable && !cache_hit {
            let page_index = offset_in_file / PAGE_SIZE;
            kevlar_platform::page_refcount::page_ref_inc(paddr);
            page_cache_insert(cache_file_ptr, page_index, paddr);
        }
    }

    // Fault-around: for anonymous mappings, prefault adjacent pages to reduce
    // the number of page faults (and their associated exception + EPT overhead).
    // This mirrors Linux's fault_around_bytes behavior.
    // Batch-allocate pages to amortize allocator lock overhead.
    //
    // IMPORTANT: Do NOT prefault across a 2MB boundary. Doing so would
    // pre-populate PTEs in the next PDE, preventing a future huge page
    // mapping in that region.
    if is_anonymous {
        use kevlar_platform::address::PAddr;
        const FAULT_AROUND_PAGES: usize = 16;

        // Count how many pages we can prefault (up to VMA end).
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
            // Allocate zeroed pages: drains prezeroed pool first, then
            // falls back to dirty batch + memset for the remainder.
            let mut allocated = 0;
            // Drain prezeroed pool.
            if kevlar_platform::page_allocator::prezeroed_4k_count() > 0 {
                while allocated < num_prefault {
                    if let Some(p) = kevlar_platform::page_allocator::alloc_page_prezeroed() {
                        pages[allocated] = p;
                        allocated += 1;
                    } else {
                        break;
                    }
                }
            }
            // Remaining from dirty batch + zero.
            if allocated < num_prefault {
                let batch = alloc_page_batch(&mut pages[allocated..], num_prefault - allocated);
                for i in allocated..(allocated + batch) {
                    zero_page(pages[i]);
                }
                allocated += batch;
            }

            // Initialize refcounts.
            for i in 0..allocated {
                kevlar_platform::page_refcount::page_ref_init(pages[i]);
            }

            // Batch-map: one page table traversal for all pages.
            let start_addr = UserVAddr::new(aligned_vaddr.value() + PAGE_SIZE).unwrap();
            let mapped = vm.page_table_mut().batch_try_map_user_pages_with_prot(
                start_addr, &pages, allocated, prot_flags,
            );

            // Free any pages that weren't mapped (PTE already occupied).
            for i in 0..allocated {
                if mapped & (1 << i) == 0 {
                    if kevlar_platform::page_refcount::page_ref_dec(pages[i]) {
                        kevlar_platform::page_allocator::free_pages(pages[i], 1);
                    }
                }
            }
        }
    }
}
