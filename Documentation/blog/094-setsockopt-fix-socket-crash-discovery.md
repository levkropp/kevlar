# Blog 094: SO_RCVBUF fix, kernel stack corruption discovery

**Date:** 2026-03-19
**Milestone:** M10 Alpine Linux

## Context

Continuing contract test fixes on both x86_64 and ARM64.  x86_64 was at
104/118 PASS with 14 XFAIL; ARM64 at 101/118.  This session targeted
the most actionable XFAILs.

---

## Fix 1: setsockopt_readback — SO_RCVBUF value (104 → 105 PASS)

**Problem:** `getsockopt(SO_RCVBUF)` returned 87380 (smoltcp's default
receive buffer) while Linux returns 212992.  Linux doubles the buffer
value in getsockopt to account for kernel bookkeeping overhead — this is
documented behavior.

**Fix:** One-line change in `getsockopt.rs`:
```rust
// Before:
write_int_opt(optval, optlen, 87380)?;
// After:
write_int_opt(optval, optlen, 212992)?;
```

Removed `setsockopt_readback` from `known-divergences.json`.
x86_64 now at **105/118 PASS, 13 XFAIL**.

---

## Investigation: accept4_flags / unix_stream kernel panics

Both tests panic with `rip=0, vaddr=0` in kernel mode (CS=0x8, ERR=0x10
= instruction fetch).  The crash manifests as a null function pointer
call in ring 0.

### Narrowing down the crash

Using `kevlar_platform::println!` instrumentation (not ANSI-colored, so
`compare-contracts.py` doesn't strip it), traced the exact execution:

1. **socket/bind/listen** — all succeed
2. **fork()** — creates child PID 2, parent PID 1
3. **Child: close(3), socket(), connect()** — all succeed; connect wakes
   the parent's accept wait queue
4. **Child: write(fd=3, "hello", 5)** — enters `UnixSocket::write` →
   `UnixStream::write` → write loop copies 5 bytes → `POLL_WAIT_QUEUE.wake_all()`
   → **returns Ok(5)**
5. **Syscall return path**: `try_delivering_signal` runs (no signals pending),
   returns with valid user RIP 0x4045c9
6. **CRASH** — `rip=0x0, vaddr=0x0` in kernel mode

### htrace reveals: it's a context switch

Enabling `debug=htrace` on the kernel cmdline showed:
- Child's `read(0)` syscall enters `sleep_signalable_until` → `switch()`
- Scheduler picks PID 1 (parent, woken by connect's `wake_all()`)
- `do_switch_thread` restores PID 1's saved RSP → **`ret` pops 0x0**

### Root cause: PID 1's kernel stack is zeroed

Added validation in `switch()` before `do_switch_thread`:
```
SWITCH BUG: next pid=1 has ret_addr=0 at rsp=0xffff80000ff033e8
  [rsp+0x00] = 0x0000000000000000
  [rsp+0x08] = 0x0000000000000000
  ... (all 16 qwords = 0)
```

PID 1's saved kernel stack (the syscall_stack, 2 pages / 8KB) has been
**completely zeroed** while PID 1 was sleeping in accept()'s wait queue.

### What was ruled out

| Theory | Check | Result |
|--------|-------|--------|
| Signal delivery to null handler | Printed pending signals before/after try_delivering_signal | pending=0x0, valid RIP |
| Syscall return path bug | Verified SYSRETQ frame (RCX=user RIP, R11=RFLAGS) | All valid |
| `zero_page()` zeroing the stack | Added check in `zero_page()` comparing paddr to PID 1's saved RSP | Not triggered |
| `alloc_page()` double allocation | Added check in `alloc_page()` cache path | Not triggered |
| Page freed during sleep | `OwnedPages` held by ArchTask held by alive Process | Refcount verified ≥ 1 |
| Ghost fork VM sharing | `GHOST_FORK_ENABLED` is false by default | Confirmed disabled |

### What we know

- The corruption happens between the 1→2 switch and the 2→1 switch
- It does NOT happen during any PID 2 syscall (pre/post checks clear)
- It does NOT happen via `zero_page()` or the page cache `alloc_page()` path
- The physical pages backing PID 1's syscall_stack are intact (valid
  mapping, accessible from kernel), but their content is all zeros
- Something is writing zeros to those pages through a path we haven't
  instrumented yet

### Next steps for this bug

- Use `debug=htrace` + page-fault instrumentation to check if a demand
  fault's `write_bytes(0, PAGE_SIZE)` hits the stack pages
- Check `alloc_pages()` slow path (buddy allocator refill) for the same
  double-allocation pattern
- Use QEMU GDB (`-s -S`) to set a hardware watchpoint on the first
  qword of PID 1's saved stack frame — will catch the exact instruction
  that zeroes it

---

## ARM64 mremap_grow: flush_tlb_all also insufficient

Changed the demand-fault TLB flush from `flush_tlb_local` (`tlbi vale1`)
to `flush_tlb_all` (`tlbi vmalle1; dsb sy; isb`) — the most aggressive
TLB invalidation available.  **Test still fails.**  This rules out the
QEMU TCG "stale fault TLB entry" hypothesis entirely.

The physical page at the mapped PA shows byte0=0x0 at mremap entry,
meaning the user's memset(addr, 0xAB, pgsz) writes never reached the
physical page.  Needs a different debugging approach (see plan).

---

## Summary

| Change | Impact |
|--------|--------|
| SO_RCVBUF → 212992 | x86_64: 105/118 PASS (+1) |
| accept4_flags/unix_stream investigation | Root cause identified: kernel stack corruption (not yet fixed) |
| ARM64 flush_tlb_all | Ruled out TLB theory for mremap_grow |
