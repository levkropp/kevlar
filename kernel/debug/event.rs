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
        }
        w.write_char('\n')
    }
}
