# 287 — Phases 1 + 2: sp_el0 task mock + folio ERR_PTR safety

The K35 investigation closed with a clear two-layer
diagnosis: Linux fs `.ko` code reads `sp_el0` as
`current_task *`, and Kevlar's `sp_el0` holds the user
stack pointer.  Plus the K34 finding: the data-flow path
ultimately needs Linux-PAGE_OFFSET-relative VAs that we
don't have mapped.

Two phases shipped this turn that take the mount chain
from "deterministic crash inside fc_fill_super" to "runs
end-to-end with clean errno propagation, zero panics."

## Phase 1 — `sp_el0` task_struct mock

### What broke

erofs.ko's compiled `erofs_read_superblock` opens with the
arm64 stack-protect prologue:

```
43d8: mrs   x0, sp_el0
43dc: ldr   x1, [x0, #1912]    // current->stack_canary
43e0: str   x1, [sp, #40]
```

Linux 7.0 arm64 stores the running task's `task_struct *`
in `sp_el0`.  Kernel functions read it via `mrs`, then
dereference fields like `stack_canary` (+1912), `pid`,
`mm`, etc.

Kevlar uses `sp_el0` differently: it holds the user-space
stack pointer, saved/restored on EL0↔EL1 transitions via
`PtRegs[31]`.  When erofs's stack-protect prologue ran, it
read whatever was last in `sp_el0` (typically a user SP),
treated it as a `task_struct *`, and faulted on the
1912-byte deref.

### What ships

Per-CPU 4 KiB `KabiTaskMock` buffer with the few Linux
task_struct fields fs `.ko` code reads — currently just
`stack_canary` at +1912, set to a recognisable sentinel
`0xDEAD_DEAD_BEEF_BEEF`.  More fields added as future fs
code reads them.

```rust
// kernel/kabi/task_mock.rs
pub const STACK_CANARY_OFF: usize = 1912;
const STACK_CANARY_SENTINEL: u64 = 0xDEAD_DEAD_BEEF_BEEF;

#[repr(C, align(4096))]
struct AlignedMock(UnsafeCell<[u8; 4096]>);

static MOCKS: [AlignedMock; MAX_CPUS] = [...];

pub fn install_for_current_cpu() {
    let cpu = arch::cpu_id() as usize;
    let addr = mock_addr_for(cpu);
    let head = arch::arm64_specific::cpu_local_head();
    head.kabi_task_mock_ptr = addr;
    unsafe { core::arch::asm!("msr sp_el0, {}", in(reg) addr); }
}
```

Trap.S `SAVE_REGS` sets `sp_el0` to the mock on every
EL0→EL1 entry:

```asm
mrs     x0, sp_el0
str     x0, [sp, #(31 * 8)]      // save user SP to PtRegs[31]
mrs     x0, tpidr_el1
ldr     x0, [x0, #32]            // CpuLocalHead.kabi_task_mock_ptr
cbz     x0, 1f                   // skip if not installed
msr     sp_el0, x0               // sp_el0 = per-CPU mock
1:
```

CpuLocalHead gets a new field at offset 32:

```rust
pub struct CpuLocalHead {
    pub sp_el1: u64,                  // +0
    pub sp_el0_save: u64,             // +8
    pub preempt_count: u32,           // +16
    pub need_resched: u32,            // +20
    pub fp_owner: u64,                // +24
    pub kabi_task_mock_ptr: u64,      // +32  (NEW)
}
```

`RESTORE_REGS` is unchanged — user SP from `PtRegs[31]`
overwrites the mock pointer on EL1→EL0, which is what
already happens.

### Verification

A boot-time probe right before `fill_super` dispatch:

```
kabi: pre-fill_super sp_el0=0xffff00004207c000
      canary[+1912]=0xdeaddeadbeefbeef
```

The canary read works through the same `mrs sp_el0; ldr
[+1912]` sequence erofs uses.  Mount progressed past the
stack-protect prologue.

## Phase 2 — folio stubs return ERR_PTR

### What broke (next layer)

After Phase 1, erofs got past the stack-protect check and
called `erofs_read_metabuf` to read the on-disk superblock.
That dispatched through Linux's inline
`read_mapping_folio` → `read_cache_folio` (our kABI shim)
→ stub returning null.

Erofs's `IS_ERR(folio)` check is Linux's `(unsigned long)ptr
>= -4095`.  Null is **not** in that range — null means
"miss" not "error".  So erofs treated the null folio as
valid and called inline `kmap_local_page(NULL)`.  That
expansion does:

```c
__addr = PAGE_OFFSET + ((NULL - VMEMMAP_START)
                        / sizeof(struct page)) * PAGE_SIZE;
```

— pointer arithmetic from null gave a Linux-PAGE_OFFSET-
relative VA at `0xffff_8010_0000_0400`, which we don't
have mapped.  Deterministic L0 translation fault.

### What ships

Shift `__filemap_get_folio_mpol`, `pagecache_get_page`,
`filemap_alloc_folio_noprof`, and `read_cache_folio` from
returning null to returning `ERR_PTR(-EIO)`:

```rust
#[inline]
fn err_ptr_eio() -> *mut c_void {
    super::block::err_ptr(-5)
}

#[unsafe(no_mangle)]
pub extern "C" fn read_cache_folio(...) -> *mut c_void {
    log::warn!("kabi: read_cache_folio (stub) — ERR_PTR(-EIO)");
    err_ptr_eio()
}
```

The IS_ERR convention encodes negative errnos as
"impossible-to-be-valid" pointer values.  Erofs's check
catches them; the caller propagates `-EIO` up through the
mount chain; `get_tree_nodev_synth` sees `fill_super
returned -5`, frees the sb, and returns the error.

`find_get_page` keeps returning null because its API
contract is "null on miss, no IS_ERR".

### Verification

```
kabi: erofs init_module returned 0
kabi: erofs init_fs_context returned 0 — fc->ops populated
kabi: filp_open_synth verify: f_mapping=... f_inode=...
      i_mode=0o100644 a_ops=... read_folio=...
kabi: get_tree_nodev_synth: sb=..., calling fill_super
kabi: read_cache_folio (stub) — ERR_PTR(-EIO)
kabi: get_tree_nodev_synth: fill_super returned -5 — bailing
kabi: erofs ops->get_tree returned -5
```

Zero panics.  The mount chain traverses every kABI surface
and returns errno cleanly back to the caller.  Default
boot 5/5 LXDE 8/8 pass.

## What's actually shippable

Two commits this session:

  * **`0da4dbf`** Phase 1 — `kernel/kabi/task_mock.rs` (NEW),
    `platform/arm64/cpu_local.rs` (CpuLocalHead +
    `kabi_task_mock_ptr`), `platform/arm64/trap.S`
    (SAVE_REGS sets `sp_el0` from CpuLocalHead).
  * **`d770a3a`** Phase 2 — `kernel/kabi/filemap.rs`
    (folio stubs return `ERR_PTR(-EIO)` instead of null).

Plus the phased plan file at
`.claude/plans/ethereal-nibbling-treehouse.md`.

## What's left

The full phase plan, in order:

| Phase | Goal | Status |
|---|---|---|
| **1** | sp_el0 task mock | ✅ |
| **2** | Folio ERR_PTR safety | ✅ |
| **3** | Real folio + read_folio backed by initramfs | ⏳ |
| **4** | Real super_block + inode + dentry alloc | ⏳ |
| **5** | `KabiDirectory` (lookup, readdir) | ⏳ |
| **6** | `KabiFile` (read regular file contents) | ⏳ |
| **7** | Userspace mount(2) + `ls /mnt/erofs` | ⏳ |

Phase 3 is where the work gets interesting.  We need
`read_cache_folio` to return a folio whose data buffer is
accessible at the address Linux's inline
`kmap_local_page` computes — which means either:

  * (a) Deriving the runtime values of `VMEMMAP_START` and
    `PAGE_OFFSET` Linux's compiled code uses, then
    synthesising page pointers that satisfy
    `kmap_local_page(page) = our_buffer_VA`.
  * (b) Setting up arm64 page-table aliases that map
    Kevlar paddrs at Linux's expected
    `0xffff_8000_0000_0000`-relative VAs across the
    whole kernel direct map.

Both are tractable but neither is single-commit work.
The good news: the phases stack — Phase 3 doesn't have
to find the next compat layer.  When it lands, Phase 4
becomes "iterate on the next struct field erofs reads"
the same way K33 Phase 2c found shrinker_alloc and
alloc_workqueue_noprof.

## Status

| Milestone | Status |
|---|---|
| K30-K33: kABI ext4 control flow | ✅ |
| K34: filp_open synth + get_tree_nodev | ✅ |
| K35: investigation, struct-offset fixes, sp_el0 finding | ✅ |
| Phase 1: sp_el0 task mock | ✅ |
| Phase 2: folio ERR_PTR safety | ✅ |
| Phase 3-7: real data flow | ⏳ |

The interesting moment: erofs.ko now runs through every
kABI shim, reads our mock task_struct, attempts to read
disk pages, sees a clean "no folio available" error,
bails gracefully.  Every line of erofs's compiled mount
machinery is reachable from Kevlar.  The remaining work
is feeding it real data instead of error pointers.
