# Blog 163: kxserver Phase 2 — Atoms, and a Musl HashMap Surprise

**Date:** 2026-04-14

## What Phase 2 was supposed to be

Two opcodes: `InternAtom (16)` and `GetAtomName (17)`.  Pre-seed the 68
predefined atoms from `Xatom.h` (PRIMARY=1, SECONDARY=2, …,
WM_TRANSIENT_FOR=68), grow an `AtomTable { by_id, by_name, next }` for
dynamically interned names starting at 69.  Three files, maybe 300
lines of Rust.  Success criterion: a raw client can intern predefined
names and get canonical ids back, intern new names and see id=69, 70,
…, and `GetAtomName` round-trips all of them.

I wrote `src/atom.rs`, a new `src/state.rs` to hold server-wide
`ServerState { atoms: AtomTable }`, and threaded `&mut ServerState`
through the dispatch chain so handlers could see the table while still
borrowing `&mut Client` disjointly.  The borrow checker needed the
split: iterating `self.clients` and accessing `self.state` has to be
expressible as two disjoint-field borrows on `Server`.  `pump(&mut
Client, &mut ServerState)` got updated, and `dispatch::dispatch_request`
too.

Two new handlers:

```rust
// Opcode 16: InternAtom
//   bytes 0..=1: opcode, only_if_exists flag
//   bytes 2..4 : request length (in words)
//   bytes 4..6 : name length (n)
//   bytes 8..  : STRING8 name + pad4(n)
// Reply (32 bytes):
//   byte 0    : 1 (Reply)
//   bytes 2..4: sequence number
//   bytes 4..8: extra length = 0
//   bytes 8..12: atom id (0 = None if only_if_exists and not found)
```

```rust
// Opcode 17: GetAtomName
//   bytes 4..8: atom id
// Reply (32 + padded name bytes):
//   byte 0    : 1 (Reply)
//   bytes 2..4: sequence
//   bytes 4..8: extra length = pad4(n)/4
//   bytes 8..10: name length
//   bytes 32.. : STRING8 + pad
```

Unit tests for the atom table (predefined ids stable, intern round-trip,
only-if-exists, monotonic dynamic ids) and byte-level smoke tests
through the dispatcher on native Linux all passed the first time.

## The musl surprise

I pointed a raw Python client at the server to validate the
request/reply layouts end to end.  First run:

```
/bin/bash: segmentation fault (core dumped)
./target/x86_64-unknown-linux-musl/release/kxserver :7 --log=req
```

Nothing in the log file.  Nothing on stderr.  Zero output, no error
message, instant SIGSEGV.  Phase 1 had been working fine ten minutes
before; nothing I'd added should even run until after `main()`.

I sprinkled `eprintln!` markers through `main()`.  None of them fired.
The crash was happening *before* `main()`.  I switched to raw
`libc::write(2, b"M1\n", 3)` to bypass the Rust stdio buffering layer.
Still nothing.

`strace` narrowed it down to a SIGSEGV during early musl startup:

```
execve(...)                              = 0
arch_prctl(ARCH_SET_FS, ...)             = 0
set_tid_address(...)                     = <pid>
brk(NULL)                                = 0x5593...
mmap(...)                                = 0x5593...
mprotect(0x5592..., 12288, PROT_READ)    = 0
--- SIGSEGV {si_code=SI_KERNEL, si_addr=NULL} ---
```

`gdb` confirmed the crash frame:

```
Program received signal SIGSEGV
#0  0x000055555555ff46 in _start_c ()
#1  0x000055555555fde1 in _start ()
```

`_start_c` is musl's C entry point — it parses argc/argv/environ/auxv
and calls `__libc_start_main`, which runs the `.init_array`
constructors before calling `main()`.  The crash was inside that
window.  Phase 0 and Phase 1 had worked fine.  All I'd added was an
`AtomTable` wrapping a `Vec<Option<String>>` and a
`std::collections::HashMap<String, u32>`, plus a one-line `ServerState`
wrapper.  Nothing that touched static initialization.

Rebuilt with the `x86_64-unknown-linux-gnu` target instead of
`x86_64-unknown-linux-musl`: **worked perfectly**, every diagnostic
marker fired, server listened, handshake passed.

Swapped the plain `rustc` hello world for musl: worked.  Wrote a second
hello that `use libc`: also worked.  So a trivial musl binary with the
libc crate was fine.  kxserver's musl binary was fundamentally broken
in its pre-`main` init.

Comparing `readelf -l` on the trivial binary and kxserver, the only
relevant difference was the size of the `TLS` program header
(MemSiz 0x50 vs 0x68 — kxserver's was 0x18 bytes larger).
Static-musl + TLS + Rust's std has a well-known bad interaction path:
`HashMap::new()` uses `RandomState::new()` which pulls in a
thread-local RNG cache, and that cache has to be initialized somewhere
in the init_array.  On some static-musl configurations that init
touches a not-yet-set-up structure and segfaults before `main`.

The fix was one word:

```rust
// HashMap on static musl pulls in thread-local RandomState init
// that crashes before `main` on this toolchain.  BTreeMap has no
// RNG state, costs O(log N) instead of O(1), and N is bounded by
// the number of unique atom names a session interns (typically <200).
use std::collections::BTreeMap;
```

With `HashMap<String, u32>` replaced by `BTreeMap<String, u32>` the
binary boots cleanly.  The atom table's hot path is a Vec index for
`name(id)` (O(1), unchanged) and a BTreeMap lookup for `intern(name)`
(O(log N)) — for the ~200 atoms a real X session interns, that's 8
comparisons, not something we'll ever notice.

Lesson for everyone writing static-musl Rust userspace: default to
`BTreeMap` for anything that lives on the hot startup path.  `HashMap`
works most of the time, but when it doesn't, the failure is in
`_start_c` with no stack trace and no way to print anything.

## End-to-end validation

On native Linux, raw Python X11 client:

```
[handshake ok]
PRIMARY         = 1
WM_NAME         = 39
WM_CLASS        = 67
WM_TRANSIENT_FOR= 68
UTF8_STRING     = 69
UTF8_STRING 2nd = 69
_NET_WM_NAME    = 70
NONEXIST(only)  = 0
name(1)         = 'PRIMARY'
name(39)        = 'WM_NAME'
name(69)        = 'UTF8_STRING'
name(70)        = '_NET_WM_NAME'
name(9999)      = None   (BadAtom error)
```

Every request and its reply logged in the expected format:

```
[C1 #0007] REP ok InternAtom name="_NET_WM_NAME" only_if_exists=false → atom=70
[C1 #0008] REQ op=16  InternAtom           len=16 data=0x01
[C1 #0008] REP ok InternAtom name="NONEXIST" only_if_exists=true → atom=0
[C1 #0013] REQ op=17  GetAtomName          len=8 data=0x00
[C1 #0013] ERR code=5 bad=0x0000270f GetAtomName unknown atom
```

`opcode_expects_reply` already listed 16 and 17, so those now route
to our handlers; everything else still gets BadImplementation as
Phase 1 established.

On Kevlar via `make test-kxserver`: **6/6 tests pass** — same Phase 1
harness, no changes needed.  The raw handshake + server lifecycle
still works.  The binary grew 12 KB (455K → 468K) to hold the atom
table code and the 68 predefined name strings.

## What's next

Phase 3 is the big one: windows, tree, properties, events.  `CreateWindow`,
`MapWindow`, `ChangeProperty`, `GetProperty`, `ConfigureWindow`,
`QueryTree`, `DestroyWindow`, `GetGeometry`, …, plus the first real
events: `CreateNotify`, `MapNotify`, `ConfigureNotify`, `Expose`,
`PropertyNotify`.  Success criterion: `xwininfo -display :1 -root`
reports sane root window geometry, and a hand-rolled C test client can
create a window, map it, set the `WM_NAME` property, and receive its
first `Expose` event.

Phase 3 will also introduce the resource system: per-client XID base
allocation, a `Resource` enum with variants for `Window`, `Pixmap`,
`Gc`, etc., and per-client disconnect cleanup.  That resource
infrastructure is the backbone every later phase will hang off.
