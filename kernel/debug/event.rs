// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Structured debug event types and zero-allocation JSONL serialization.
//!
//! Each event serializes to a single line: `DBG {"type":"...","pid":...,...}\n`
//! This format is simultaneously:
//! - Greppable in raw serial logs (`grep ^DBG`)
//! - Machine-parseable (JSONL after stripping the 4-byte prefix)
//! - LLM-friendly (self-describing, structured, compact)

use core::fmt::{self, Write};

/// Backtrace frame captured at event time.
#[derive(Clone, Copy)]
pub struct BtFrame {
    pub addr: usize,
    pub symbol: &'static str,
    pub offset: usize,
}

/// All debug event types emitted by the kernel.
///
/// Each variant maps 1:1 to a JSON `"type"` string. Fields are kept minimal
/// to avoid allocation — only scalars, static strings, and small fixed arrays.
pub enum DebugEvent<'a> {
    SyscallEntry {
        pid: i32,
        name: &'a str,
        number: usize,
        args: [usize; 6],
    },
    SyscallExit {
        pid: i32,
        name: &'a str,
        number: usize,
        result: isize,
        errno: Option<&'a str>,
    },
    CanaryCheck {
        pid: i32,
        fsbase: usize,
        expected: u64,
        found: u64,
        corrupted: bool,
        when: &'a str, // "pre_syscall" | "post_syscall"
        syscall_name: &'a str,
    },
    PageFault {
        pid: i32,
        vaddr: usize,
        ip: usize,
        reason: &'a str,
        resolved: bool,
        vma_start: Option<usize>,
        vma_end: Option<usize>,
        vma_type: Option<&'a str>,
    },
    Signal {
        pid: i32,
        signal: i32,
        signal_name: &'a str,
        action: &'a str,
        handler_addr: Option<usize>,
        ip: usize,
    },
    UserFault {
        pid: i32,
        exception: &'a str,
        ip: usize,
        signal_delivered: i32,
    },
    ProcessExit {
        pid: i32,
        status: i32,
        by_signal: bool,
    },
    ProcessExec {
        pid: i32,
        argv0: &'a str,
        entry: usize,
    },
    ProcessFork {
        parent_pid: i32,
        child_pid: i32,
    },
    Panic {
        message: &'a str,
        backtrace: &'a [BtFrame],
    },
    UnimplementedSyscall {
        pid: i32,
        name: &'a str,
        number: usize,
    },
    /// Emitted for each copy_to_user / copy_from_user call.
    /// High volume — only enabled with `USERCOPY` filter.
    #[allow(dead_code)]
    Usercopy {
        pid: i32,
        direction: &'a str,     // "to_user" | "from_user" | "fill" | "read_cstr"
        user_addr: usize,       // destination (to_user) or source (from_user)
        len: usize,
        context: &'a str,       // what operation set this (e.g. "ioctl:TCGETS", "signal_stack")
    },
    /// Enhanced page fault with register state — emitted when a fault occurs
    /// during a usercopy operation (IP in usercopy region).
    #[allow(dead_code)]
    UsercopyFault {
        pid: i32,
        fault_addr: usize,      // CR2 / FAR_EL1
        ip: usize,              // faulting instruction
        usercopy_label: &'a str, // "leading_bytes" | "bulk_qwords" | "trailing_bytes" | "strncpy" | "memset"
        /// Register context at fault:
        dst_ptr: usize,         // RDI (destination pointer)
        src_ptr: usize,         // RSI (source pointer)
        remaining: usize,       // RCX (remaining count)
        original_len: usize,    // RDX (original length, may be clobbered)
        context: &'a str,       // usercopy context tag
    },
    /// Detailed signal stack setup trace — each individual write.
    SignalStackWrite {
        pid: i32,
        signal: i32,
        write_what: &'a str,    // "trampoline" | "return_addr" | "siginfo" | etc.
        user_addr: usize,
        len: usize,
        user_rsp_before: usize,
        user_rsp_after: usize,
    },
    /// Assembly-level usercopy trace dump — actual register values at
    /// copy_to_user / copy_from_user entry. Emitted when canary corruption
    /// or usercopy fault is detected, showing the last N copies.
    UsercopyTraceDump {
        pid: i32,
        trigger: &'a str,       // "canary_corruption" | "usercopy_fault" | "manual"
        total_calls: u64,       // total calls since trace enabled
        entries: &'a [(usize, usize, usize, usize)], // (dst, src, len, ret_addr)
    },
    /// Exec prefault page source trace (kwab-exec).
    /// Each entry: (page_index_in_2MB_region, source_tag, file_page_index).
    /// source_tag: 0=cache_hit, 1=file_read, 2=zero/unmapped.
    ExecPrefault {
        pid: i32,
        vaddr: usize,
        cached: u16,
        file_read: u16,
        zero: u16,
        total: u16,
    },
    /// Post-exec page content verification failure (kwab-verify).
    PageVerifyFail {
        pid: i32,
        vaddr: usize,
        first_diff: u16,
        expected_byte: u8,
        actual_byte: u8,
        file_offset: usize,
        vma_start: usize,
    },
    /// Huge page assembly sub-page content mismatch (kwab-verify).
    HugePageVerifyFail {
        pid: i32,
        huge_base: usize,
        sub_page: u16,
        first_diff: u16,
        expected_byte: u8,
        actual_byte: u8,
        vma_start: usize,
        vma_type: &'a str,  // "file_ro" | "file_rw" | "anon" | "none"
    },
    /// Post-exec page verification summary (kwab-verify).
    PageVerifyOk {
        pid: i32,
        verified: u32,
        failed: u32,
    },
    /// VM audit result (kwab-audit).
    /// Each entry: (vma_start, vma_end, prot_bits, mapped_pages, unmapped_pages, perm_mismatches).
    VmAudit {
        pid: i32,
        entries: &'a [(usize, usize, u8, u16, u16, u16)],
    },
    /// Comprehensive crash report emitted when a process is killed by a signal.
    /// Includes per-process syscall trace, VMA map, and register state.
    CrashReport {
        pid: i32,
        signal: i32,
        signal_name: &'a str,
        cmdline: &'a str,
        fault_addr: usize,
        ip: usize,
        fsbase: usize,
        rax: u64, rbx: u64, rcx: u64, rdx: u64,
        rsi: u64, rdi: u64, rbp: u64, rsp: u64,
        r8: u64, r9: u64, r10: u64, r11: u64,
        r12: u64, r13: u64, r14: u64, r15: u64,
        rflags: u64,
        /// Recent syscalls: (nr, result, arg0, arg1).
        syscalls: &'a [(u16, i32, u32, u32)],
        /// VM areas: (start, end, type_str).
        vmas: &'a [(usize, usize, &'a str)],
    },
}

impl<'a> DebugEvent<'a> {
    /// Write this event as a `DBG {...}\n` line to the given writer.
    ///
    /// This is zero-allocation: everything is formatted inline via `Write`.
    /// The output is valid JSONL (one JSON object per line) after stripping
    /// the `DBG ` prefix.
    pub fn write_jsonl<W: Write>(&self, w: &mut W) -> fmt::Result {
        w.write_str("DBG ")?;
        match self {
            DebugEvent::SyscallEntry { pid, name, number, args } => {
                write!(w,
                    r#"{{"type":"syscall_entry","pid":{},"name":"{}","nr":{},"args":[{:#x},{:#x},{:#x},{:#x},{:#x},{:#x}]}}"#,
                    pid, name, number, args[0], args[1], args[2], args[3], args[4], args[5]
                )?;
            }
            DebugEvent::SyscallExit { pid, name, number, result, errno } => {
                write!(w,
                    r#"{{"type":"syscall_exit","pid":{},"name":"{}","nr":{},"result":{}"#,
                    pid, name, number, result
                )?;
                if let Some(e) = errno {
                    write!(w, r#","errno":"{}""#, e)?;
                }
                w.write_str("}")?;
            }
            DebugEvent::CanaryCheck { pid, fsbase, expected, found, corrupted, when, syscall_name } => {
                write!(w,
                    r#"{{"type":"canary_check","pid":{},"fsbase":{:#x},"expected":{:#x},"found":{:#x},"corrupted":{},"when":"{}","syscall":"{}"}}"#,
                    pid, fsbase, expected, found, corrupted, when, syscall_name
                )?;
            }
            DebugEvent::PageFault { pid, vaddr, ip, reason, resolved, vma_start, vma_end, vma_type } => {
                write!(w,
                    r#"{{"type":"page_fault","pid":{},"vaddr":{:#x},"ip":{:#x},"reason":"{}","resolved":{}"#,
                    pid, vaddr, ip, reason, resolved
                )?;
                if let (Some(start), Some(end)) = (vma_start, vma_end) {
                    write!(w, r#","vma_start":{:#x},"vma_end":{:#x}"#, start, end)?;
                }
                if let Some(vt) = vma_type {
                    write!(w, r#","vma_type":"{}""#, vt)?;
                }
                w.write_str("}")?;
            }
            DebugEvent::Signal { pid, signal, signal_name, action, handler_addr, ip } => {
                write!(w,
                    r#"{{"type":"signal","pid":{},"signal":{},"signal_name":"{}","action":"{}","ip":{:#x}"#,
                    pid, signal, signal_name, action, ip
                )?;
                if let Some(h) = handler_addr {
                    write!(w, r#","handler":{:#x}"#, h)?;
                }
                w.write_str("}")?;
            }
            DebugEvent::UserFault { pid, exception, ip, signal_delivered } => {
                write!(w,
                    r#"{{"type":"user_fault","pid":{},"exception":"{}","ip":{:#x},"signal_delivered":{}}}"#,
                    pid, exception, ip, signal_delivered
                )?;
            }
            DebugEvent::ProcessExit { pid, status, by_signal } => {
                write!(w,
                    r#"{{"type":"process_exit","pid":{},"status":{},"by_signal":{}}}"#,
                    pid, status, by_signal
                )?;
            }
            DebugEvent::ProcessExec { pid, argv0, entry } => {
                write!(w,
                    r#"{{"type":"process_exec","pid":{},"argv0":"{}","entry":{:#x}}}"#,
                    pid, argv0, entry
                )?;
            }
            DebugEvent::ProcessFork { parent_pid, child_pid } => {
                write!(w,
                    r#"{{"type":"process_fork","parent_pid":{},"child_pid":{}}}"#,
                    parent_pid, child_pid
                )?;
            }
            DebugEvent::Panic { message, backtrace } => {
                // Escape the panic message for JSON (replace " and \ and newlines).
                w.write_str(r#"{"type":"panic","message":""#)?;
                for ch in message.chars() {
                    match ch {
                        '"' => w.write_str("\\\"")?,
                        '\\' => w.write_str("\\\\")?,
                        '\n' => w.write_str("\\n")?,
                        '\r' => w.write_str("\\r")?,
                        c => w.write_char(c)?,
                    }
                }
                w.write_str(r#"","backtrace":["#)?;
                for (i, frame) in backtrace.iter().enumerate() {
                    if i > 0 {
                        w.write_char(',')?;
                    }
                    write!(w,
                        r#"{{"addr":{:#x},"sym":"{}","off":{:#x}}}"#,
                        frame.addr, frame.symbol, frame.offset
                    )?;
                }
                w.write_str("]}")?;
            }
            DebugEvent::UnimplementedSyscall { pid, name, number } => {
                write!(w,
                    r#"{{"type":"unimplemented_syscall","pid":{},"name":"{}","nr":{}}}"#,
                    pid, name, number
                )?;
            }
            DebugEvent::Usercopy { pid, direction, user_addr, len, context } => {
                write!(w,
                    r#"{{"type":"usercopy","pid":{},"dir":"{}","addr":{:#x},"len":{},"ctx":"{}"}}"#,
                    pid, direction, user_addr, len, context
                )?;
            }
            DebugEvent::UsercopyFault { pid, fault_addr, ip, usercopy_label, dst_ptr, src_ptr, remaining, original_len, context } => {
                write!(w,
                    r#"{{"type":"usercopy_fault","pid":{},"fault_addr":{:#x},"ip":{:#x},"label":"{}","dst":{:#x},"src":{:#x},"remaining":{},"orig_len":{},"ctx":"{}"}}"#,
                    pid, fault_addr, ip, usercopy_label, dst_ptr, src_ptr, remaining, original_len, context
                )?;
            }
            DebugEvent::SignalStackWrite { pid, signal, write_what, user_addr, len, user_rsp_before, user_rsp_after } => {
                write!(w,
                    r#"{{"type":"signal_stack_write","pid":{},"signal":{},"what":"{}","addr":{:#x},"len":{},"rsp_before":{:#x},"rsp_after":{:#x}}}"#,
                    pid, signal, write_what, user_addr, len, user_rsp_before, user_rsp_after
                )?;
            }
            DebugEvent::UsercopyTraceDump { pid, trigger, total_calls, entries } => {
                write!(w,
                    r#"{{"type":"ucopy_trace_dump","pid":{},"trigger":"{}","total_calls":{},"entries":["#,
                    pid, trigger, total_calls
                )?;
                for (i, (dst, src, len, ret_addr)) in entries.iter().enumerate() {
                    if i > 0 {
                        w.write_char(',')?;
                    }
                    write!(w,
                        r#"{{"dst":{:#x},"src":{:#x},"len":{},"ret":{:#x}}}"#,
                        dst, src, len, ret_addr
                    )?;
                }
                w.write_str("]}")?;
            }
            DebugEvent::ExecPrefault { pid, vaddr, cached, file_read, zero, total } => {
                write!(w,
                    r#"{{"type":"exec_prefault","pid":{},"vaddr":{:#x},"cached":{},"file_read":{},"zero":{},"total":{}}}"#,
                    pid, vaddr, cached, file_read, zero, total
                )?;
            }
            DebugEvent::PageVerifyFail { pid, vaddr, first_diff, expected_byte, actual_byte, file_offset, vma_start } => {
                write!(w,
                    r#"{{"type":"page_verify_fail","pid":{},"vaddr":{:#x},"first_diff":{},"expected":{:#x},"actual":{:#x},"file_offset":{:#x},"vma_start":{:#x}}}"#,
                    pid, vaddr, first_diff, expected_byte, actual_byte, file_offset, vma_start
                )?;
            }
            DebugEvent::HugePageVerifyFail { pid, huge_base, sub_page, first_diff, expected_byte, actual_byte, vma_start, vma_type } => {
                write!(w,
                    r#"{{"type":"huge_page_verify_fail","pid":{},"huge_base":{:#x},"sub_page":{},"first_diff":{},"expected":{:#x},"actual":{:#x},"vma_start":{:#x},"vma_type":"{}"}}"#,
                    pid, huge_base, sub_page, first_diff, expected_byte, actual_byte, vma_start, vma_type
                )?;
            }
            DebugEvent::PageVerifyOk { pid, verified, failed } => {
                write!(w,
                    r#"{{"type":"page_verify_ok","pid":{},"verified":{},"failed":{}}}"#,
                    pid, verified, failed
                )?;
            }
            DebugEvent::VmAudit { pid, entries } => {
                write!(w, r#"{{"type":"vm_audit","pid":{},"vmas":["#, pid)?;
                for (i, (start, end, prot, mapped, unmapped, mismatches)) in entries.iter().enumerate() {
                    if i > 0 { w.write_char(',')?; }
                    write!(w,
                        r#"{{"start":{:#x},"end":{:#x},"prot":{},"mapped":{},"unmapped":{},"perm_mismatch":{}}}"#,
                        start, end, prot, mapped, unmapped, mismatches
                    )?;
                }
                w.write_str("]}")?;
            }
            DebugEvent::CrashReport {
                pid, signal, signal_name, cmdline, fault_addr, ip, fsbase,
                rax, rbx, rcx, rdx, rsi, rdi, rbp, rsp,
                r8, r9, r10, r11, r12, r13, r14, r15, rflags,
                syscalls, vmas,
            } => {
                write!(w,
                    r#"{{"type":"crash_report","pid":{},"signal":{},"signal_name":"{}","cmdline":""#,
                    pid, signal, signal_name
                )?;
                // Escape cmdline for JSON.
                for ch in cmdline.chars() {
                    match ch {
                        '"' => w.write_str("\\\"")?,
                        '\\' => w.write_str("\\\\")?,
                        '\n' => w.write_str("\\n")?,
                        '\r' => w.write_str("\\r")?,
                        c => w.write_char(c)?,
                    }
                }
                write!(w,
                    r#"","fault_addr":{:#x},"ip":{:#x},"fsbase":{:#x},"regs":{{"#,
                    fault_addr, ip, fsbase
                )?;
                write!(w,
                    r#""rax":{:#x},"rbx":{:#x},"rcx":{:#x},"rdx":{:#x},"rsi":{:#x},"rdi":{:#x},"rbp":{:#x},"rsp":{:#x}"#,
                    rax, rbx, rcx, rdx, rsi, rdi, rbp, rsp
                )?;
                write!(w,
                    r#","r8":{:#x},"r9":{:#x},"r10":{:#x},"r11":{:#x},"r12":{:#x},"r13":{:#x},"r14":{:#x},"r15":{:#x},"rflags":{:#x}}}"#,
                    r8, r9, r10, r11, r12, r13, r14, r15, rflags
                )?;
                // Syscall trace array.
                w.write_str(r#","syscalls":["#)?;
                for (i, (nr, result, a0, a1)) in syscalls.iter().enumerate() {
                    if i > 0 {
                        w.write_char(',')?;
                    }
                    let name = crate::syscalls::syscall_name_by_number(*nr as usize);
                    write!(w,
                        r#"{{"nr":{},"name":"{}","result":{},"a0":{:#x},"a1":{:#x}}}"#,
                        nr, name, result, a0, a1
                    )?;
                }
                // VMA array.
                w.write_str(r#"],"vmas":["#)?;
                for (i, (start, end, type_str)) in vmas.iter().enumerate() {
                    if i > 0 {
                        w.write_char(',')?;
                    }
                    write!(w,
                        r#"{{"start":{:#x},"end":{:#x},"type":"{}"}}"#,
                        start, end, type_str
                    )?;
                }
                w.write_str("]}")?;
            }
        }
        w.write_char('\n')
    }
}
