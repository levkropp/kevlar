## Blog 200: xfce4-panel doesn't show up until T+48

**Date:** 2026-04-21

After blog 199 landed the munmap TLB deadlock fix, the `test-xfce`
output looked like:

```
TEST_PASS xfwm4_running
TEST_FAIL xfce4_panel_running (panel not found)
TEST_PASS xfce4_session_running
TEST_END 3/4
```

The panel is always the hold-out.  I assumed this was a panel-specific
crash, either a kernel bug it triggers or a missing syscall.

It isn't either.

## The probe

`testing/test_xfce_panel_probe.c` is a variant of `test-xfce` that
launches the same D-Bus + Xorg + xfce4-session stack, then scans
`/proc/*/comm` every second for 120 s, recording when each Failsafe
client first appears.

Run output:

```
T+1   session=17 wm=-1  set=-1 conf=-1 panel=-1
T+2   session=17 wm=29  set=-1 conf=22 panel=-1   ← xfwm4 spawned
...
T+24  session=17 wm=29  set=-1 conf=22 panel=-1
T+25  session=17 wm=29  set=31 conf=22 panel=-1   ← xfsettingsd spawned
...
T+47  session=17 wm=29  set=31 conf=22 panel=-1
T+48  session=17 wm=29  set=31 conf=22 panel=35   ← xfce4-panel spawned
```

Three Failsafe clients spawn at T+2, T+25, T+48 — each exactly ~23 s
apart.  The XFCE Failsafe priority list is:

| priority | client        |
|---|---|
| 15 | xfwm4        |
| 20 | xfsettingsd  |
| 25 | xfce4-panel  |
| 30 | Thunar --daemon |
| 35 | xfdesktop    |

So Thunar will spawn around T+71 and xfdesktop around T+94.  The
15 s sleep in `test-xfce` checks at T+16 — panel is still 32 s away.

## Why 23 s?

`xfce4-session` (xfce-4.16 source, `session-manager.c:on_client_registered`)
launches each Failsafe client with `g_spawn_async`, then waits for the
client to "register" via the **X Session Management** protocol — an
ICE protocol handshake where the client connects back to the session
manager's Unix socket at `$SESSION_MANAGER` and sends a `SmcRegisterClient`
message.

If the client registers, the session manager launches the next
priority immediately.  If not, a ~30 s timeout fires and the manager
proceeds anyway.  23 s is close enough to the documented 30 s default
that this is almost certainly what we're hitting.

The xfce-session log confirms the fallback:

```
xfsettingsd: No window manager registered on screen 0.
(xfsettingsd:30): xfsettingsd-WARNING: Failed to get the
                  _NET_NUMBER_OF_DESKTOPS property.
```

xfsettingsd started before xfwm4 had declared itself via the EWMH
selection protocol.  Both are timing out on the same thing —
inter-client state.

## What's actually broken

Not the kernel.  Or at least not directly.  The XSM / ICE handshake
involves:

1. Client reads `$SESSION_MANAGER` env var for the manager's socket.
2. Client `connect()`s to that AF_UNIX socket.
3. Client sends an `IceOpenConnection` packet.
4. Manager replies with `IceAccept`.
5. Client sends `SmcRegisterClient` with a client ID.
6. Manager replies with `SmcRegisterClientReply`.

Some step is silent.  We know:

- Unix sockets work for basic D-Bus (dbus-daemon runs, clients
  connect).
- ICE sockets are at `/tmp/.ICE-unix/$pid` — Kevlar's tmpfs is
  mounted there.
- xfwm4 previously logged "ICE I/O Error — Disconnected from session
  manager" on some runs (blog 199 era), suggesting the channel is
  fragile.

Full ICE-protocol root-cause is a separate investigation — involves
tracing the handshake with strace-equivalent and comparing against
Linux.

## Why extending the test to 60 s made things worse

First guess: just wait 60 s, panel will be there by T+48, everything
works, 4/4.

Actual result over 5 runs at 60 s sleep:

| run | result |
|---|---|
| x1 | timeout |
| x2 | 1/4 (panel SIGSEGV at T+5) |
| x3 | timeout (dbus-daemon KERNEL_PTR_LEAK) |
| x4 | timeout |
| x5 | 1/4 |

The extended window doesn't reach a healthy 4/4.  What it reaches is
the *next* set of kernel bugs — the stale-TLB kernel-pointer leak
that blog 186 / blog 199 catalog, and a panel-specific SIGSEGV at
`0xa694032e8` (no VMA — wild pointer, probably an uninitialized
GObject field dereferenced because WM state wasn't ready).

So test-xfce at 15 s looks artificially optimistic (3/4 most runs),
and at 60 s looks artificially pessimistic (1/4 most runs).  Reality
is: *on a clean run, every component comes up eventually*, but the
kernel can't stay stable for the 48 s it takes to get there.

Reverting the sleep change for now.  The probe is committed as
`testing/test_xfce_panel_probe.c` for future investigation.

## Next steps

1. **Fix the ICE/XSM slow handshake.**  Trace what byte flows over
   the ICE socket and diff against Linux.  If a `read()` blocks too
   long, or a `connect()` returns the wrong error, that's where.
   Zero-to-one client registrations per second would cut panel
   start from T+48 to T+3.

2. **Track down the panel SIGSEGV at `0xa694032e8`.**  Likely
   correlates with xfwm4 not having taken the WM_S0 X selection
   yet — panel reads something from the window manager state that
   isn't there.  Fixing (1) probably fixes (2) as a side-effect.

3. **Widen the munmap TLB fix** (mprotect/mremap/madvise/vm.rs) —
   each has the same deadlock pattern but the straight-pattern
   rollout tripped a kernel page fault.  One-at-a-time.

4. **LXDE as validation DE** — once XFCE reaches reliable 4/4,
   test LXDE to confirm the fixes generalize.

The work on XFCE is now in "each next bug takes longer than the
last" territory.  Each fix reveals the next issue.  That's a good
sign of progress: the early bugs were ubiquitous (every syscall
deadlocked under broad sti), the current ones are specific (only
fire after 30 s of XFCE startup, only affect the ICE handshake).
