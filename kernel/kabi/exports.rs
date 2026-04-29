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
    if let Some(s) = all().iter().find(|s| s.name == name) {
        return Some(s.addr as usize);
    }
    // Phase 8 (ext4 arc): also search the runtime table populated
    // by previously-loaded `.ko` modules' `__ksymtab` sections.
    runtime::lookup(name)
}

/// Runtime exports — populated by `loader::load_module` from each
/// `.ko`'s `__ksymtab` section after relocations are applied.  Lets
/// ext4.ko find symbols exported by the previously-loaded jbd2.ko
/// without us hand-stubbing every one.
pub mod runtime {
    use alloc::string::String;
    use alloc::vec::Vec;
    use kevlar_platform::spinlock::SpinLock;

    pub struct RuntimeExport {
        pub name: String,
        pub addr: usize,
    }

    pub static RUNTIME_EXPORTS: SpinLock<Vec<RuntimeExport>> =
        SpinLock::new(Vec::new());

    pub fn lookup(name: &str) -> Option<usize> {
        let table = RUNTIME_EXPORTS.lock();
        for entry in table.iter() {
            if entry.name == name {
                return Some(entry.addr);
            }
        }
        None
    }

    pub fn register(name: &str, addr: usize) {
        let mut table = RUNTIME_EXPORTS.lock();
        // Last-loaded wins; warn on duplicate so collisions are visible.
        if let Some(existing) = table.iter_mut().find(|e| e.name == name) {
            log::warn!(
                "kabi: runtime export {:?} re-registered \
                 (was {:#x}, now {:#x})",
                name, existing.addr, addr,
            );
            existing.addr = addr;
            return;
        }
        table.push(RuntimeExport { name: String::from(name), addr });
    }

    pub fn count() -> usize {
        RUNTIME_EXPORTS.lock().len()
    }
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

/// Export a kernel-side `static` (data, not code) under its source
/// name.  Modules `extern` it with a matching declaration and the
/// loader resolves the symbol to the static's address.  Used for
/// kABI globals like `platform_bus_type`.
#[macro_export]
macro_rules! ksym_static {
    ($item:ident) => {
        const _: () = {
            #[allow(unsafe_code)]
            #[unsafe(link_section = ".ksymtab")]
            #[used]
            static __KSYM_ENTRY: $crate::kabi::exports::KSym =
                $crate::kabi::exports::KSym {
                    name: stringify!($item),
                    addr: &raw const $item as *const (),
                };
        };
    };
}

/// Export a function under an explicit name string, decoupled from
/// its Rust identifier.  Useful when the kABI shim needs to use a
/// non-conflicting Rust name (e.g. `kabi_copy_to_user`) while
/// modules still link against the canonical Linux name
/// (`copy_to_user`).
#[macro_export]
macro_rules! ksym_named {
    ($name:literal, $func:ident) => {
        const _: () = {
            #[allow(unsafe_code)]
            #[unsafe(link_section = ".ksymtab")]
            #[used]
            static __KSYM_ENTRY: $crate::kabi::exports::KSym =
                $crate::kabi::exports::KSym {
                    name: $name,
                    addr: $func as *const (),
                };
        };
    };
}
