// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! Module-load orchestration: read .ko bytes, lay sections out in
//! kernel memory, copy + relocate, and locate the entry function.

use alloc::string::String;
use alloc::vec::Vec;

use kevlar_platform::address::VAddr;
use kevlar_platform::page_allocator::{alloc_pages, AllocPageFlags};
use kevlar_vfs::file_system::FileSystem;

use crate::fs::initramfs::INITRAM_FS;
use crate::fs::opened_file::OpenOptions;
use crate::kabi::elf::{self, RelObj, SHF_ALLOC, SHT_NOBITS, SHT_PROGBITS};
use crate::kabi::exports;
use crate::kabi::reloc;
use crate::kabi::symbols;
use crate::prelude::*;

const PAGE_SIZE: usize = 4096;

/// A loaded kernel module — bookkeeping enough to find the entry
/// function.  Real Linux's `struct module` is K2+ scope.
pub struct LoadedModule {
    pub name: String,
    pub base: VAddr,
    pub len: usize,
    /// Resolved address of the requested entry function (e.g. `my_init`),
    /// if one was found.
    pub init_fn: Option<extern "C" fn() -> i32>,
}

impl LoadedModule {
    /// Invoke the module's init function with proper Linux ABI setup.
    ///
    /// On arm64, Linux 7.0 modules built with `CONFIG_SHADOW_CALL_STACK=y`
    /// (Ubuntu's default) use `x18` as the shadow-call-stack pointer.
    /// Function entry executes `str x30, [x18], #8` which writes the
    /// LR to the SCS area; function exit reverses.  Our Rust kernel
    /// doesn't use SCS, so x18 is uninitialized when we call into the
    /// module — and the SCS write traps.  Allocate a small SCS area
    /// here, point x18 at it, then call.  After the call x18 is back
    /// where we put it (init function's epilogue pops its own write).
    pub fn call_init(&self) -> Option<i32> {
        let f = self.init_fn?;
        Some(call_module_init_with_scs(f))
    }
}

#[cfg(target_arch = "aarch64")]
#[allow(unsafe_code)]
fn call_module_init_with_scs(f: extern "C" fn() -> i32) -> i32 {
    call_with_scs_1(f as *const (), 0) as i32
}

#[cfg(not(target_arch = "aarch64"))]
fn call_module_init_with_scs(f: extern "C" fn() -> i32) -> i32 {
    f()
}

/// Call a Linux module function (1 pointer arg → i32) with a fresh
/// shadow-call-stack pointer in `x18`.  Same rationale as
/// `call_module_init_with_scs`, generalised for the deeper kABI
/// dispatch paths (`init_fs_context`, `ops->get_tree`, `fc_fill_super`,
/// etc.).  Without SCS handling on these calls, the module's
/// `str x30, [x18], #8` prologue can write to whatever x18 happens
/// to hold — fine when it lands on writable memory, fatal when it
/// doesn't.  The fault HVF can't classify when it lands on an RO
/// page is one such case.
///
/// 8 KiB SCS — plenty for fc_fill_super's deepest call chain.
#[cfg(target_arch = "aarch64")]
#[allow(unsafe_code)]
pub fn call_with_scs_1(f: *const (), arg0: usize) -> isize {
    let mut scs: Vec<u8> = alloc::vec![0u8; 65536];
    let scs_ptr = scs.as_mut_ptr();
    let result: isize;
    unsafe {
        // Save x18 on the stack across the .ko call: AAPCS64 says x9
        // is caller-saved, so the .ko's call chain will clobber it
        // and any "saved" copy of x18 we leave in x9 gets corrupted.
        // After the call, we need x18 back to whatever Rust set it
        // to, so save+restore via the stack (16-byte SP alignment).
        core::arch::asm!(
            "str x18, [sp, #-16]!",
            "mov x18, {scs}",
            "blr {fp}",
            "ldr x18, [sp], #16",
            scs = in(reg) scs_ptr,
            fp = in(reg) f,
            in("x0") arg0,
            lateout("x0") result,
            clobber_abi("C"),
        );
    }
    drop(scs);
    result
}

#[cfg(not(target_arch = "aarch64"))]
pub fn call_with_scs_1(f: *const (), arg0: usize) -> isize {
    let f: extern "C" fn(usize) -> isize = unsafe { core::mem::transmute(f) };
    f(arg0)
}

/// 2-arg variant for `fill_super(sb, fc)`.
///
/// Same stack-save pattern as `call_with_scs_1`: x9 is caller-saved
/// in AAPCS64, so saving x18 in x9 across `blr` lets the .ko's deep
/// call chain clobber it; the closing `mov x18, x9` then restores
/// garbage.  Use the regular stack instead.
#[cfg(target_arch = "aarch64")]
#[allow(unsafe_code)]
pub fn call_with_scs_2(f: *const (), arg0: usize, arg1: usize) -> isize {
    let mut scs: Vec<u8> = alloc::vec![0u8; 65536];
    let scs_ptr = scs.as_mut_ptr();
    let result: isize;
    unsafe {
        core::arch::asm!(
            "str x18, [sp, #-16]!",
            "mov x18, {scs}",
            "blr {fp}",
            "ldr x18, [sp], #16",
            scs = in(reg) scs_ptr,
            fp = in(reg) f,
            in("x0") arg0,
            in("x1") arg1,
            lateout("x0") result,
            clobber_abi("C"),
        );
    }
    drop(scs);
    result
}

#[cfg(not(target_arch = "aarch64"))]
pub fn call_with_scs_2(f: *const (), arg0: usize, arg1: usize) -> isize {
    let f: extern "C" fn(usize, usize) -> isize = unsafe { core::mem::transmute(f) };
    f(arg0, arg1)
}

/// Load a `.ko` from the initramfs at `path`, resolve its undefined
/// symbols against the kernel exports, apply relocations, and return
/// a `LoadedModule` whose `init_fn` is the address of the symbol named
/// `init_sym` (typically `"my_init"` for K1).
pub fn load_module(path: &str, init_sym: &str) -> Result<LoadedModule> {
    let bytes = read_initramfs_file(path)?;
    let obj = RelObj::parse(&bytes)?;

    log::info!(
        "kabi: loaded {} ({} bytes, {} sections, {} symbols)",
        path,
        bytes.len(),
        obj.sections.len(),
        obj.symtab.len()
    );

    // ── .modinfo parse ────────────────────────────────────────────
    if let Some(mi_bytes) = find_section_bytes(&obj, ".modinfo") {
        let info = crate::kabi::modinfo::parse(mi_bytes);
        log::info!(
            "kabi: {} license={:?} author={:?} desc={:?}",
            path, info.license, info.author, info.description
        );
        if info.depends.as_deref().unwrap_or("").len() > 0 {
            log::info!("kabi: {} depends={:?}", path, info.depends);
        }
    }

    // ── Layout pass ───────────────────────────────────────────────
    //
    // Walk SHF_ALLOC sections in-order, computing per-section offsets
    // within the module image.  Order them naturally: text first,
    // then rodata, then data, then bss — but rather than sorting by
    // name we just respect the section header order from the linker,
    // which is already that ordering for `aarch64-linux-musl-gcc -c`.
    let mut layout: Vec<Option<usize>> = (0..obj.sections.len()).map(|_| None).collect();
    let mut total: usize = 0;
    for (i, sh) in obj.sections.iter().enumerate() {
        if (sh.sh_flags & SHF_ALLOC) == 0 {
            continue;
        }
        let align = if sh.sh_addralign == 0 { 1 } else { sh.sh_addralign as usize };
        total = align_up(total, align);
        layout[i] = Some(total);
        total += sh.sh_size as usize;
    }

    if total == 0 {
        log::warn!("kabi: no allocatable sections in {}", path);
        return Err(Errno::ENOEXEC.into());
    }

    // Reserve a PLT-style trampoline area at the end of the image.
    // The kernel direct map can place module pages well beyond
    // CALL26's ±128MB range from the kernel `.text` (where exported
    // symbols like `printk` live).  When that happens, we materialize
    // a 16-byte stub `ldr x16, [pc,#8]; br x16; .quad sym_va` here
    // and retarget the CALL26 to point at the stub.
    total = align_up(total, 16);
    let stub_area_offset = total;
    const MAX_STUBS: usize = 256;
    const STUB_SIZE: usize = 16;
    total += MAX_STUBS * STUB_SIZE;

    let total_pages = (align_up(total, PAGE_SIZE)) / PAGE_SIZE;
    let paddr = alloc_pages(total_pages, AllocPageFlags::KERNEL)
        .map_err(|_| Error::new(Errno::ENOMEM))?;
    let base_va = paddr.as_vaddr();

    log::info!(
        "kabi: image layout: {} bytes ({} pages) at {:#x}",
        total, total_pages, base_va.value()
    );

    // ── Copy / zero pass ───────────────────────────────────────────
    let mut section_va_map: Vec<Option<usize>> = (0..obj.sections.len()).map(|_| None).collect();
    for (i, sh) in obj.sections.iter().enumerate() {
        let off = match layout[i] {
            Some(o) => o,
            None => continue,
        };
        let dst = base_va.value() + off;
        section_va_map[i] = Some(dst);

        let size = sh.sh_size as usize;
        match sh.sh_type {
            SHT_PROGBITS => {
                let src_off = sh.sh_offset as usize;
                let src_end = src_off + size;
                if src_end > bytes.len() {
                    log::warn!("kabi: section {} extends past file end", i);
                    return Err(Errno::ENOEXEC.into());
                }
                #[allow(unsafe_code)]
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        bytes.as_ptr().add(src_off),
                        dst as *mut u8,
                        size,
                    );
                }
            }
            SHT_NOBITS => {
                #[allow(unsafe_code)]
                unsafe {
                    core::ptr::write_bytes(dst as *mut u8, 0, size);
                }
            }
            _ => {} // other section types in the layout — shouldn't happen post-SHF_ALLOC filter
        }
    }

    // ── Pre-pass: enumerate all undefined external symbols ──────
    // Bailing on the first missing kABI export makes incremental
    // development painful — you fix one, hit the next, repeat.
    // Log the complete list up front so a single boot identifies
    // the entire batch of stubs to add.
    {
        let mut missing: Vec<&str> = Vec::new();
        for sym in obj.symtab.iter() {
            if sym.st_shndx == symbols::SHN_UNDEF {
                let name = obj.sym_name(sym);
                if name.is_empty() {
                    continue;
                }
                if exports::lookup(name).is_none()
                    && !missing.iter().any(|n| *n == name)
                {
                    missing.push(name);
                }
            }
        }
        if !missing.is_empty() {
            log::warn!(
                "kabi: {} undefined external symbol(s) for {}:",
                missing.len(), path,
            );
            for name in &missing {
                log::warn!("    UNDEF: {}", name);
            }
        } else {
            log::info!("kabi: all external symbols resolved for {}", path);
        }
    }

    // ── Relocation pass ───────────────────────────────────────────
    // CALL26/JUMP26 reach is ±128MB; if the symbol is farther, route
    // through a 16-byte trampoline allocated in the stub area.  Reuse
    // stubs across multiple references to the same symbol.
    let mut stubs: Vec<(usize, usize)> = Vec::new();
    let mut stubs_used: usize = 0;

    let mut nrelocs = 0usize;
    for table in obj.rela_tables() {
        // Only apply relocations against sections we loaded.
        let target_va = match section_va_map.get(table.target_section).and_then(|v| *v) {
            Some(va) => va,
            None => continue,
        };

        for rel in table.entries {
            #[allow(unsafe_code)]
            let r_info = rel.r_info;
            let r_sym = (r_info >> 32) as u32;
            let r_type = (r_info & 0xffff_ffff) as u32;

            let sym = obj
                .symtab
                .get(r_sym as usize)
                .ok_or_else(|| Error::new(Errno::ENOEXEC))?;
            let sym_name = obj.sym_name(sym);

            let sym_va = symbols::resolve(
                sym.st_shndx,
                sym.st_value,
                sym_name,
                &section_va_map,
            )?;

            let target_addr = target_va + rel.r_offset as usize;

            // For aarch64 CALL26 (283) / JUMP26 (282), substitute a
            // trampoline address if the direct branch is out of range.
            let effective_sym_va = if r_type == 282 || r_type == 283 {
                let off = (sym_va as i64) - (target_addr as i64);
                if off >= -(128i64 << 20) && off < (128i64 << 20) {
                    sym_va
                } else if let Some(&(_, sv)) = stubs.iter().find(|(s, _)| *s == sym_va) {
                    sv
                } else {
                    if stubs_used >= MAX_STUBS {
                        log::warn!("kabi: stub area exhausted ({})", MAX_STUBS);
                        return Err(Errno::ENOEXEC.into());
                    }
                    let stub_va =
                        base_va.value() + stub_area_offset + stubs_used * STUB_SIZE;
                    write_trampoline(stub_va, sym_va);
                    stubs.push((sym_va, stub_va));
                    stubs_used += 1;
                    stub_va
                }
            } else {
                sym_va
            };

            reloc::apply(r_type, target_addr, effective_sym_va, rel.r_addend)?;
            nrelocs += 1;
        }
    }
    log::info!(
        "kabi: applied {} relocations ({} trampoline(s))",
        nrelocs, stubs_used
    );

    // ── I-cache sync ──────────────────────────────────────────────
    // We just wrote new instructions into pages whose old contents
    // may sit in any CPU's I-cache.  Make those writes globally
    // observable + invalidate the I-cache before transferring control.
    kevlar_platform::arch::sync_icache_range(base_va, total);

    // ── Find the entry function ───────────────────────────────────
    let mut init_fn: Option<extern "C" fn() -> i32> = None;
    for sym in obj.symtab.iter() {
        if obj.sym_name(sym) != init_sym {
            continue;
        }
        // Must be defined inside the module (a real section index).
        if (sym.st_shndx as usize) >= obj.sections.len() {
            continue;
        }
        if let Some(Some(sec_va)) = section_va_map.get(sym.st_shndx as usize) {
            let addr = *sec_va + sym.st_value as usize;
            #[allow(unsafe_code)]
            let f: extern "C" fn() -> i32 = unsafe { core::mem::transmute(addr) };
            init_fn = Some(f);
            break;
        }
    }

    if init_fn.is_none() {
        log::warn!(
            "kabi: entry symbol '{}' not found in module {}",
            init_sym, path
        );
    }

    Ok(LoadedModule {
        name: String::from(path),
        base: base_va,
        len: total,
        init_fn,
    })
}

/// Read all bytes of an initramfs file into a kernel buffer.
fn read_initramfs_file(path: &str) -> Result<Vec<u8>> {
    use kevlar_vfs::inode::{Directory, INode};

    // INITRAM_FS is `Arc<InitramFs>` (a FileSystem).  Walk the
    // requested path component-by-component starting at the
    // filesystem's root directory.
    let initram = INITRAM_FS.clone();
    let mut current: alloc::sync::Arc<dyn Directory> = initram.root_dir()?;
    let mut last: Option<INode> = None;
    for component in path.split('/') {
        if component.is_empty() {
            continue;
        }
        let inode = current.lookup(component)?;
        match inode.clone() {
            INode::Directory(d) => {
                current = d;
                last = Some(inode);
            }
            INode::FileLike(_) | INode::Symlink(_) => {
                last = Some(inode);
            }
        }
    }
    let inode = last.ok_or_else(|| Error::new(Errno::ENOENT))?;

    let file = inode.as_file()?;
    let stat = file.stat()?;
    let size = stat.size.0 as usize;

    let mut buf: Vec<u8> = alloc::vec![0u8; size];
    let n = file.read(0, (&mut buf[..]).into(), &OpenOptions::readwrite())?;
    if n != size {
        log::warn!(
            "kabi: short read of {}: got {} of {} bytes",
            path, n, size
        );
        buf.truncate(n);
    }
    Ok(buf)
}

#[inline]
fn align_up(x: usize, a: usize) -> usize {
    (x + a - 1) & !(a - 1)
}

/// Find a section by name and return its raw bytes from the file.
/// Used by the loader to extract `.modinfo`.
fn find_section_bytes<'a>(obj: &RelObj<'a>, name: &str) -> Option<&'a [u8]> {
    for sh in obj.sections.iter() {
        if obj.section_name(sh) == name {
            let start = sh.sh_offset as usize;
            let end = start + sh.sh_size as usize;
            if end <= obj.buf.len() {
                return Some(&obj.buf[start..end]);
            }
        }
    }
    None
}

/// Materialize a 16-byte aarch64 PLT stub that performs an absolute
/// branch to `sym_va`:
///
/// ```text
///   ldr  x16, [pc, #8]   ; 0x58000050 — load the literal below
///   br   x16             ; 0xd61f0200
///   .quad sym_va
/// ```
///
/// `x16` (IP0) is callee-clobberable per the AAPCS64 procedure-call
/// standard, so a CALL26 jump landing on this stub does not corrupt
/// any caller-visible state.
#[allow(unsafe_code)]
fn write_trampoline(stub_va: usize, sym_va: usize) {
    unsafe {
        core::ptr::write_unaligned(stub_va as *mut u32, 0x5800_0050u32);
        core::ptr::write_unaligned((stub_va + 4) as *mut u32, 0xd61f_0200u32);
        core::ptr::write_unaligned((stub_va + 8) as *mut u64, sym_va as u64);
    }
}
