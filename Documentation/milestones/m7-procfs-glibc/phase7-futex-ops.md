# Phase 7: Futex Operations

**Duration:** ~2 days
**Prerequisite:** Phase 6 (stubs must be in place first)
**Goal:** Implement FUTEX_CMP_REQUEUE, FUTEX_WAKE_OP, FUTEX_WAIT_BITSET.

## Why this is the hardest phase

Futex operations are the synchronization primitive beneath every mutex,
condvar, and barrier in glibc's NPTL.  Getting the semantics wrong causes
deadlocks, lost wakeups, or data corruption — all of which are
non-deterministic and hard to debug.

`FUTEX_CMP_REQUEUE` alone has caused kernel bugs in Linux.  The
operation must be atomic: check a value, wake some waiters, and move
others to a different queue — all while holding the right locks in the
right order.

## Operations to implement

### 1. FUTEX_CMP_REQUEUE (op 4)

Used by: `pthread_cond_broadcast()`, `pthread_cond_signal()`

Semantics:
```
futex(uaddr1, FUTEX_CMP_REQUEUE, val, val2, uaddr2, val3)
```
1. Read `*uaddr1`.  If != `val3`, return `-EAGAIN`
2. Wake up to `val` waiters on `uaddr1`
3. Requeue up to `val2` waiters from `uaddr1` to `uaddr2`
4. Return number of woken + requeued waiters

**Critical detail:** The check-and-requeue must be atomic.  Hold the
wait queue lock for uaddr1 during the entire operation.  The requeue
moves WaitQueue entries from one queue to another without waking them —
they'll be woken when someone does FUTEX_WAKE on uaddr2.

```rust
pub fn futex_cmp_requeue(
    &self,
    uaddr1: UserVAddr,
    val: i32,      // max to wake
    val2: usize,   // max to requeue
    uaddr2: UserVAddr,
    val3: i32,     // expected value at uaddr1
) -> Result<isize> {
    let current_val = uaddr1.read::<i32>()?;
    if current_val != val3 {
        return Err(Errno::EAGAIN.into());
    }

    let mut woken = 0;
    let mut requeued = 0;

    // Wake up to val waiters on uaddr1
    // Requeue up to val2 remaining waiters to uaddr2
    // ... (WaitQueue manipulation under lock)

    Ok((woken + requeued) as isize)
}
```

### 2. FUTEX_WAKE_OP (op 5)

Used by: glibc's internal lock implementation

Semantics:
```
futex(uaddr1, FUTEX_WAKE_OP, val, val2, uaddr2, val3)
```
1. Atomically read old value at `uaddr2`, apply operation (encoded in
   `val3`), write new value
2. Wake up to `val` waiters on `uaddr1`
3. If the old value at `uaddr2` passes a comparison test (also encoded
   in `val3`), wake up to `val2` waiters on `uaddr2`

The `val3` encoding:
- Bits 31-28: operation (SET, ADD, OR, ANDN, XOR)
- Bits 27-24: comparison (EQ, NE, LT, LE, GT, GE)
- Bits 23-12: operation argument
- Bits 11-0: comparison argument

```rust
fn decode_futex_op(val3: u32) -> (Op, Cmp, u32, u32) {
    let op = (val3 >> 28) & 0xF;
    let cmp = (val3 >> 24) & 0xF;
    let oparg = (val3 >> 12) & 0xFFF;
    let cmparg = val3 & 0xFFF;
    (op, cmp, oparg, cmparg)
}
```

### 3. FUTEX_WAIT_BITSET (op 9)

Used by: `pthread_cond_timedwait()` with CLOCK_MONOTONIC

Semantics: Same as FUTEX_WAIT but with a bitmask for selective wakeup.
`val3` is the bitset.  FUTEX_WAKE_BITSET wakes only waiters whose
bitset has overlapping bits.

For initial implementation: treat bitset=0xFFFFFFFF (match all) as
equivalent to FUTEX_WAIT.  This covers the common glibc case.

```rust
FUTEX_WAIT_BITSET => {
    // bitset in val3, typically 0xFFFFFFFF
    // For now, ignore bitset and behave like FUTEX_WAIT
    self.futex_wait(uaddr, val, timeout)
}
```

### 4. FUTEX_PRIVATE_FLAG (128)

glibc uses `FUTEX_WAIT_PRIVATE` (0 | 128) and `FUTEX_WAKE_PRIVATE`
(1 | 128).  The PRIVATE flag means the futex is process-local (no
cross-process sharing).  Since we don't support cross-process futexes
anyway, just mask off the flag:

```rust
let op = raw_op & !FUTEX_PRIVATE_FLAG;  // strip private bit
```

## WaitQueue changes

Current WaitQueue only supports wake.  Need to add:
- `requeue_to(&self, other: &WaitQueue, max: usize) -> usize`
  — Move up to `max` waiters from `self` to `other` without waking

## Testing

Contract test: `testing/contracts/scheduling/futex_requeue.c`
```c
// Two threads: producer signals condvar, consumer waits
// This exercises FUTEX_CMP_REQUEUE under the hood
// Verify no deadlock and correct wakeup count
```

Run with `-smp 4` to catch race conditions.

## Success criteria

- [ ] FUTEX_CMP_REQUEUE works (condvar broadcast/signal)
- [ ] FUTEX_WAKE_OP works (glibc internal locks)
- [ ] FUTEX_WAIT_BITSET works (timedwait with CLOCK_MONOTONIC)
- [ ] FUTEX_PRIVATE_FLAG stripped correctly
- [ ] No deadlocks under -smp 4
- [ ] glibc pthreads condvar test passes
