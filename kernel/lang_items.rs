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
    use crate::logger::KERNEL_LOG_BUF;
    use core::sync::atomic::Ordering;

    if PANICKED.load(Ordering::SeqCst) {
        kevlar_platform::print::get_debug_printer().print_bytes(b"\ndouble panic!\n");
        kevlar_platform::arch::halt();
    }

    // Under Fortress/Balanced: try to unwind to a catch_unwind frame.
    // If a service triggered this panic, execution will resume at the
    // catch_unwind call site in services.rs, returning Err.
    // If no catch frame exists, begin_panic returns and we fall through
    // to the crash dump below.
    #[cfg(any(feature = "profile-fortress", feature = "profile-balanced"))]
    {
        use alloc::boxed::Box;
        use alloc::string::ToString;
        let msg = info.to_string();
        let _ = unwinding::panic::begin_panic(Box::new(msg));
        // begin_panic returned — no catch frame found, this is a core panic.
    }

    PANICKED.store(true, Ordering::SeqCst);
    error!("{}", info);
    kevlar_platform::backtrace::backtrace();

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
        warn!("booting boot2dump...");
        let dump_as_bytes = core::slice::from_raw_parts(
            dump_buf as *const u8,
            core::mem::size_of::<KernelDump>(),
        );
        boot2dump::save_to_file_and_reboot("kevlar.dump", dump_as_bytes);
    }
}
