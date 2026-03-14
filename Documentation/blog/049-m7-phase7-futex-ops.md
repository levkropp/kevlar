# M7 Phase 7: Futex Operations

Phase 7 implements the three missing futex operations that glibc's
NPTL threading library requires: CMP_REQUEUE, WAKE_OP, and
WAIT_BITSET.

## Why these matter

glibc's pthread condvars use `FUTEX_CMP_REQUEUE` for
`pthread_cond_broadcast()` and `pthread_cond_signal()`.  Internal
glibc locks use `FUTEX_WAKE_OP`.  Timed waits with CLOCK_MONOTONIC
use `FUTEX_WAIT_BITSET`.  Without these, glibc-linked pthreads
programs deadlock or crash during initialization.

## Implementation

### FUTEX_CMP_REQUEUE (op 4)

The most complex operation.  Atomically: read `*uaddr1`, compare to
`val3` (return EAGAIN on mismatch), wake up to `val` waiters on
`uaddr1`, then move up to `val2` remaining waiters from `uaddr1`'s
queue to `uaddr2`'s queue without waking them.

This required adding `WaitQueue::requeue_to()` — a method that moves
waiters between queues under lock without calling `resume()`.

### FUTEX_WAKE_OP (op 5)

Encodes both an arithmetic operation and a comparison in `val3`:
- Bits 31-28: operation (SET, ADD, OR, ANDN, XOR)
- Bits 27-24: comparison (EQ, NE, LT, LE, GT, GE)
- Bits 23-12: operation argument
- Bits 11-0: comparison argument

Atomically reads the old value at `uaddr2`, applies the operation,
writes back.  Wakes up to `val` on `uaddr1`, and conditionally wakes
up to `val2` on `uaddr2` if the old value passes the comparison.

### FUTEX_WAIT_BITSET (op 9) / FUTEX_WAKE_BITSET (op 10)

Same as WAIT/WAKE but with a bitmask for selective wakeup.  Since we
don't yet need per-bitset filtering, these currently behave like
WAIT/WAKE.  The one semantic difference enforced: bitset=0 returns
EINVAL (matching Linux).

### WaitQueue additions

- `wake_n(max)` — wake up to `max` waiters, return count woken
- `requeue_to(other, max)` — move up to `max` waiters to another
  queue without waking, return count moved

The existing `_wake_one()` and `wake_all()` are unchanged.

## Contract test

The `futex_requeue.c` test verifies:

- CMP_REQUEUE with mismatched val3 returns EAGAIN
- CMP_REQUEUE with matching val3 and no waiters returns 0
- WAKE_OP applies SET operation and updates the target value
- WAIT_BITSET with value mismatch returns EAGAIN
- WAIT_BITSET with bitset=0 returns EINVAL
- WAKE with no waiters returns 0
- FUTEX_PRIVATE_FLAG is stripped correctly

## Results

26/26 contract tests pass, 14/14 threading tests pass on -smp 4
(including the condvar test which exercises CMP_REQUEUE).

## What's next

Phase 8 integrates everything: glibc hello-world, glibc pthreads,
and `ps aux` exercising the full /proc + glibc stack.
