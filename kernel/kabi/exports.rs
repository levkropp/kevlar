// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Kernel-side symbol exports — the "Linux kABI surface" we expose to
//! loaded `.ko` modules.
//!
//! Every kernel function callable from a loaded module gets a
//! `ksym!(func_name)` invocation, which emits a `KSym` entry into the
//! `.ksymtab` linker section.  At module-load time, undefined symbols
//! in the module's `.symtab` are resolved by linear-searching this
//! table.  K1 has exactly one entry (`printk`); K2+ adds the slab,
//! wait_queue, work_queue, completion, etc. surface.
//!
//! Boundary symbols `__ksymtab_start` and `__ksymtab_end` are
//! provided by the linker scripts (see `kernel/arch/{arm64,x64}/*.ld`).

#[repr(C)]
pub struct KSym {
    pub name: &'static str,
    /// Raw function pointer.  We hold this as `*const ()` instead of
    /// `usize` because casting `fn(...) -> ...` to `usize` is
    /// disallowed in const context (Rust 2024); the cast to `*const ()`
    /// is fine.  Modules see it as an `extern "C"` function pointer
    /// after the loader does the final transmute at call time.
    pub addr: *const (),
}

// SAFETY: function pointers stored as `*const ()` are read-only and
// trivially Send/Sync.  The static lives in .rodata.
#[allow(unsafe_code)]
unsafe impl Sync for KSym {}

#[allow(unsafe_code)]
unsafe extern "C" {
    static __ksymtab_start: KSym;
    static __ksymtab_end: KSym;
}

/// All kernel symbols exported to loaded modules.
pub fn all() -> &'static [KSym] {
    #[allow(unsafe_code)]
    unsafe {
        let start = &raw const __ksymtab_start;
        let end = &raw const __ksymtab_end;
        let len = end.offset_from(start) as usize;
        core::slice::from_raw_parts(start, len)
    }
}

/// Linear-search the kernel exports table.  K1 has only one entry;
/// K2+ will sort + binary-search.
pub fn lookup(name: &str) -> Option<usize> {
    all().iter().find(|s| s.name == name).map(|s| s.addr as usize)
}

/// Declare a kernel symbol exportable to loaded `.ko` modules.
///
/// The macro emits a `KSym` static into the `.ksymtab` section, which
/// the linker scripts collect between `__ksymtab_start` and
/// `__ksymtab_end`.  `#[used]` + `KEEP(*(.ksymtab))` together prevent
/// `--gc-sections` from dropping the entry.
#[macro_export]
macro_rules! ksym {
    ($func:ident) => {
        const _: () = {
            #[allow(unsafe_code)]
            #[unsafe(link_section = ".ksymtab")]
            #[used]
            static __KSYM_ENTRY: $crate::kabi::exports::KSym =
                $crate::kabi::exports::KSym {
                    name: stringify!($func),
                    addr: $func as *const (),
                };
        };
    };
}
