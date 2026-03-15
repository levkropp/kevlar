# Fork Performance Parity Plan

## Current State
- Fork kernel: 30µs (0.7x Linux ✓)
- Fork+exit+wait: 266µs (7.0x Linux ✗)
- Root cause: L1/L2 cache thrashing from 32KB per-process kernel stacks

## Linux's Architecture (what we need to match)

### 1. Per-CPU stack cache (BIGGEST WIN)
Linux caches 2 recently-freed kernel stacks per CPU (`cached_stacks[2]`).
On fork: `this_cpu_xchg(cached_stacks[i], NULL)` → lock-free, O(1).
On exit: `this_cpu_cmpxchg(cached_stacks[i], NULL, stack)` → returns to cache.

**Why this helps:** Reused stacks are WARM in L1/L2 cache. A fresh buddy
allocation returns cold memory that evicts the parent's cache lines.

**Implementation:**
- Static per-CPU array of 2 `OwnedPages` slots
- `alloc_kernel_stack()`: try per-CPU cache first, fall back to buddy
- `free_kernel_stack()`: try returning to per-CPU cache, fall back to buddy
- In fork: use `alloc_kernel_stack()` instead of `alloc_pages_owned()`
- In exit/gc: use `free_kernel_stack()` when dropping ArchTask

### 2. Per-CPU interrupt/syscall stacks (eliminate 2 allocs per fork)
Linux uses per-CPU IRQ stacks (`pcpu_hot.hardirq_stack_ptr`), not per-process.
IST stacks are per-CPU in the `cpu_entry_area`.
The syscall entry stack is just `pcpu_hot.top_of_stack` (the task's kernel stack).

**Implementation:**
- Remove `interrupt_stack` and `syscall_stack` from `ArchTask`
- Allocate them once per CPU in boot/SMP init
- `switch_task()` sets `head.rsp0` and TSS IST from per-CPU storage
- Fork no longer allocates these stacks → 2 fewer allocs, 16KB less memory

### 3. Lazy FPU restore (defer xrstor to return-to-userspace)
Linux saves FPU eagerly (`xsave` on switch) but restores LAZILY
(sets `TIF_NEED_FPU_LOAD`, actual `xrstor` happens on iretq/sysretq
return path). If the next task runs entirely in kernel (which fork's
child does before exiting), the restore is skipped entirely.

**Implementation:**
- Add `needs_fpu_load: bool` flag to ArchTask
- `switch_task()`: xsave prev, set next.needs_fpu_load = true
- `syscall_exit()` / `iret return path`: if needs_fpu_load, xrstor
- For fork+_exit child: xsave prev on switch, but child never returns
  to userspace (it calls _exit which switches back), so xrstor is skipped

### 4. Reduce kernel stack to 2 pages (8KB)
Linux uses 16KB (order-2) but many configs use 8KB. Our Rust code has
deeper call stacks due to unwinding + debug info. Test with 2 pages
first; increase to 3 (12KB) if it crashes.

**Implementation:**
- `KERNEL_STACK_SIZE = PAGE_SIZE * 2`
- Test all profiles + systemd boot
- If stack overflow: use PAGE_SIZE * 3

### 5. Slab-like task_struct allocation
Linux allocates task_struct from a kmem_cache (slab allocator) which
keeps recently freed objects warm. Our Process is heap-allocated via
Arc::new which goes through the global allocator.

**Implementation (lower priority):**
- Create a fixed-size object pool for Process structs
- On fork: pop from pool. On exit: push back to pool.
- Pool entries stay cache-warm.

## Expected Impact

| Change | Estimated saving | Cumulative |
|--------|-----------------|------------|
| Per-CPU stack cache | ~200µs (eliminates cold-cache penalty) | 66µs |
| Per-CPU IST stacks | ~10µs (2 fewer allocs, 16KB less memory) | 56µs |
| Lazy FPU restore | ~4µs (skip xrstor for fork child) | 52µs |
| 8KB kernel stack | ~5µs (less cache pressure) | 47µs |
| Target | **47µs** (1.2x Linux) | |
| Linux baseline | **35µs** | |

## Implementation Order
1. Per-CPU stack cache (biggest win, medium complexity)
2. Per-CPU IST stacks (medium win, medium complexity)
3. Lazy FPU restore (small win, small complexity)
4. Kernel stack reduction (small win, easy but risky)
