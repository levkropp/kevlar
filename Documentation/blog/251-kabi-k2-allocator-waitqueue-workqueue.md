# 251 — kABI K2: kmalloc, wait queues, work queues, completions

K2 lands.  Loaded `.ko` modules can now allocate memory, sleep on
wait queues, schedule work to run on a kernel-thread worker,
signal completions back to a sleeping caller, and embed Linux's
`.modinfo` metadata that the loader parses on the way in.

The demo: `k2.ko` calls `kmalloc`, initializes a wait_queue_head
+ completion + work_struct, schedules the work, sleeps on the
wait queue, gets woken from the worker thread (which itself
called `vmalloc` + `msleep`), waits on the completion (already
signaled, fast-path), flushes the work, frees everything, and
returns 0.  Five primitives in concert, ~80 lines of C.

The proof on serial:

```
kabi: loaded /lib/modules/k2.ko (4632 bytes, 14 sections, 42 symbols)
kabi: /lib/modules/k2.ko license=Some("GPL") author=Some("Kevlar")
       desc=Some("kABI K2 demo: alloc + wait + work + completion")
kabi: image layout: 4992 bytes (2 pages) at 0xffff00007d140000
kabi: applied 55 relocations (18 trampoline(s))
[mod] [k2] init begin
[mod] [k2] scheduling work
kabi: workqueue worker started (pid=2)
[mod] [k2] work handler running on worker thread
[mod] [k2] work handler done
[mod] [k2] woken by worker
[mod] [k2] completion observed
[mod] [k2] init done
kabi: k2 init_module returned 0
```

55 relocations applied; 18 of them trampolined because the kABI
exports live in the kernel `.text` ~1 GB away from the heap
pages where the module image landed (same problem K1 surfaced
for `printk`; the K1 trampoline machinery scales naturally to
this scale).

`make ARCH=arm64 test-module-k2` is the regression target.  K1's
`test-module` still passes — K2 didn't regress the foundation.

## Surface

K2 ships ~30 new exported symbols across five subsystems.

### Allocator

```c
void *kmalloc(size_t, gfp_t);   void *kzalloc(size_t, gfp_t);
void *kcalloc(size_t, size_t, gfp_t);
void *krealloc(void *, size_t, gfp_t);
void  kfree(void *);
void *vmalloc(size_t);          void *vzalloc(size_t);
void  vfree(void *);
void *kvmalloc(size_t, gfp_t);  void *kvzalloc(size_t, gfp_t);
void  kvfree(void *);
```

The kmalloc backend wraps Kevlar's existing buddy-system heap
(`platform/global_allocator.rs`).  Linux's `kfree` doesn't take a
size argument, so the K2 shim pre-pends a 16-byte size header to
every allocation; `kfree` reads the header, reconstructs the
`Layout`, and calls the global allocator's `dealloc`.  The 16
bytes also keep the user-visible alignment at 16, which matches
GCC's `__BIGGEST_ALIGNMENT__` on aarch64.

`vmalloc` allocates page-multiples via `alloc_pages`.  K2's
vmalloc is *physically contiguous* — the buddy allocator gives
us multi-page contiguous chunks — which is a simplification
over Linux's vmap-stitched non-contiguous form.  The user-
visible API is the same: `vmalloc(size)` returns a kernel VA
you can use as a flat buffer.  K3+ may add real vmap if any
driver actually needs the non-contiguous semantics.

`gfp_t` flag bits are *ignored*.  Kevlar's heap is already IRQ-
safe via an interrupt-disabling spinlock, so `GFP_KERNEL` vs
`GFP_ATOMIC` collapses.  `__GFP_ZERO` is honored implicitly by
the `kzalloc`/`vzalloc` variants.

### wait_queue_head + wake_up

K2's `struct wait_queue_head` is opaque from the module side:

```c
struct wait_queue_head { void *_kevlar_inner; };
```

`init_waitqueue_head` heap-allocates a Kevlar `WaitQueue`
(`kernel/process/wait_queue.rs` — already lost-wakeup-safe and
signal-interruptible) and stashes the pointer in the shim.
`wake_up*` shims walk through that pointer to the real
primitive.

Linux's `wait_event(wq, condition)` macro expands into a loop
around `prepare_to_wait` / `schedule` / `condition` / `finish_wait`.
K2 doesn't try to duplicate that macro on the C side; it
provides a single shim, `kabi_wait_event(wq, cond_fn, arg)`,
that takes a condition callback evaluated each wakeup.  K3+ may
ship a header macro that expands to the Linux shape if any
real driver needs the exact macro semantics.

### completion

```c
struct completion { void *_kevlar_inner; };
```

A wait_queue + an `AtomicBool` flag.  `complete()` sets the flag
and wakes one sleeper; `complete_all()` wakes everyone;
`wait_for_completion` blocks until the flag is set.

### work_struct + workqueue

```c
struct work_struct {
    void *_kevlar_inner;
    void (*func)(struct work_struct *);
};
```

The `func` pointer is at the same offset as in Linux because
the `INIT_WORK(&w, my_handler)` expansion — and a future K3
reimplementation of that expansion in the K2 header — write
to it directly.

Kevlar already had `DeferredJob` (`kernel/deferred_job.rs`), but
those callbacks run in *interrupt context* and must not sleep.
Real Linux modules expect `schedule_work` to defer to a context
that *can* sleep — work handlers routinely call `msleep`,
allocate with `GFP_KERNEL`, etc.  K2 needed a real worker thread.

So K2 spawns a single dedicated kthread (`kabi_wq`, PID 2 on
the demo run above) whose body is just a drain loop:

```rust
loop {
    let work = wake_wq.sleep_until(WORKER_QUEUE.pop_front_or_none())?;
    work.func(work);
    work.pending = false;
    work.flush_wq.wake_all();
}
```

`schedule_work` enqueues + wakes; `flush_work` blocks on the
per-`work_struct` flush wait queue until the work transitions
from running back to idle.  `cancel_work_sync` removes from the
queue if pending or flushes if running.

K2 has one global FIFO worker.  Linux has per-CPU queues, named
workqueues, priorities, drain workqueues, freezable workqueues —
none of which are necessary for a `printk + msleep + wake_up`
demo.  K3+ when needed.

### scheduler shims

```c
void *kabi_current(void);   int kabi_current_pid(void);
void  kabi_current_comm(char *buf, size_t len);
void  msleep(uint32_t ms);  void schedule(void);
int   cond_resched(void);   int64_t schedule_timeout(int64_t ticks);
```

`kabi_current` returns an opaque `*mut Process` that the module
passes back to `kabi_current_pid()` / `kabi_current_comm()`.
Modules don't dereference the Process struct directly — that
needs Linux struct-layout faithfulness, which is K3 work.

`msleep` wraps the existing `kernel::timer::_sleep_ms`.
`schedule_timeout(ticks)` honors the timeout (Kevlar's
TICK_HZ = 100, so 1 jiffy = 10 ms) and always returns 0 — no
early-wake support yet.

### .modinfo

Linux modules embed metadata via `MODULE_LICENSE`, `MODULE_AUTHOR`,
etc., which expand to `__MODULE_INFO()` macros that emit
NUL-terminated `key=value` strings into the `.modinfo` section.
The K2 loader parses this and logs license, author, description
on the way in.  Future K3+ work uses the `depends=` field to
drive recursive module load and `vermagic=` to gate ABI
compatibility against the pinned Linux 7.0 surface.

## What's hardest about this milestone

Two things stand out.

### The kthread plumbing

Spawning a kernel thread that runs Rust code from scratch
required adding a `Process::new_kthread_with_entry(name, fn)`
factory.  Kevlar's existing `Process::new_idle_thread()` was
the closest template, but idle threads have `is_idle: true` —
the scheduler only runs them when nothing else is runnable.
A worker thread needs to run *in preference* to idle.

The arch side already had `ArchTask::new_kthread(ip, stack_top)`
plus a `kthread_entry:` assembly stub (in
`platform/arm64/usermode.S`) that pops the entry IP from the
stack and calls into it.  Both architectures had this, both
tagged `#[allow(unused)]` — written but never wired.  K1
didn't need them; K2 became their first caller.

The signature of `new_kthread(ip, stack_top)` had a wrinkle: it
also internally allocated a `kernel_stack`, but used the
caller-passed `stack_top` for the actual SP.  Bug — the
internal stack was leaked memory.  K2 fixed it to use the
internal allocation as the SP and dropped the second argument.

The plumbing looks like this when the worker spawns:

```
boot_kernel:
  process::init()           ← scheduler up, idle threads ready
  ...
  kabi::init()
    work::init()
      WORKER_WAKE.init(WaitQueue::new)
      Process::new_kthread_with_entry("kabi_wq", worker_thread_entry)
        alloc_pid()                    ← table lock, fresh PID
        ArchTask::new_kthread(IP)      ← stack alloc, push IP for kthread_entry
        Process { is_idle: false, ... }
        PROCESSES.insert + SCHEDULER.enqueue
```

When the BSP later calls `kabi_wait_event` (sleeping in
`init_module`), the scheduler picks the worker thread, which
runs `worker_thread_entry`, which pops a `work_struct` from the
queue and invokes its `func`.

### Ordering: when is the scheduler "up"?

K1 loaded `hello.ko` *before* `process::init()` because
`hello.ko` was synchronous — call `printk`, return.  No
scheduler interaction needed.

K2's `k2.ko` calls `schedule_work` which wakes a kthread, then
calls `kabi_wait_event` which sleeps the BSP.  That requires
both the kthread to exist (so spawn it first) and the scheduler
to be running (so `_sleep_ms` can switch away and find the
worker).

The first attempt put `kabi::init()` in the same boot location
as the `hello.ko` load — way before `process::init()` — and
panicked immediately on `Once::deref` when the kthread spawn
hit `INITIAL_ROOT_FS` (which `process::init` populates).

The fix: keep the K1 load early (it really doesn't depend on
anything), but defer the K2 init + load until after
`process::init()`.  Two boot stops for two demos with two
different requirement sets, both kept simple by not trying to
make the early one depend on the late one.

## Out of scope (still K3+)

- **Linux struct-layout faithfulness.**  K2's `wait_queue_head`,
  `completion`, `work_struct` are Kevlar-specific opaque
  shapes.  Real Linux `.ko` binaries embed these structs
  directly and read fields at fixed offsets — K3 makes the
  layouts match.
- **`struct device` / `struct driver` / `struct bus_type`.**
  The whole device-model spine.  Without it nothing PCI-,
  platform-, or DRM-shaped registers anything.
- **Module unload.**  K2 modules live forever; no
  `module_exit()` path, no `delete_module(2)`.
- **RCU.**  Nothing in K1-K2 needed it; first thing inside DRM
  that does will block until RCU stubs land.
- **W^X.**  Module pages are still RWX via the boot direct
  map.  Real Linux flips RW during reloc, RX after.
- **x86_64.**  K2 stays arm64-only.  Worker spawn, per-cpu
  accessors, and reloc handlers all need the x64 port —
  manageable once K3 stabilizes the kABI shape.

## Status

| Surface | Status |
|---|---|
| K1 — ELF .ko loader | ✅ |
| K2 — kmalloc / wait / work / completion / modinfo | ✅ |
| K2 demo: k2.ko exercises every primitive | ✅ |
| K3 — struct device, driver, bus_type, layout faithfulness | ⏳ next |
| K4-K9 | ⏳ |

## Cumulative kABI surface (K1 + K2)

```
printk
kmalloc kzalloc kcalloc krealloc kfree
vmalloc vzalloc vfree kvmalloc kvzalloc kvfree
init_waitqueue_head destroy_waitqueue_head
wake_up wake_up_all wake_up_interruptible wake_up_interruptible_all
kabi_wait_event
init_completion destroy_completion
complete complete_all wait_for_completion
kabi_init_work schedule_work flush_work cancel_work_sync
kabi_current kabi_current_pid kabi_current_comm
msleep schedule cond_resched schedule_timeout
```

~32 symbols.  Linear scan in the kABI lookup is still
microsecond-cheap; the binary-search switchover defers to K6+
when the count crosses ~1000.

## What K3 looks like

K3 is the structural milestone.  Two intertwined pieces:

**Layout faithfulness.**  Linux's `struct wait_queue_head` is
24 bytes (a `spinlock_t` + a `struct list_head`).  Linux's
`struct work_struct` is 32 bytes (a 64-bit data field, two
list_heads, a function pointer).  K3's K2-shim reimplementation
matches those exact byte-layouts so a `.ko` binary built
against the Linux 7.0 UAPI headers links and runs without
recompilation.

**Device model.**  `struct device`, `struct driver`,
`struct bus_type`, `struct kobject`, the `register_*` /
`unregister_*` family.  This is the spine that PCI, platform,
and DRM all hang their registration off.  Without it, nothing
truly driver-shaped registers anything.

K3's demo target: a module that calls `platform_driver_register`
+ `platform_device_register` and watches the bus drive their
`probe` against each other.  No I/O, no real device — just
the device-model bookkeeping firing end-to-end.

That's the milestone where Kevlar starts looking like a real
Linux replacement for driver code, instead of "a microkernel
that happens to also load .ko files."
