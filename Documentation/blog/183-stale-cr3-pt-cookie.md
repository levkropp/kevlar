## Blog 183: the stale-CR3 PT-cookie race

**Date:** 2026-04-19

After [blog 180](180-the-cld-bug.md) landed the CLD fix and took XFCE's
kernel-crash rate from 37% to 0/10, a residual ~25% panic crept back
when we started running the full desktop under real load. Different
signature:

```
[PANIC] CPU=0 at platform/x64/paging.rs:32
panicked: PT page 0x3890f000 cookie corrupted: 0x0 (expected 0xbeefca11deadf00d)

backtrace:
  alloc_pt_page+0x35f
  duplicate_table
  PageTable::duplicate_from
  Process::fork
  sys_fork
```

A page-table page pulled from the `PT_PAGE_POOL` cache had its magic
cookie — a sentinel value stored at PT-entry 511 of every pooled
page — overwritten with `0x0`. The magic is set on every free
(`free_pt_page`) and checked on every alloc. Between free and alloc,
nothing should touch that page. But something was.

## The setup

Kevlar caches freed page-table pages in a spin-locked `Vec<PAddr>`
(`PT_PAGE_POOL`, up to 32 entries) to skip the buddy allocator on the
fork/CoW hot path. Every PT page we put there or hand out has a
specific 8-byte sentinel at offset `0xFF8` — the last u64 of the page.
User page tables never use PT-entry 511 at level 1 (it'd map a
kernel-half VA), so if the sentinel ever changes while a page sits in
the pool, something other than our PT code wrote to it.

Teardown of a process address space calls
`flush_tlb_for_teardown` → `tlb_remote_flush_all_pcids` (IPI to every
other CPU, wait for ACK, each target runs `INVPCID type=3` which kills
every TLB entry in every PCID). After the IPI returns, no CPU has a
cached translation to any PT page in the dying address space. Then
`teardown_table` walks the tree and `free_pt_page`s every intermediate
level.

So what's writing to the freed pages?

## The flight recorder points at fork

```
seq=22545  CPU=0  CTX_SWITCH  from_pid=51 to_pid=42
seq=22546  CPU=1  PREEMPT     pid=1
... hundreds of MUNMAP from pid=37 on CPU 1 ...
seq=22XXX  [PANIC] alloc_pt_page
```

PID 37 is on CPU 1 doing a tear-down storm (a forked XFCE child
shutting down). PID 1 was just preempted on CPU 1. CPU 0 — where the
panic fires — is running a separate `fork` from some other process.

`fork` calls `duplicate_table` recursively, which calls `alloc_pt_page`
for every new PT level. The pool pop on CPU 0 finds a page whose
cookie is zero.

## The walker isn't bounded by TLB flush

The TLB flush tells every CPU "drop cached translations." But CR3 is
*not* invalidated by an `INVPCID` or IPI — it's only rewritten by an
explicit `mov cr3, ...`. The hardware page walker uses the current
CR3 when refilling the TLB after a miss. If CR3 still points at an
address space that is being torn down elsewhere, the walker can still
traverse its PT pages, read them, and write back A/D bits.

Under what circumstance does CR3 keep pointing at a dead address
space?

## The scheduler leaves CR3 untouched on Vm-less switches

```rust
// kernel/process/switch.rs
if let Some(vm) = next.vm().clone() {
    let lock = vm.lock_no_irq();
    lock.page_table().switch();  // writes CR3
}
// else: CR3 stays whatever prev had
```

Kernel threads and the idle thread don't own a `Vm`. When the
scheduler picks one as `next`, we skip the CR3 reload. CR3 keeps
whatever value `prev`'s address space gave it.

Concretely, the race is:

1. Thread T of process X runs on CPU B. CR3 = X's pml4.
2. T exits. Scheduler picks idle as next. `next.vm()` is `None`, so
   CR3 stays = X's pml4.
3. X's last reference drops. `Vm::Drop` runs `teardown_user_pages`
   on *another CPU* (A). It sends a TLB-flush IPI; CPU B's TLB is
   cleared.
4. CPU A frees X's PT pages into `PT_PAGE_POOL`.
5. Now an interrupt hits CPU B. The kernel interrupt path accesses
   some kernel VA. TLB miss (just flushed). Hardware walker reads
   CR3 (= X's pml4, still), walks the kernel half through the shared
   top-half entries, and along the way sets an A bit on some entry
   inside a PT page that CPU A has already freed and is now in the
   pool.

At offset `0xFF8`, that A-bit write turns `0xBEEF_CA11_DEAD_F00D` into
a value that doesn't match, and the next `fork` panics. Any offset
other than `0xFF8` is silent — we just have a quietly-corrupted PT
page that eventually ends up under somebody else's page tables. 🙃

## The fix

When the scheduler hands the CPU to a task with no Vm, we switch CR3
to the kernel's bootstrap PML4:

```rust
// kernel/process/switch.rs
if let Some(vm) = next.vm().clone() {
    let lock = vm.lock_no_irq();
    lock.page_table().switch();
} else {
    // Task has no Vm (idle thread, kernel thread). Load the kernel
    // bootstrap PML4 so CR3 doesn't keep pointing at the outgoing
    // task's pml4.
    kevlar_platform::arch::load_kernel_page_table();
}
```

`load_kernel_page_table` writes CR3 with `__kernel_pml4` as the paddr
and no PCID bits. `__kernel_pml4` contains only the shared kernel-half
mappings — no user PTs, nothing that can be in the dying set. From
this point on the walker on an idle CPU can never touch a user PT
page.

The cost is one CR3 write per idle-switch: a full TLB flush for
kernel mappings. In practice idle CPUs don't do much kernel work,
so the flush is cheap. There's a future optimization to skip the
write when the previous task also had no Vm (idle→kthread→idle), but
correctness comes first.

## Result

Before: 2–3 kernel panics per 8 `make test-xfce` runs, all
`paging.rs:32 cookie corrupted: 0x0`.

After: **10 pass, 0 crash out of 10**. Threading regression 14/14.

## What the class of bug looks like

This is the third structural kernel race in the XFCE chase:
- CLD: `rep stosb` ran backward because user-set DF flag persisted
  into kernel.
- IF=0 drop trap: `Vm::Drop` ran under IF=0 and the TLB flush fell
  through the IPI-requires-IF=1 guard.
- **Stale CR3 on idle**: CR3 kept pointing at a dying address space
  on CPUs that switched to a Vm-less task.

All three have the same shape: a *rule* (DF=0 for `rep`, IF=1 for
IPI-based flushes, CR3-not-pointing-at-freed-memory) that Kevlar
relied on implicitly. Each time the rule held *most* of the time, so
symptoms only fired under load and looked random. Each time the fix
was one-line or two-line, once we understood the invariant.

The hardest part was always seeing *which* invariant mattered. The
flight recorder and the PT_PAGE cookie together turned this one from
"maybe a race, maybe memory corruption somewhere" into "walker wrote
A bit at offset 0xFF8 while the page was in our pool, so CR3 must
still be live for that address space."

## Next

The kernel side of XFCE is now free of known crash bugs on this
workload. What's left for graphical XFCE is userspace: xfwm4 and
xfce4-panel still SIGSEGV intermittently (their crashes, not ours,
but they're what's keeping the `make test-xfce` score below 4/4).
The [strace-diff harness](182-strace-diff.md) is the tool for the
userspace side.
