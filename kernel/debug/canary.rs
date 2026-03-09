// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Stack canary monitoring for userspace processes.
//!
//! On x86_64, the TLS canary lives at `fsbase + 0x28` (musl/glibc convention).
//! The kernel places AT_RANDOM on the user stack during execve; the C runtime
//! reads it and stores the canary value at the TLS offset.
//!
//! This module reads the canary before and after each syscall to detect
//! stack buffer overflows in userspace. Corruption is reported as a structured
//! debug event that an LLM or MCP tool can analyze.

use super::emit;
use super::event::DebugEvent;
use super::filter::DebugFilter;
use kevlar_platform::address::UserVAddr;

/// The TLS offset where musl/glibc stores the stack canary on x86_64.
#[cfg(target_arch = "x86_64")]
const CANARY_TLS_OFFSET: usize = 0x28;

/// Read the current canary value from the process's TLS area.
///
/// Returns `(fsbase, canary_value)` or `None` if fsbase is not set.
pub fn read_canary(fsbase: usize) -> Option<(usize, u64)> {
    if fsbase == 0 {
        return None;
    }

    #[cfg(target_arch = "x86_64")]
    {
        let canary_addr = match UserVAddr::new_nonnull(fsbase + CANARY_TLS_OFFSET) {
            Ok(addr) => addr,
            Err(_) => return None,
        };
        let canary = canary_addr.read::<u64>().unwrap_or(0xDEAD_DEAD_DEAD_DEAD);
        Some((fsbase, canary))
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = fsbase;
        None // ARM64 canary tracking not yet implemented
    }
}

/// Check canary and emit a debug event if corrupted.
///
/// `previous_canary` is the canary value recorded at syscall entry (or after execve).
/// If `None`, this is the first check and we just record without comparing.
///
/// Returns the current canary value for the caller to store.
pub fn check_and_emit(
    pid: i32,
    fsbase: usize,
    previous_canary: Option<u64>,
    when: &str,
    syscall_name: &str,
) -> Option<u64> {
    let (fsbase_val, current) = match read_canary(fsbase) {
        Some(v) => v,
        None => return None,
    };

    let corrupted = match previous_canary {
        Some(prev) => prev != current && prev != 0,
        None => false,
    };

    // Always emit if corrupted; otherwise only emit if both CANARY and SYSCALL are enabled.
    if corrupted || emit::is_enabled(DebugFilter::CANARY | DebugFilter::SYSCALL) {
        let event = DebugEvent::CanaryCheck {
            pid,
            fsbase: fsbase_val,
            expected: previous_canary.unwrap_or(0),
            found: current,
            corrupted,
            when,
            syscall_name,
        };

        let category = if corrupted {
            DebugFilter::CANARY
        } else {
            // Only emit non-corruption canary checks when both flags are set
            DebugFilter::CANARY
        };
        emit::emit(category, &event);
    }

    // If corrupted, dump the assembly-level usercopy trace ring buffer.
    // This shows the ACTUAL register values (dst, src, len, return_addr) for
    // the last 32 copy_to_user/copy_from_user calls — the len field reveals
    // which copy used the wrong length.
    if corrupted {
        debug_warn!(
            "CANARY CORRUPTED pid={} fsbase={:#x} expected={:#x} found={:#x} when={} syscall={}",
            pid, fsbase_val, previous_canary.unwrap_or(0), current, when, syscall_name
        );
        super::dump_usercopy_trace(pid, "canary_corruption");
    }

    Some(current)
}
