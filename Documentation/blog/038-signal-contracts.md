# M6.5 Phase 4: Signal Contracts

Phase 4 validates Linux signal delivery contracts: handler registration,
signal masking, delivery order, and coalescing.

---

## Tests implemented

**delivery_order** — Sends SIGUSR1 to self 5 times while masked.  After
unmasking, verifies the handler ran exactly once (standard signal
coalescing).  This confirms that Kevlar's signal pending bitmask
correctly coalesces multiple sends of the same standard signal.

**handler_context** — Registers a SIGUSR2 handler via `sigaction()`,
sends the signal, verifies the handler ran with the correct signal
number.  Also tests that replacing a handler returns the old one,
and that `SIG_IGN` suppresses delivery.

**mask_semantics** — Already passing from Phase 1.  Tests `sigprocmask`
block/unblock with pending signal delivery after unmasking.

**sa_restart** — Existing test, requires `setitimer`/`SIGALRM` to deliver
a signal during a blocking `read()`.  Kevlar's `alarm()` is stubbed,
so this test timeouts.  Tracked for M7.

## Known gaps

- **SA_RESTART**: Requires `alarm()` or `setitimer()` to deliver SIGALRM
  during a blocking syscall.  Currently stubbed.
- **Coredumps**: Not implemented (M9 scope).
- **Real-time signals**: `sigqueue()` not tested.  Standard signal
  coalescing works; real-time queueing is untested.
- **Signal during syscall**: The interaction between signal delivery
  and in-progress syscalls (EINTR vs SA_RESTART) is not validated yet.

## Results

| Test | Status |
|------|--------|
| signals.delivery_order | PASS |
| signals.handler_context | PASS |
| signals.mask_semantics | PASS |
| signals.sa_restart | TIMEOUT (needs alarm) |
| **Total** | **3/4 PASS** |

Full suite: **15/16 PASS**.
