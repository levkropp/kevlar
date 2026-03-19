// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Structured kernel debug event system.
//!
//! # Overview
//!
//! This module provides a structured, zero-allocation debug event system
//! designed for POSIX kernel debugging with LLMs and MCP tools as first-class
//! consumers. All events are emitted as JSONL (one JSON object per line) to
//! the kernel's serial output, prefixed with `DBG ` for easy filtering.
//!
//! # Event Categories
//!
//! Events are grouped into categories that can be independently enabled via
//! the kernel command line (`debug=syscall,signal,fault,...`) or at runtime:
//!
//! - `syscall` — Entry/exit for every syscall with args and return values
//! - `signal` — Signal delivery with action and handler address
//! - `fault` — Page faults and CPU exceptions with VMA context
//! - `process` — Fork, exec, exit lifecycle events
//! - `canary` — Stack canary corruption detection
//! - `memory` — Memory allocation events (mmap, brk)
//! - `panic` — Kernel panic with structured backtrace
//!
//! # Output Format
//!
//! ```text
//! DBG {"type":"syscall_entry","pid":1,"name":"write","nr":1,"args":[0x1,0x4000a0,0xd,0x0,0x0,0x0]}
//! DBG {"type":"syscall_exit","pid":1,"name":"write","nr":1,"result":13}
//! DBG {"type":"canary_check","pid":1,"fsbase":0x7f000000,"expected":0xdeadbeef,"found":0x41414141,"corrupted":true,"when":"post_syscall","syscall":"read"}
//! ```
//!
//! # Usage from MCP/LLM
//!
//! The host-side MCP debug server (`tools/mcp-debug-server/`) consumes these
//! events from the QEMU serial output and exposes them as MCP tools. An LLM
//! can query syscall traces, detect canary corruptions, and analyze faults
//! without needing to understand raw kernel internals.

pub mod canary;
pub mod emit;
pub mod event;
pub mod filter;
pub mod htrace;
pub mod profiler;
pub mod tracer;
pub mod usercopy;

// Re-export commonly used items.
pub use emit::{emit, get_filter, is_enabled, parse_cmdline_filter, set_filter};
pub use event::DebugEvent;
pub use filter::DebugFilter;

use crate::process::signal::Signal;

/// Initialize the debug system from a kernel command-line `debug=...` value.
///
/// Called from `boot_kernel()` after parsing the command line.
pub fn init(debug_cmdline: Option<&str>) {
    let filter = match debug_cmdline {
        Some(value) => parse_cmdline_filter(value),
        None => {
            // In debug builds, default to panic + canary monitoring.
            if cfg!(debug_assertions) {
                DebugFilter::PANIC | DebugFilter::CANARY | DebugFilter::FAULT
            } else {
                DebugFilter::empty()
            }
        }
    };

    set_filter(filter);

    // Enable the assembly-level usercopy trace ring buffer when canary or
    // usercopy debugging is active. This captures the actual register values
    // (dst, src, len, return_addr) at every copy_to_user / copy_from_user
    // call, written directly by the assembly probe.
    if filter.intersects(DebugFilter::CANARY | DebugFilter::USERCOPY | DebugFilter::FAULT) {
        kevlar_platform::usercopy_trace::enable();
    }

    // Enable per-syscall cycle profiler when profile flag is set.
    if filter.contains(DebugFilter::PROFILE) {
        profiler::enable();
    }

    // Enable span tracer for exec/fork/page-fault phase profiling.
    if filter.contains(DebugFilter::TRACE) {
        tracer::enable();
    }

    // Enable hierarchical call tracer for debugging call chains.
    if filter.contains(DebugFilter::HTRACE) {
        htrace::enable();
    }

    if !filter.is_empty() {
        info!("debug: enabled categories: {:?}", filter);
    }
}

/// Emit a UsercopyFault event when a page fault occurs during a usercopy
/// operation. Called from the page fault handler when the faulting IP
/// is in the usercopy assembly region.
///
/// `ip` is the faulting instruction pointer (one of the usercopy labels).
/// `fault_addr` is the address that was being accessed (CR2 on x86_64).
#[allow(dead_code)]
pub fn emit_usercopy_fault(pid: i32, fault_addr: usize, ip: usize) {
    // Determine which usercopy phase we're in based on the IP.
    // On x86_64, the assembly labels are:
    //   usercopy1  = leading bytes alignment (rep movsb)
    //   usercopy1b = bulk qword copy (rep movsq)
    //   usercopy1c = trailing bytes (rep movsb)
    //   usercopy2  = strncpy_from_user
    //   usercopy3  = memset_user
    #[cfg(target_arch = "x86_64")]
    let label = {
        #[allow(unsafe_code)]
        #[allow(dead_code)]
        unsafe extern "C" {
            fn usercopy1();
            fn usercopy1b();
            fn usercopy1c();
            fn usercopy1d();
            fn usercopy2();
            fn usercopy3();
        }
        let ip_val = ip as u64;
        if ip_val == usercopy1 as *const () as u64 {
            "leading_bytes"
        } else if ip_val == usercopy1b as *const () as u64 {
            "bulk_qwords"
        } else if ip_val == usercopy1c as *const () as u64 {
            "trailing_bytes"
        } else if ip_val == usercopy1d as *const () as u64 {
            "small_copy"
        } else if ip_val == usercopy2 as *const () as u64 {
            "strncpy"
        } else if ip_val == usercopy3 as *const () as u64 {
            "memset"
        } else {
            "unknown"
        }
    };

    #[cfg(not(target_arch = "x86_64"))]
    let label = "unknown";

    let ctx = usercopy::get_context();

    emit(DebugFilter::FAULT, &DebugEvent::UsercopyFault {
        pid,
        fault_addr,
        ip,
        usercopy_label: label,
        // We don't have register access here (they're in the InterruptFrame
        // which is in the platform crate). Log what we can.
        dst_ptr: 0,
        src_ptr: 0,
        remaining: 0,
        original_len: 0,
        context: ctx,
    });

    // Dump the assembly-level trace buffer — shows the last 32 copies with
    // actual register values. The most recent entry is the faulting copy.
    dump_usercopy_trace(pid, "usercopy_fault");
}

/// Dump the assembly-level usercopy trace ring buffer as a debug event.
///
/// Called automatically on canary corruption and usercopy faults. Can also
/// be called manually from GDB or via the MCP server.
///
/// The trace buffer is written by the assembly `copy_to_user` / `copy_from_user`
/// probe and contains the **actual CPU register values** at function entry:
/// - `dst` = rdi (destination pointer)
/// - `src` = rsi (source pointer)
/// - `len` = rdx (byte count — THIS is what we're hunting for)
/// - `ret_addr` = return address (identifies the Rust caller)
pub fn dump_usercopy_trace(pid: i32, trigger: &str) {
    use kevlar_platform::usercopy_trace;

    if !usercopy_trace::is_enabled() {
        return;
    }

    let snapshot = usercopy_trace::snapshot();
    let total_calls = usercopy_trace::call_count();

    // Convert to the tuple format the event expects.
    // Use a stack-allocated buffer to avoid heap allocation.
    let mut tuples = [(0usize, 0usize, 0usize, 0usize); 32];
    let count = snapshot.len();
    for (i, entry) in snapshot.iter().enumerate() {
        tuples[i] = (entry.dst, entry.src, entry.len, entry.ret_addr);
    }

    emit(DebugFilter::CANARY | DebugFilter::FAULT, &DebugEvent::UsercopyTraceDump {
        pid,
        trigger,
        total_calls,
        entries: &tuples[..count],
    });
}

/// Map a signal number to its name. Used by debug event emitters.
pub fn signal_name(sig: Signal) -> &'static str {
    match sig {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        10 => "SIGUSR1",
        11 => "SIGSEGV",
        12 => "SIGUSR2",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        16 => "SIGSTKFLT",
        17 => "SIGCHLD",
        18 => "SIGCONT",
        19 => "SIGSTOP",
        20 => "SIGTSTP",
        21 => "SIGTTIN",
        22 => "SIGTTOU",
        _ => "SIG?",
    }
}
