# Blog 164: kxserver Phase 3 — Windows, Properties, Events, and Non-PIE on Kevlar

**Date:** 2026-04-15

## What Phase 3 shipped

Fifteen new opcodes across three categories, plus the event subsystem
that makes X clients usable:

**Window management (1–15):**
- `CreateWindow`, `ChangeWindowAttributes`, `GetWindowAttributes`,
  `DestroyWindow`, `DestroySubwindows`
- `MapWindow`, `MapSubwindows`, `UnmapWindow`, `ConfigureWindow`
- `GetGeometry`, `QueryTree`

**Properties (18–21):**
- `ChangeProperty`, `DeleteProperty`, `GetProperty`, `ListProperties`

**Events — generated and delivered:**
- `CreateNotify(16)`, `DestroyNotify(17)`, `UnmapNotify(18)`,
  `MapNotify(19)`, `ConfigureNotify(22)`, `Expose(12)`, `PropertyNotify(28)`

The kxserver source grew by six new modules:
`resources.rs` (XID map + `Resource` enum), `window.rs` (tree, geometry,
attributes, per-client event-mask listeners), `property.rs`
(name/type/format/data), `event.rs` (32-byte wire builders for every
notify variant), and `state.rs` (grew to hold the `ResourceMap` and
create the root window). `dispatch.rs` expanded from ~170 to ~700
lines with one handler per opcode.

The success criterion — a raw client creates + maps + titles + tears
down a window and receives all the expected events — landed in a
12-step Python smoke test:

```
[handshake ok]
GetGeometry root: 1024x768
sent CreateWindow 36 bytes
GetGeometry child: 320x240+100+50
QueryTree: n=1 child=0x200001
ev code=19 (expect 19 MapNotify)
ev code=12 (expect 12 Expose)
ev code=28 (expect 28 PropertyNotify)
GetProperty → 'test window'
ListProperties n=1 atom=39
ev code=18 (expect 18 UnmapNotify)
ev code=17 (expect 17 DestroyNotify)
final QueryTree: 0
ALL PHASE 3 TESTS PASSED
```

Every reply parses, every event code matches, every property round-trips.
On native Linux this took one pass once I remembered that the test
client has to `SelectInput(PROPERTY_CHANGE)` before it can see
`PropertyNotify` events for its own ChangeProperty calls — that's the
X11 spec, not a bug, but I forgot and stared at the first failing
assertion for a minute.

## The Kevlar ELF relocation landmine

Phase 3 worked first try on native Linux.  On Kevlar it crashed.
Every invocation:

```
TEST_FAIL kxserver_help (Segmentation fault
USER FAULT: GENERAL_PROTECTION_FAULT pid=3 ip=0xa0000a883
```

That's a userspace GP fault at IP `0xa0000a883` — deep in kxserver's
own `.text`.  None of my `eprintln!("[kxserver] main entered")` markers
fired.  The crash was happening before `main()`, during static
initialization.

Two things made this confusing:

1. **Phase 2 worked on Kevlar**; only Phase 3 broke it.  So it wasn't a
   fundamental Kevlar-musl incompatibility.
2. **Phase 3 worked on my host**.  Same toolchain, same binary, same
   everything.

`objdump -d` at the faulting IP showed code inside Rust's unwinder /
frame-info runtime:

```
94d0: 48 8b 84 24 a0 01 00 00  mov 0x1a0(%rsp),%rax
94d8: 4c 89 1c 11              mov %r11, (%rcx,%rdx,1)      <-- fault here
```

A write to `(rcx + rdx)` where those registers came from walking the
`.eh_frame_hdr` table.  This is `__register_frame_info` doing PC-offset
fixups on the frame-description table at program startup.

The FDE fixup loop walks the `.eh_frame` section, finds each frame's
`initial_location` encoding, and — if the binary is PIE — rewrites it
to an absolute runtime address.  On a normal Linux host, `ld-musl`
applies the PIE relocations for `.eh_frame` at load time so the loop
walks valid data and the writes land in a writable mapping.  Kevlar's
ELF loader maps PIE segments at a hard-coded `0xa00000000` base but
does not apply all of the R_X86_64_RELATIVE relocations the way musl's
dynamic loader does on Linux.  Or, more likely, it does apply them but
the static musl runtime's `__register_frame_info` pass also rewrites
them *a second time* assuming they're still offsets, and the double
relocation runs off the end of the section into a read-only page.

Phase 2's binary was just below the threshold where the `.eh_frame`
table got large enough for the loop to actually walk into the broken
region.  Phase 3 added enough non-trivial functions to push it past.

I didn't want to chase this into Kevlar's ELF loader in the middle of a
kxserver phase, so I took the pragmatic shortcut: **build kxserver as
non-PIE**.

```toml
# tools/kxserver/.cargo/config.toml
[target.x86_64-unknown-linux-musl]
linker = "x86_64-linux-musl-gcc"
rustflags = [
    "-C", "target-feature=+crt-static",
    "-C", "relocation-model=static",
    "-C", "link-arg=-no-pie",
]
```

A non-PIE binary loads at a fixed address, has no `R_X86_64_RELATIVE`
relocations to apply, and its `.eh_frame` fixups are no-ops (or missing
entirely).  The binary reports as:

```
ELF 64-bit LSB executable, x86-64, version 1 (SYSV), statically linked, stripped
```

`statically linked` (no interpreter!) instead of `pie executable, dynamically
linked, interpreter /lib/ld-musl-x86_64.so.1`.  No musl `ld` at all —
the binary talks directly to the kernel through the `syscall` instruction.

Kevlar's ELF loader handles non-PIE ET_EXEC cleanly (this is the same
path it uses for Xorg, busybox, xterm), so one line of config fixed
the GP fault.

## One more Makefile gotcha

After adding the non-PIE rustflags, my local `cargo build --release`
produced the right binary.  `make kxserver-bin` still produced a PIE
binary.

The Kevlar top-level `Makefile` does:

```makefile
export RUSTFLAGS = -Z emit-stack-sizes
```

That line applies to the kernel build and propagates to every child
process — including `cd tools/kxserver && cargo build --release`.
When cargo sees `RUSTFLAGS` in the environment it *replaces*, not
merges, whatever is in `.cargo/config.toml`.  So my non-PIE flags
vanished whenever `make` invoked cargo for kxserver.

The fix is a one-line kxserver-bin recipe tweak:

```makefile
.PHONY: kxserver-bin
kxserver-bin:
    $(PROGRESS) "CARGO" "kxserver (static musl)"
    cd tools/kxserver && env -u RUSTFLAGS cargo build --release
```

`env -u RUSTFLAGS` clears the variable for the cargo subprocess so the
.cargo/config.toml rustflags can actually take effect.  Documented in
the Makefile target with a comment explaining why.

## Architectural choices worth noting

**Resource IDs**: the setup reply already advertises `resource-id-base =
client_id << 21` and `resource-id-mask = 0x1FFFFF`.  Phase 3 finally
uses that — a `belongs_to_client` helper validates that any XID sent in
a request has its top bits matching the sender's client id.  XID 0x20
is the root window, which is in the server's own id range (below any
client), so both clients can reference it but neither can allocate
over it.

**Window tree**: children are a `Vec<u32>` of XIDs per window, not
`Vec<Arc<Window>>` or `Vec<Box<Window>>`.  The tree is expressed
entirely through the flat `ResourceMap: BTreeMap<XID, Resource>`,
which gives us O(log N) lookup, zero pointer cycles, and a clean
reap-on-disconnect story via `ResourceMap::reap_client(owner)`.

**Event queue**: each `Client` carries a `Vec<[u8; 32]>` event queue.
Handlers push events onto the current client's queue (or would, if we
had cross-client delivery yet).  After each dispatched request, the
`pump()` loop calls `c.flush_events()` which drains the queue into the
outgoing wire buffer.  Events always carry the *sequence number of the
last request processed*, not the opcode that generated them — the
`Client::queue_event` helper stamps that in bytes 2..=3 at enqueue
time.

**Listener mechanism**: `ChangeWindowAttributes`'s `CWEventMask` bit
(bit 11) maps to a `Window::set_listener(client, mask)` call, storing
the (client, mask) pair in `Window::listeners: Vec<Listener>`.  Event
delivery walks the listener list for each relevant window and queues
the event to every listener whose mask matches the required bit.
Multiple clients may listen on the same window with different masks;
they each get their own copy of matching events.

**Cross-client delivery is a Phase 8 problem**: the current
`deliver_to_structure` / `deliver_to_substructure` helpers only queue
events to the *currently dispatching* client.  If listener C is
different from the dispatching client, the event is dropped with a
warning.  This is fine for the kxserver test cases (they SelectInput on
their own windows) and for `xwininfo`-style introspection (no events
needed).  Real twm support requires cross-client event routing, which
needs a server-wide client registry.  That lands in Phase 8.

## Tests

| Suite | Before Phase 3 | After Phase 3 |
|---|---|---|
| `test-kxserver` (Kevlar) | 6/6 | **6/6** |
| Native Python Phase 3 smoke test | N/A | **12/12 opcodes exercised, every event matches** |
| `test-threads-smp` | 14/14 | **14/14** |
| `test-contracts` | 157–158/159 | **157–158/159** (same 1 pre-existing FAIL + 1 flaky DIVERGE) |

Zero regressions.  The non-PIE build + `env -u RUSTFLAGS` combo is
permanent — Phase 4 and beyond will inherit it automatically.

## What's next

Phase 4 is framebuffer rendering: open `/dev/fb0`, mmap the Bochs
VGA backing store, and implement `CreateGC`, `ChangeGC`, `AllocColor`,
`PolyFillRectangle`, `PolyRectangle`, `ClearArea`, `CopyArea`,
`PolyLine`, `PolyPoint`.  Success criterion: the raw client from
Phase 3 fills its window with a solid color and we see it on the
QEMU display.

Phase 4 is also where the log output starts being actually useful for
chasing visual bugs — every draw op logs the window, the absolute
screen coordinates, and the pixel value, so if something renders
in the wrong place or the wrong color, one grep tells us which
`PolyFillRectangle` caused it.
