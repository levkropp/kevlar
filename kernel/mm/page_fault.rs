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

/// Scan the 4KB user page at `user_va` for kernel-direct-map-shaped
/// 8-byte values.  Reports density + offsets + paddr distribution.
/// Used by task #25's KERNEL_PTR_LEAK detector to answer the critical
/// question: did one pointer coincidentally land here, or is this page
/// fulla-kernel-data (= stale TLB to a recycled kernel page)?
///
/// Reads via the kernel direct map of the user's paddr, which is
/// always safe to read (kernel straight-map is RO-readable everywhere).
#[allow(unsafe_code)]
fn scan_user_page_for_kernel_ptrs(user_va: u64, gpr_name: &str) {
    use kevlar_platform::address::UserVAddr;
    let page_va = user_va & !0xFFF;
    let Some(uva) = UserVAddr::new(page_va as usize) else { return };
    let current = current_process();
    let vm_ref = current.vm();
    let Some(vm_arc) = vm_ref.as_ref() else { return };
    let vm = vm_arc.lock_no_irq();
    let Some(paddr) = vm.page_table().lookup_paddr(uva) else { return };
    drop(vm);
    // Safe to read via kernel straight-map: paddr maps to a kernel VA
    // inside [KERNEL_BASE_ADDR, KERNEL_BASE_ADDR + straight_map_end].
    let ptr = paddr.as_ptr::<u64>();
    let mut kptr_count = 0usize;
    let mut first_kptr: Option<(usize, u64)> = None;
    let mut all_paddrs_kernel_image = true;  // are they all < 47MB (kernel image)?
    let mut all_paddrs_kernel_heap = true;   // are they all > 64MB (heap region)?
    for i in 0..(kevlar_platform::arch::PAGE_SIZE / 8) {
        let v = unsafe { core::ptr::read_volatile(ptr.add(i)) };
        if (v >> 47) == 0x1ffff {
            kptr_count += 1;
            if first_kptr.is_none() { first_kptr = Some((i, v)); }
            let target_paddr = v & 0x0000_7fff_ffff_ffff;
            if target_paddr >= 0x3_000_000 { all_paddrs_kernel_image = false; }
            if target_paddr < 0x4_000_000 { all_paddrs_kernel_heap = false; }
        }
    }
    if kptr_count > 0 {
        let region = if all_paddrs_kernel_image { "kernel-image"
                    } else if all_paddrs_kernel_heap { "kernel-heap"
                    } else { "mixed" };
        log::warn!(
            "  LEAK_PAGE_SCAN {}: vaddr={:#x} paddr={:#x} kernel_ptrs={} \
             in {} region (page_size=4096)",
            gpr_name, page_va, paddr.value(), kptr_count, region,
        );
        if let Some((off, val)) = first_kptr {
            log::warn!(
                "    first kernel-VA at +{:#05x} = {:#018x} (target paddr={:#x})",
                off * 8, val, val & 0x0000_7fff_ffff_ffff,
            );
        }
        // Dump offsets of up to 8 kernel-VA words so we can see the
        // pattern (clustered at struct-member offsets vs scattered).
        if kptr_count <= 64 {
            let mut logged = 0;
            for i in 0..(kevlar_platform::arch::PAGE_SIZE / 8) {
                let v = unsafe { core::ptr::read_volatile(ptr.add(i)) };
                if (v >> 47) == 0x1ffff {
                    log::warn!("    [+{:#05x}] = {:#018x}", i * 8, v);
                    logged += 1;
                    if logged >= 8 { break; }
                }
            }
        }
    }
}

/// Deliver SIGSEGV to the current process. If the default action is Terminate
/// (no user handler installed), kill the process immediately instead of
/// queuing the signal. This prevents infinite fault loops where the faulting
/// instruction is retried after the page fault handler returns.
/// Use for unrecoverable faults (invalid address, no VMA).
#[allow(unsafe_code)]
fn deliver_sigsegv_fatal() {
    let current = current_process();
    let pid = current.pid().as_i32();
    let cmdline = current.cmdline();
    // Read the fault address from CR2 and stashed registers for diagnostics.
    let fault_addr = unsafe { x86::controlregs::cr2() };
    let cpu = kevlar_platform::arch::cpu_id() as usize;
    let regs = kevlar_platform::crash_regs::take(cpu);
    warn!("SIGSEGV: pid={} cmd={} fault_addr={:#x}", pid, cmdline.as_str(), fault_addr);
    // Blog 186: a userspace deref of a kernel direct-map pointer means
    // kernel data landed in a user page (stale TLB or missed zero-fill).
    if (fault_addr >> 47) == 0x1ffff {
        warn!(
            "KERNEL_PTR_LEAK: pid={} fault_addr={:#x} — kernel direct-map pointer dereferenced from userspace (paddr={:#x})",
            pid, fault_addr, fault_addr & 0x0000_7fff_ffff_ffff,
        );
    }
    if let Some(r) = regs {
        warn!("  RIP={:#x} RSP={:#x} RBP={:#x} RAX={:#x}", r.rip, r.rsp, r.rbp, r.rax);
        warn!("  RDI={:#x} RSI={:#x} RDX={:#x} RCX={:#x}", r.rdi, r.rsi, r.rdx, r.rcx);
        // Flag any GPR that looks like a kernel direct-map pointer.
        let gprs = [("RIP", r.rip), ("RSP", r.rsp), ("RBP", r.rbp), ("RAX", r.rax),
                    ("RDI", r.rdi), ("RSI", r.rsi), ("RDX", r.rdx), ("RCX", r.rcx)];
        for (name, v) in gprs {
            if (v >> 47) == 0x1ffff {
                warn!("  KERNEL_PTR_LEAK: {}={:#x} is a kernel direct-map pointer (paddr={:#x})",
                      name, v, v & 0x0000_7fff_ffff_ffff);
            }
        }
        // LEAK_PAGE_SCAN: on a KERNEL_PTR_LEAK, scan the user pages
        // pointed to by RDI/RBP/RSI for additional kernel-VA-shaped
        // values.  Whichever user page contained the leaked pointer
        // likely has RDI or RBP pointing at or near it (most musl/
        // glibc allocator paths load via `mov reg, [rdi+off]`).  The
        // kernel-ptr *density* tells us whether the page has one
        // coincidental leaked value or is fulla-kernel-data (= stale
        // TLB to a recycled kernel page).
        if (fault_addr >> 47) == 0x1ffff {
            let user_regs = [("RDI", r.rdi), ("RBP", r.rbp), ("RSI", r.rsi)];
            for (name, v) in user_regs {
                if v > 0x1000 && (v >> 47) == 0 {
                    scan_user_page_for_kernel_ptrs(v, name);
                }
            }
        }
        // If RIP=0 (NULL function pointer call), dump the object at RBP
        // to see which function pointers are initialized vs NULL.
        #[allow(unsafe_code)]
        if r.rip == 0 {
            if r.rsp > 0x1000 && r.rsp < 0x7FFF_FFFF_FFFF {
                let ret_addr = unsafe { *(r.rsp as *const u64) };
                warn!("  NULL call — return addr at [RSP]={:#x}", ret_addr);
            }
            // Dump the object at RBP (the vtable/struct being called through)
            if r.rbp > 0x1000 && r.rbp < 0x7FFF_FFFF_FFFF {
                warn!("  Object at RBP={:#x}:", r.rbp);
                for i in 0..16u64 {
                    let val = unsafe { *((r.rbp + i * 8) as *const u64) };
                    let marker = if i == 8 { " ← [RBP+0x40] = call target" } else { "" };
                    warn!("    [RBP+{:#04x}] = {:#018x}{}", i * 8, val, marker);
                }
            }
        }
    }
    // Task #25: also verify the text page for SIGSEGV — the
    // ip might point into a library's text that was correct at
    // mmap time but got stomped.
    if let Some(r) = &regs {
        if r.rip > 0x1000 {
            verify_text_page_at_ip(r.rip as usize);
        }
    }
    let action = current.signals().lock_no_irq().get_action(SIGSEGV);
    if matches!(action, SigAction::Terminate) {
        Process::exit_by_signal(SIGSEGV);
    } else {
        // If we're already inside a signal handler (signaled_frame_stack is
        // non-empty), a SIGSEGV here means the handler itself faulted.
        // Force-kill to prevent infinite recursion. Linux does this via
        // signal masking during handler execution.
        let in_handler = current.is_in_signal_handler();
        if in_handler {
            warn!("SIGSEGV inside SIGSEGV handler pid={}, force killing", pid);
            Process::exit_by_signal(SIGSEGV);
        }
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

/// Task #25 diagnostic: invoked from the user-fault handler when a
/// process crashes with GP or INVALID_OPCODE.  Walks the current
/// process's VMA list, finds the file-backed text VMA containing
/// `ip`, re-reads the same file offset for 128 bytes, and prints a
/// diff against the live physical frame.  If the diff is non-zero,
/// the page was stomped AFTER it was first mapped — which rules out
/// the page-fault fill path and points at either a user-mode write
/// through a wrong mapping (CoW refcount / PCID race) or a kernel
/// path writing to the wrong physical frame.
pub fn verify_text_page_at_ip(ip: usize) {
    let current = current_process();
    let vm_ref = current.vm();
    let vm_arc = match vm_ref.as_ref() {
        Some(a) => a,
        None => return,
    };
    let ip_vaddr = match UserVAddr::new_nonnull(ip) {
        Ok(v) => v,
        Err(_) => return,
    };
    let mut vm = vm_arc.lock_no_irq();
    let vma = match vm.find_vma_cached(ip_vaddr) {
        Some(v) => v,
        None => {
            warn!("verify_text: no VMA for ip={:#x}", ip);
            return;
        }
    };
    let prot = vma.prot().bits();
    let (file, vma_file_offset, file_size) = match vma.area_type() {
        VmAreaType::File { file, offset, file_size } => {
            (file.clone(), *offset, *file_size)
        }
        _ => {
            warn!("verify_text: VMA is not file-backed (prot={:#x})", prot);
            return;
        }
    };
    // Compute the file offset for the faulting IP.
    let offset_in_vma = vma.offset_in_vma(
        kevlar_platform::address::UserVAddr::new_nonnull(
            align_down(ip, 4096)
        ).unwrap()
    );
    let file_off = vma_file_offset + offset_in_vma;
    // Bounded read of 128 bytes centered on the IP.
    let ip_in_page = ip & 0xfff;
    let read_off_in_page = ip_in_page.saturating_sub(16);
    let read_file_off = file_off + read_off_in_page;
    let mut expected = [0u8; 128];
    let max_len = core::cmp::min(128, file_size.saturating_sub(read_off_in_page));
    let rr = file.read(
        read_file_off,
        (&mut expected[..max_len]).into(),
        &OpenOptions::readwrite(),
    );
    let read_len = match rr {
        Ok(n) => n,
        Err(e) => {
            warn!("verify_text: file read error: {:?}", e);
            return;
        }
    };
    drop(vm);
    // Read the live bytes from the user mapping.
    let mut live = [0u8; 128];
    let page_vaddr = align_down(ip, 4096);
    let live_start = page_vaddr + read_off_in_page;
    #[allow(unsafe_code)]
    for i in 0..read_len {
        let p = (live_start + i) as *const u8;
        live[i] = unsafe { core::ptr::read_volatile(p) };
    }
    // Compare and print first difference plus a full side-by-side.
    let mut first_bad: Option<usize> = None;
    for i in 0..read_len {
        if expected[i] != live[i] {
            first_bad = Some(i);
            break;
        }
    }
    // Look up the physical address backing this page — if it's
    // different from what was originally assigned, the PTE was
    // remapped (CoW / PCID race).  If it's the same, the physical
    // page itself was stomped (use-after-free or DMA write).
    let current_paddr = {
        let vm_ref2 = current.vm();
        let vm2 = vm_ref2.as_ref().unwrap().lock_no_irq();
        vm2.page_table().lookup_paddr(
            UserVAddr::new_nonnull(page_vaddr).unwrap()
        )
    };

    match first_bad {
        None => {
            warn!(
                "verify_text: ip={:#x} file_off={:#x} len={} paddr={:?} — NO DIFF",
                ip, file_off, read_len,
                current_paddr.map(|p| p.value()),
            );
        }
        Some(off) => {
            warn!(
                "verify_text: ip={:#x} file_off={:#x} first_bad_off={} live={:#04x} expected={:#04x} paddr={:?}",
                ip, file_off, off, live[off], expected[off],
                current_paddr.map(|p| p.value()),
            );
            // Dump 32 bytes of each side for context.
            let hex_live = (0..read_len.min(32))
                .map(|i| alloc::format!("{:02x}", live[i]))
                .collect::<alloc::vec::Vec<_>>()
                .join(" ");
            let hex_exp = (0..read_len.min(32))
                .map(|i| alloc::format!("{:02x}", expected[i]))
                .collect::<alloc::vec::Vec<_>>()
                .join(" ");
            warn!("  live:     {}", hex_live);
            warn!("  expected: {}", hex_exp);
        }
    }
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
                    VmAreaType::DeviceMemory { .. } => "device",
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
                    // Dump user stack to find the call chain leading to ip=0
                    #[allow(unsafe_code)]
                    if r.rsp > 0x1000 && r.rsp < 0x7FFF_FFFF_FFFF {
                        warn!("  user stack at rsp={:#x}:", r.rsp);
                        for i in 0..16u64 {
                            let addr = r.rsp + i * 8;
                            let val = unsafe { *(addr as *const u64) };
                            warn!("    [rsp+{:#x}] = {:#018x}", i * 8, val);
                        }
                    }
                    // Dump the calling instruction by reading [rsp] - 5 bytes
                    // (typical call *%rax is 2 bytes: ff d0)
                    #[allow(unsafe_code)]
                    {
                        let ret = if r.rsp > 0x1000 && r.rsp < 0x7FFF_FFFF_FFFF {
                            unsafe { *(r.rsp as *const u64) }
                        } else { 0 };
                        if ret > 0x1000 && ret < 0x7FFF_FFFF_FFFF {
                            let mut bytes = [0u8; 8];
                            for j in 0..8usize {
                                bytes[j] = unsafe { *((ret - 8 + j as u64) as *const u8) };
                            }
                            warn!("  call site (ret_addr-8..ret_addr): {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                                  bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]);
                        }
                    }
                    // Dump the object at RBP — shows which vtable entries are NULL vs populated
                    #[allow(unsafe_code)]
                    if r.rbp > 0x1000 && r.rbp < 0x7FFF_FFFF_FFFF {
                        warn!("  Object at RBP={:#x}:", r.rbp);
                        for i in 0..20u64 {
                            let val = unsafe { *((r.rbp + i * 8) as *const u64) };
                            let marker = if i == 8 { " ← call target (NULL!)" } else { "" };
                            warn!("    [+{:#04x}] = {:#018x}{}", i * 8, val, marker);
                        }
                    }
                }
            }
            // Dump executable VMAs to identify which library the return address is in
            if let Some(vm) = current_process().vm().clone() {
                let lock = vm.lock_no_irq();
                warn!("  executable VMAs:");
                for vma in lock.vm_areas() {
                    if vma.prot().contains(crate::ctypes::MMapProt::PROT_EXEC) {
                        let end = vma.start().value() + vma.len();
                        warn!("    {:012x}-{:012x} r-x", vma.start().value(), end);
                    }
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
            warn!(
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
            // Stack auto-growth: if the fault is just below the stack VMA,
            // grow it downward (Linux does this transparently). This is
            // critical for programs with deep call stacks (Xorg, GTK).
            if vm.try_grow_stack(aligned_vaddr) {
                // Stack grown — the page will be demand-faulted normally.
                // Re-lookup the VMA (it now covers the faulting address).
                match vm.find_vma_cached(unaligned_vaddr) {
                    Some(vma) => vma,
                    None => {
                        // Growth succeeded but still no VMA — shouldn't happen.
                        kevlar_platform::page_allocator::free_pages(paddr, 1);
                        drop(vm); drop(vm_ref);
                        deliver_sigsegv_fatal();
                        return;
                    }
                }
            } else {
            let pid = current.pid().as_i32();
            // Dump instruction bytes at the faulting IP for crash investigation.
            // Only read if the IP page is actually mapped (otherwise the kernel
            // would page-fault while trying to dump diagnostics — crash on crash).
            #[allow(unsafe_code)]
            if ip > 0x1000 && ip < 0x7FFF_FFFF_FFFF
                && (ip & !(PAGE_SIZE - 1)) == ((ip + 15) & !(PAGE_SIZE - 1))
            {
                let mapped = UserVAddr::new(ip & !(PAGE_SIZE - 1))
                    .and_then(|p| vm.page_table().lookup_paddr(p))
                    .is_some();
                if mapped {
                    let mut ibytes = [0u8; 16];
                    for j in 0..16usize {
                        ibytes[j] = unsafe { *((ip + j) as *const u8) };
                    }
                    warn!("  code at ip={:#x}: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                          ip, ibytes[0], ibytes[1], ibytes[2], ibytes[3],
                          ibytes[4], ibytes[5], ibytes[6], ibytes[7],
                          ibytes[8], ibytes[9], ibytes[10], ibytes[11],
                          ibytes[12], ibytes[13], ibytes[14], ibytes[15]);
                }
            }
            // Always dump VMAs on SIGSEGV for dlopen/Alpine investigation.
            {
                warn!("PAGE FAULT NO VMA: pid={} addr={:#x} ip={:#x} reason={:?}",
                      pid, unaligned_vaddr.value(), ip, _reason);
                // Find the VMA containing the IP — if the code page has wrong
                // data (demand paging bug), this shows the file + offset.
                for vma in vm.vm_areas().iter() {
                    let vs = vma.start().value();
                    let ve = vs + vma.len();
                    if ip >= vs && ip < ve {
                        if let VmAreaType::File { file, offset, file_size } = vma.area_type() {
                            let offset_in_vma = ip - vs;
                            let file_off = offset + offset_in_vma;
                            warn!("  IP VMA: [{:#x}-{:#x}] file_off={:#x} (vma_off={:#x} base_off={:#x} fsz={:#x})",
                                  vs, ve, file_off, offset_in_vma, offset, file_size);
                            // Verify page content: look up the PTE, read the
                            // physical page, compare with file content.
                            let page_vaddr = UserVAddr::new_nonnull(ip & !0xFFF).ok();
                            if let Some(pv) = page_vaddr {
                                if let Some(paddr) = vm.page_table().lookup_paddr(pv) {
                                    // Read 8 bytes from the physical page at the IP offset
                                    let page_off = ip & 0xFFF;
                                    #[allow(unsafe_code)]
                                    let actual = unsafe {
                                        core::ptr::read_volatile(
                                            paddr.as_ptr::<u8>().add(page_off) as *const u64
                                        )
                                    };
                                    // Read 8 bytes from the file at the same offset
                                    let file_page_off = offset + (pv.value() - vs);
                                    let mut expected = [0u8; 8];
                                    let _ = file.read(
                                        file_page_off + page_off,
                                        (&mut expected[..]).into(),
                                        &OpenOptions::readwrite(),
                                    );
                                    let expected_val = u64::from_ne_bytes(expected);
                                    if actual != expected_val {
                                        warn!("  PAGE MISMATCH at ip: paddr={:#x} actual={:#018x} file={:#018x}",
                                              paddr.value(), actual, expected_val);
                                    } else {
                                        warn!("  page content MATCHES file (paddr={:#x})", paddr.value());
                                    }
                                } else {
                                    warn!("  NO PTE for IP page {:#x}", ip & !0xFFF);
                                }
                            }
                        } else {
                            warn!("  IP VMA: [{:#x}-{:#x}] anonymous/device", vs, ve);
                        }
                        break;
                    }
                }
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
                        VmAreaType::DeviceMemory { phys_base } => {
                            warn!("  VMA[{}]: [{:#x}-{:#x}] prot={:#x} device phys={:#x}",
                                  i, vma.start().value(), vma.end().value(), vma.prot().bits(),
                                  phys_base);
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
            // Userspace should never dereference the kernel high half. If a
            // fault addr has KERNEL_BASE bits set, something leaked a kernel
            // direct-map pointer into userspace memory or registers. See
            // blog 186 for the shape of this bug.
            if (unaligned_vaddr.value() >> 47) == 0x1ffff {
                warn!(
                    "KERNEL_PTR_LEAK: pid={} fault_addr={:#x} ip={:#x} — kernel direct-map pointer dereferenced from userspace",
                    pid, unaligned_vaddr.value(), ip,
                );
            }
            // Dump registers for crash investigation.
            if pid > 2 {
                let cpu = kevlar_platform::arch::cpu_id() as usize;
                if let Some(r) = kevlar_platform::crash_regs::take(cpu) {
                    warn!("  RDI={:#x} RSI={:#x} RBP={:#x} RSP={:#x} RAX={:#x}",
                        r.rdi, r.rsi, r.rbp, r.rsp, r.rax);
                    // Flag any GPR that looks like a kernel direct-map pointer.
                    let regs = [("RDI", r.rdi), ("RSI", r.rsi), ("RBP", r.rbp),
                                ("RSP", r.rsp), ("RAX", r.rax)];
                    for (name, v) in regs {
                        if (v >> 47) == 0x1ffff {
                            warn!("  KERNEL_PTR_LEAK: {}={:#x} is a kernel direct-map pointer (paddr={:#x})",
                                  name, v, v & 0x0000_7fff_ffff_ffff);
                        }
                    }
                    // LEAK_PAGE_SCAN runs inside deliver_sigsegv_fatal()
                    // below, which runs after vm is dropped (avoiding a
                    // self-deadlock on vm_arc.lock_no_irq()).
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
        } // else (no stack growth)
        } // None
    };

    // EARLY EXIT: If the page is already present in the page table, this is a
    // permission fault (wrong PTE flags), NOT a demand fault (missing page).
    // Handle it directly without reading from the file — this preserves modified
    // MAP_PRIVATE page content (e.g., dynamic linker relocations in the GOT).
    // Without this, we'd allocate a fresh page, fill it with original file data
    // (overwriting relocations), and try_map would catch it — but the wasted I/O
    // is expensive and the fresh-page allocation can fail under memory pressure.
    if _reason.contains(PageFaultReason::PRESENT) {
        kevlar_platform::page_allocator::free_pages(paddr, 1);
        let prot_flags = vma.prot().bits();
        let vma_is_shared = vma.is_shared();
        let is_anonymous = matches!(vma.area_type(), VmAreaType::Anonymous);
        // vma is no longer used — vm can be reborrowed.

        // Split huge page before any 4KB-level PTE operations. If the faulting
        // address is inside a 2MB huge page, we must decompose it into 512 × 4KB
        // PTEs first so that CoW and update_page_flags work on the correct PTE.
        if vm.page_table().is_huge_mapped(aligned_vaddr).is_some() {
            vm.page_table_mut().split_huge_page(aligned_vaddr);
            vm.page_table().flush_tlb_local(aligned_vaddr);
        }

        if prot_flags == 0 {
            drop(vm); drop(vm_ref);
            warn!(
                "SIGSEGV: PROT_NONE+PRESENT fault at {:#x} (pid={}, ip={:#x})",
                unaligned_vaddr.value(), current.pid().as_i32(), ip
            );
            current.send_signal(SIGSEGV);
            return;
        }

        // CoW write fault: page is present read-only, VMA allows writes.
        if _reason.contains(PageFaultReason::CAUSED_BY_WRITE) && (prot_flags & 2 != 0) {
            if let Some(old_paddr) = vm.page_table().lookup_paddr(aligned_vaddr) {
                if vma_is_shared {
                    vm.page_table_mut().update_page_flags(aligned_vaddr, prot_flags);
                    vm.page_table().flush_tlb_local(aligned_vaddr);
                    return;
                }
                let is_ghost = vm.is_ghost_forked;
                let refcount = kevlar_platform::page_refcount::page_ref_count(old_paddr);
                if refcount > 1 || is_ghost {
                    if !is_ghost {
                        kevlar_platform::page_refcount::page_ref_inc(old_paddr);
                    }
                    let new_paddr = match alloc_page(AllocPageFlags::USER | AllocPageFlags::DIRTY_OK) {
                        Ok(p) => p,
                        Err(_) => {
                            if !is_ghost {
                                kevlar_platform::page_refcount::page_ref_dec(old_paddr);
                            }
                            drop(vm); drop(vm_ref);
                            warn!("pid={}: OOM during CoW dup at vaddr={}",
                                  current.pid().as_i32(), unaligned_vaddr);
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
                    // Determine whether we'll free old_paddr, but DEFER the
                    // actual free until after flush_tlb below. See task #25:
                    // between free_pages and the cross-CPU flush, a sibling
                    // thread on another CPU can still write to V via its
                    // stale TLB entry — landing the write on a paddr that
                    // the allocator has already handed to someone else.
                    let mut should_free_old = false;
                    if !is_ghost {
                        kevlar_platform::page_refcount::page_ref_dec(old_paddr);
                        if kevlar_platform::page_refcount::page_ref_dec(old_paddr) {
                            should_free_old = true;
                        }
                    }
                    vm.page_table_mut().map_user_page_with_prot(aligned_vaddr, new_paddr, prot_flags);
                    // CRITICAL FIX (task #25): must flush ALL CPUs' TLBs,
                    // not just the local one.  With PCID, context switches
                    // use bit-63 (no-invalidate) when the generation matches,
                    // so stale RO entries for this PCID survive on remote CPUs.
                    // After CoW frees old_paddr, the stale entry resolves to
                    // whatever physical page gets reallocated at that address —
                    // a totally different process's data.  Symptoms: GOT/vtable
                    // entries contain wrong function pointers → GP faults,
                    // NULL deref, HLT execution, string bytes in text segments.
                    // This was the root cause of the xfce4-session/xfwm4/iceauth
                    // crashes in blogs 175–177.
                    vm.page_table().flush_tlb(aligned_vaddr);
                    if should_free_old {
                        kevlar_platform::page_allocator::free_pages(old_paddr, 1);
                    }
                    return;
                }
                // Sole owner: just update flags to writable.
                vm.page_table_mut().update_page_flags(aligned_vaddr, prot_flags);
                vm.page_table().flush_tlb(aligned_vaddr);
                return;
            }
        }

        // Write to read-only MAP_PRIVATE file-backed page (RELR relocations).
        if _reason.contains(PageFaultReason::CAUSED_BY_WRITE) && (prot_flags & 2 == 0)
            && !vma_is_shared && !is_anonymous
        {
            if let Some(old_paddr) = vm.page_table().lookup_paddr(aligned_vaddr) {
                let new_paddr = match alloc_page(AllocPageFlags::USER | AllocPageFlags::DIRTY_OK) {
                    Ok(p) => p,
                    Err(_) => {
                        drop(vm); drop(vm_ref);
                        warn!("pid={}: OOM during file-backed CoW at vaddr={}",
                              current.pid().as_i32(), unaligned_vaddr);
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
                vm.page_table_mut().unmap_user_page(aligned_vaddr);
                vm.page_table_mut().map_user_page_with_prot(
                    aligned_vaddr, new_paddr, prot_flags | 2);
                // Same PCID TLB shootdown fix as above.
                vm.page_table().flush_tlb(aligned_vaddr);
                if kevlar_platform::page_refcount::page_ref_dec(old_paddr) {
                    kevlar_platform::page_allocator::free_pages(old_paddr, 1);
                }
                return;
            }
            // No existing page for write to read-only — genuine SIGSEGV.
            drop(vm); drop(vm_ref);
            deliver_sigsegv_fatal();
            return;
        }

        // Write to a page the VMA doesn't allow writing — SIGSEGV.
        if _reason.contains(PageFaultReason::CAUSED_BY_WRITE) && (prot_flags & 2 == 0) {
            drop(vm); drop(vm_ref);
            current.send_signal(SIGSEGV);
            return;
        }

        // Not a write fault — just update PTE flags to match VMA prot.
        // This needs a remote TLB flush too: the stale entry might be
        // PROT_NONE (from mprotect) while the new PTE is readable, and
        // a stale PROT_NONE read on another CPU would fault endlessly
        // instead of succeeding.
        vm.page_table_mut().update_page_flags(aligned_vaddr, prot_flags);
        vm.page_table().flush_tlb(aligned_vaddr);
        return;
    }

    // ---- DEMAND FAULT PATH: page not present, need to read from file/zero ----

    // Page cache state — set inside the File match arm, used after for refcount init.
    let mut cache_hit = false;
    let mut cache_shared = false; // true when paddr is a shared cached page
    let mut is_cacheable = false;
    let mut offset_in_file = 0;
    let mut cache_file_ptr: usize = 0; // Arc data ptr for cache key

    match vma.area_type() {
        VmAreaType::Anonymous => { /* Zero-filled by zero_page above. */ }
        VmAreaType::DeviceMemory { phys_base } => {
            // Device memory (e.g., framebuffer): map the physical device page
            // directly instead of allocating kernel memory and copying.
            // Free the pre-allocated page — we'll use the device's physical page.
            kevlar_platform::page_allocator::free_pages(paddr, 1);
            let offset_in_vma = aligned_vaddr.value() - vma.start().value();
            let device_paddr = PAddr::new(phys_base + offset_in_vma);

            // Use map_device_page to skip page_ref_init — device addresses
            // are PCI BARs, not managed by the page allocator.
            let prot = if vma.prot().bits() & 2 != 0 { 2 } else { 0 };
            vm.page_table_mut()
                .map_device_page(aligned_vaddr, device_paddr, prot);

            drop(vm);
            drop(vm_ref);
            return;
        }
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

            // Task #25: trace the demand-fault fill for pages that keep
            // showing up as all-zeros at crash time.  offset_in_file
            // around 0x1e000 is the hot one for xfce4-session.
            if (vma.prot().bits() & 4 != 0) && copy_len > 0 {
                if offset_in_file >= 0x1d000 && offset_in_file <= 0x1f000 {
                    warn!(
                        "FILL_TRACE: pid={} vaddr={:#x} file_off={:#x} copy_len={} \
                         page_off={} file_size={:#x} offset_in_vma={:#x} paddr={:#x}",
                        current.pid().as_i32(),
                        aligned_vaddr.value(),
                        offset_in_file,
                        copy_len,
                        offset_in_page,
                        file_size,
                        offset_in_file - *offset,
                        paddr.value(),
                    );
                }
            }

            // --- Page cache for immutable files (initramfs) ---
            let vma_readonly = vma.prot().bits() & 2 == 0;
            // Only cache FULL pages. Partial pages (copy_len < PAGE_SIZE) at
            // segment boundaries must NOT be cached: a different VMA covering
            // the same file page index may need the full 4096 bytes. If we
            // cache a partial page (e.g. rodata's last page with 0x2E0 bytes),
            // the huge page assembler reuses it for the data VMA's first page,
            // leaving function pointers / GOT entries as zero.
            is_cacheable = file.is_content_immutable()
                && offset_in_page == 0
                && copy_len == PAGE_SIZE
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
                    let read_result = file.read(
                        offset_in_file,
                        (&mut buf[offset_in_page..(offset_in_page + copy_len)]).into(),
                        &OpenOptions::readwrite(),
                    );
                    match read_result {
                        Ok(n) => {
                            if n < copy_len {
                                warn!(
                                    "DEMAND PAGE SHORT READ: pid={} vaddr={:#x} \
                                     file_off={:#x} expected={} got={} page_off={}",
                                    current.pid().as_i32(),
                                    aligned_vaddr.value(),
                                    offset_in_file,
                                    copy_len, n, offset_in_page,
                                );
                            }
                            // Task #25: after read, verify the first 8 bytes
                            // of the hot page to catch immediate corruption.
                            if offset_in_file >= 0x1d000 && offset_in_file <= 0x1f000
                                && (vma.prot().bits() & 4 != 0)
                            {
                                let actual = page_as_slice_mut(paddr);
                                let b0 = actual[offset_in_page];
                                let b1 = actual[offset_in_page + 1];
                                warn!(
                                    "FILL_VERIFY: pid={} vaddr={:#x} file_off={:#x} \
                                     read_n={} first_bytes=[{:#04x},{:#04x}] paddr={:#x}",
                                    current.pid().as_i32(),
                                    aligned_vaddr.value(),
                                    offset_in_file,
                                    n, b0, b1, paddr.value(),
                                );
                            }
                            // SMP page corruption detector (task #25).
                            //
                            // For executable pages, re-read the ENTIRE
                            // page from the file and diff it against the
                            // freshly-mapped physical frame.  The
                            // previous version only checked the first 8
                            // bytes of the page; the xfce4-session HLT
                            // corruption we tracked down was at page
                            // offset 0xcf3, so the 8-byte check missed
                            // it entirely.
                            //
                            // This is diagnostic-only — expensive (4 KB
                            // re-read per executable page-fault), and
                            // prints the first divergent byte so we can
                            // see what the corrupted byte should have
                            // been vs what it actually is.
                            if vma.prot().bits() & 4 != 0 && copy_len >= 8 {
                                let mut expected = [0u8; PAGE_SIZE];
                                let re_read = file.read(
                                    offset_in_file,
                                    (&mut expected[..copy_len]).into(),
                                    &OpenOptions::readwrite(),
                                );
                                if re_read.is_ok() {
                                    let actual = page_as_slice_mut(paddr);
                                    let mut first_bad: Option<usize> = None;
                                    for i in 0..copy_len {
                                        let want = expected[i];
                                        let got = actual[offset_in_page + i];
                                        if want != got {
                                            first_bad = Some(i);
                                            break;
                                        }
                                    }
                                    if let Some(off) = first_bad {
                                        warn!(
                                            "PAGE CORRUPTION: pid={} vaddr={:#x} \
                                             page_off={:#x} file_off={:#x} \
                                             expected={:#04x} got={:#04x} (first of {} bytes)",
                                            current.pid().as_i32(),
                                            aligned_vaddr.value() + off,
                                            off, offset_in_file + off,
                                            expected[off],
                                            actual[offset_in_page + off],
                                            copy_len - off,
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            panic!(
                                "failed to read file: {:?} (vaddr={:#x} off={:#x} len={})",
                                e, aligned_vaddr.value(), offset_in_file, copy_len
                            );
                        }
                    }
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
            VmAreaType::DeviceMemory { .. } => "device",
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
                    let ok = vm.page_table_mut().update_page_flags(aligned_vaddr, prot_flags);
                    if !ok {
                        warn!("COW refcount=1 FAILED to update flags for vaddr={:#x}", aligned_vaddr.value());
                    }
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
                    //
                    // Pin the source page by incrementing its refcount BEFORE
                    // copying. Without this, another CPU doing a concurrent COW
                    // on the same page could decrement the refcount to 0 and
                    // free it while we're still reading from it.
                    if !is_ghost {
                        kevlar_platform::page_refcount::page_ref_inc(old_paddr);
                    }
                    let new_paddr = match alloc_page(AllocPageFlags::USER | AllocPageFlags::DIRTY_OK) {
                        Ok(p) => p,
                        Err(_) => {
                            if !is_ghost {
                                kevlar_platform::page_refcount::page_ref_dec(old_paddr);
                            }
                            warn!("pid={}: OOM during CoW fault at vaddr={}",
                                  current.pid().as_i32(), unaligned_vaddr);
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
                    // Same defer-free-until-after-flush pattern as the
                    // CoW-write-to-shared-RO site above (task #25): a
                    // sibling thread on another CPU may still have
                    // V→old_paddr cached, so the page must not be
                    // recycled until the cross-CPU flush completes.
                    let mut should_free_old = false;
                    if !is_ghost {
                        // Decrement twice: once for our pin, once for the PTE removal.
                        kevlar_platform::page_refcount::page_ref_dec(old_paddr);
                        if kevlar_platform::page_refcount::page_ref_dec(old_paddr) {
                            should_free_old = true;
                        }
                    }
                    // Ghost fork: DON'T decrement — refcount was never incremented.
                    vm.page_table_mut().map_user_page_with_prot(aligned_vaddr, new_paddr, prot_flags);
                    // Cross-CPU flush: when we're the sole owner across
                    // processes we may still be multi-threaded, and sibling
                    // threads on other CPUs have PCID-tagged entries pointing
                    // to old_paddr.  Local-only flush leaves them live.
                    if should_free_old {
                        vm.page_table().flush_tlb(aligned_vaddr);
                        kevlar_platform::page_allocator::free_pages(old_paddr, 1);
                    } else {
                        vm.page_table().flush_tlb_local(aligned_vaddr);
                    }
                    return;
                }
                // refcount == 1 and not ghost: sole owner, just make writable.
                vm.page_table_mut().update_page_flags(aligned_vaddr, prot_flags);
                vm.page_table().flush_tlb_local(aligned_vaddr);
                return;
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
                            warn!("pid={}: OOM during RELR-write CoW at vaddr={}",
                                  current.pid().as_i32(), unaligned_vaddr);
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
                    // Task #25: use cross-CPU flush when we're about to free
                    // old_paddr (sibling threads on other CPUs may still have
                    // V→old_paddr cached and could write through a stale TLB
                    // entry to a page that's been reissued by the allocator).
                    let should_free_old =
                        kevlar_platform::page_refcount::page_ref_dec(old_paddr);
                    if should_free_old {
                        vm.page_table().flush_tlb(aligned_vaddr);
                        kevlar_platform::page_allocator::free_pages(old_paddr, 1);
                    } else {
                        vm.page_table().flush_tlb_local(aligned_vaddr);
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
