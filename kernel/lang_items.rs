// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Panic handler and crash dump — requires unsafe for static mut and raw pointers.
#![allow(unsafe_code)]
use core::sync::atomic::AtomicBool;

pub static PANICKED: AtomicBool = AtomicBool::new(false);
static mut KERNEL_DUMP_BUF: KernelDump = KernelDump::empty();

#[repr(C, packed)]
struct KernelDump {
    /// `0xdeadbeee`
    magic: u32,
    /// The length of the kernel log.
    len: u32,
    /// The kernel log (including the panic message).
    log: [u8; 4096],
}

impl KernelDump {
    const fn empty() -> KernelDump {
        KernelDump {
            magic: 0,
            len: 0,
            log: [0; 4096],
        }
    }
}

#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    panic!("alloc error: layout={:?}", layout);
}

/// This function is called on panic.
#[panic_handler]
#[cfg(not(test))]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Disable interrupts immediately.  The panic handler formats PanicInfo
    // (which holds references into the faulting frame's stack) using code
    // that may span many instructions.  If a hardware IRQ fires in between
    // — particularly a second #GP triggered by a bad interrupt stack — the
    // CPU will re-enter x64_handle_interrupt and panic again, producing a
    // spurious "double panic" before the first one is even logged.
    unsafe {
        #[cfg(target_arch = "x86_64")]
        core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
        #[cfg(target_arch = "aarch64")]
        core::arch::asm!("msr daifset, #2", options(nomem, nostack));
    }

    use crate::logger::KERNEL_LOG_BUF;
    use core::sync::atomic::Ordering;

    // Check for recursive/double panic immediately. Use swap so that only the
    // FIRST CPU to reach this point proceeds with full diagnostics; all others
    // disable interrupts and halt forever without printing anything, keeping
    // the serial console clean.
    if PANICKED.swap(true, Ordering::SeqCst) {
        // Another CPU already owns the panic output path.  Halt silently.
        unsafe {
            #[cfg(target_arch = "x86_64")]
            core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
        }
        loop { kevlar_platform::arch::halt(); }
    }

    // Freeze all other CPUs immediately so they stop printing to the serial
    // console.  This eliminates interleaved TEST_PASS / double-panic noise.
    kevlar_platform::arch::broadcast_halt_ipi();

    // Print the raw panic location using a crash-safe path that avoids
    // PanicInfo::fmt (which crashes when the PanicInfo is corrupt — e.g. when
    // an SMP race writes to the panicking thread's stack between the panic!()
    // call and the handler being invoked).  `location()` returns a reference
    // to a static Location whose `file` field is always a &'static str.
    {
        let cpu = kevlar_platform::arch::cpu_id();
        let printer = kevlar_platform::print::get_debug_printer();
        printer.print_bytes(b"\n[PANIC] CPU=");
        // Print CPU id without allocation.
        let mut cpu_buf = [0u8; 4];
        let mut pos = 4usize;
        let mut n = cpu;
        if n == 0 {
            pos -= 1;
            cpu_buf[pos] = b'0';
        } else {
            while n > 0 {
                pos -= 1;
                cpu_buf[pos] = b'0' + (n % 10) as u8;
                n /= 10;
            }
        }
        printer.print_bytes(&cpu_buf[pos..]);
        printer.print_bytes(b" at ");
        if let Some(loc) = info.location() {
            printer.print_bytes(loc.file().as_bytes());
            printer.print_bytes(b":");
            // Print line number without allocation.
            let mut line_buf = [0u8; 12];
            let mut pos = 12usize;
            let mut n = loc.line();
            if n == 0 {
                pos -= 1;
                line_buf[pos] = b'0';
            } else {
                while n > 0 {
                    pos -= 1;
                    line_buf[pos] = b'0' + (n % 10) as u8;
                    n /= 10;
                }
            }
            printer.print_bytes(&line_buf[pos..]);
        } else {
            printer.print_bytes(b"(no location)");
        }
        printer.print_bytes(b"\n");
    }

    // Capture the panic message NOW, before begin_panic might corrupt `info`
    // by unwinding through the interrupt handler frame that owns the
    // fmt::Arguments embedded in PanicInfo.
    use core::fmt::Write;
    let mut msg_buf = arrayvec::ArrayString::<512>::new();
    let _ = write!(msg_buf, "{}", info);

    // Under Fortress/Balanced: try to unwind to a catch_unwind frame.
    // If a service triggered this panic, execution will resume at the
    // catch_unwind call site in services.rs, returning Err.
    // If no catch frame exists, begin_panic returns and we fall through
    // to the crash dump below.
    #[cfg(any(feature = "profile-fortress", feature = "profile-balanced"))]
    {
        use alloc::boxed::Box;
        let _ = unwinding::panic::begin_panic(Box::new(alloc::string::String::from(msg_buf.as_str())));
        // begin_panic returned — no catch frame found, this is a core panic.
    }

    // Emit structured panic event for LLM/MCP consumption.
    {
        use crate::debug::{self, DebugEvent, DebugFilter};

        // Capture backtrace frames into stack-allocated array.
        let bt_frames = kevlar_platform::backtrace::capture_frames();
        let mut event_frames = [crate::debug::event::BtFrame {
            addr: 0,
            symbol: "",
            offset: 0,
        }; 16];
        let frame_count = core::cmp::min(bt_frames.len(), 16);
        for (i, f) in bt_frames.iter().take(16).enumerate() {
            event_frames[i] = crate::debug::event::BtFrame {
                addr: f.addr,
                symbol: f.symbol,
                offset: f.offset,
            };
        }

        // Force-enable panic filter for this one event.
        let old_filter = debug::emit::get_filter();
        debug::set_filter(old_filter | DebugFilter::PANIC);
        debug::emit(DebugFilter::PANIC, &DebugEvent::Panic {
            message: msg_buf.as_str(),
            backtrace: &event_frames[..frame_count],
        });
        debug::set_filter(old_filter);
    }

    error!("{}", msg_buf.as_str());
    kevlar_platform::backtrace::backtrace();

    // Dump the per-CPU flight recorder — shows what all CPUs were doing
    // in the moments before the crash.  Other CPUs are halted by this point.
    kevlar_platform::flight_recorder::dump();

    // Dump the hierarchical tracer if enabled — shows nested call chains.
    crate::debug::htrace::dump_all_cpus();

    // ktrace: dump binary trace via debugcon (fast — ~400ms for 2MB).
    #[cfg(feature = "ktrace")]
    if crate::debug::ktrace::is_enabled() {
        crate::debug::ktrace::dump();
    }

    unsafe {
        warn!("preparing a crash dump...");
        KERNEL_LOG_BUF.force_unlock();
        let dump_buf = &raw mut KERNEL_DUMP_BUF;
        let mut off = 0;
        let mut log_buffer = KERNEL_LOG_BUF.lock();
        let dump_ref = &mut *dump_buf;
        while let Some(slice) = log_buffer.pop_slice(dump_ref.log.len().saturating_sub(off))
        {
            dump_ref.log[off..(off + slice.len())].copy_from_slice(slice);
            off += slice.len();
        }

        dump_ref.magic = 0xdeadbeee;
        dump_ref.len = off as u32;

        warn!("prepared crash dump: log_len={}", off);

        // Emit the crash dump over serial as base64, framed by sentinel markers.
        // run-qemu.py / the crash analyzer can detect the sentinels and decode
        // the dump automatically.  This avoids the boot2dump approach (requires
        // a virtio-blk disk that is absent from most QEMU test runs).
        let dump_as_bytes = core::slice::from_raw_parts(
            dump_buf as *const u8,
            core::mem::size_of::<KernelDump>(),
        );

        {
            use kevlar_platform::print::get_debug_printer;
            let printer = get_debug_printer();
            printer.print_bytes(b"\n===KEVLAR_CRASH_DUMP_BEGIN===\n");

            const B64: &[u8; 64] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut col = 0usize;
            let mut i = 0usize;

            while i + 3 <= dump_as_bytes.len() {
                let b0 = dump_as_bytes[i];
                let b1 = dump_as_bytes[i + 1];
                let b2 = dump_as_bytes[i + 2];
                let out = [
                    B64[(b0 >> 2) as usize],
                    B64[(((b0 & 3) << 4) | (b1 >> 4)) as usize],
                    B64[(((b1 & 0xf) << 2) | (b2 >> 6)) as usize],
                    B64[(b2 & 0x3f) as usize],
                ];
                printer.print_bytes(&out);
                col += 4;
                if col >= 76 {
                    printer.print_bytes(b"\n");
                    col = 0;
                }
                i += 3;
            }

            // Handle the remaining 1 or 2 bytes with standard base64 padding.
            match dump_as_bytes.len() - i {
                1 => {
                    let b0 = dump_as_bytes[i];
                    printer.print_bytes(&[
                        B64[(b0 >> 2) as usize],
                        B64[((b0 & 3) << 4) as usize],
                        b'=',
                        b'=',
                    ]);
                }
                2 => {
                    let b0 = dump_as_bytes[i];
                    let b1 = dump_as_bytes[i + 1];
                    printer.print_bytes(&[
                        B64[(b0 >> 2) as usize],
                        B64[(((b0 & 3) << 4) | (b1 >> 4)) as usize],
                        B64[((b1 & 0xf) << 2) as usize],
                        b'=',
                    ]);
                }
                _ => {}
            }

            printer.print_bytes(b"\n===KEVLAR_CRASH_DUMP_END===\n");
        }
    }

    loop { kevlar_platform::arch::halt(); }
}
