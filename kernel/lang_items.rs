// SPDX-License-Identifier: MIT OR Apache-2.0
use core::sync::atomic::AtomicBool;

// Provide our own mem* functions instead of compiler-builtins-mem,
// because the compiler-builtins implementations use u128/SSE operations
// which are not available in our no-SSE kernel target.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    let mut i = 0;
    while i + 8 <= n {
        unsafe {
            (dest.add(i) as *mut u64).write_unaligned((src.add(i) as *const u64).read_unaligned());
        }
        i += 8;
    }
    while i < n {
        unsafe {
            *dest.add(i) = *src.add(i);
        }
        i += 1;
    }
    dest
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memmove(dest: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if (dest as usize) <= (src as usize) {
        unsafe { memcpy(dest, src, n) }
    } else {
        let mut i = n;
        while i > 0 {
            i -= 1;
            unsafe {
                *dest.add(i) = *src.add(i);
            }
        }
        dest
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memset(dest: *mut u8, c: i32, n: usize) -> *mut u8 {
    let byte = c as u8;
    let word = (byte as u64)
        | (byte as u64) << 8
        | (byte as u64) << 16
        | (byte as u64) << 24
        | (byte as u64) << 32
        | (byte as u64) << 40
        | (byte as u64) << 48
        | (byte as u64) << 56;
    let mut i = 0;
    while i + 8 <= n {
        unsafe {
            (dest.add(i) as *mut u64).write_unaligned(word);
        }
        i += 8;
    }
    while i < n {
        unsafe {
            *dest.add(i) = byte;
        }
        i += 1;
    }
    dest
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    let mut i = 0;
    while i < n {
        let av = unsafe { *a.add(i) };
        let bv = unsafe { *b.add(i) };
        if av != bv {
            return (av as i32) - (bv as i32);
        }
        i += 1;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn bcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    unsafe { memcmp(a, b, n) }
}

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
        kevlar_runtime::print::get_debug_printer().print_bytes(b"\ndouble panic!\n");
        kevlar_runtime::arch::halt();
    }

    PANICKED.store(true, Ordering::SeqCst);
    error!("{}", info);
    kevlar_runtime::backtrace::backtrace();

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
