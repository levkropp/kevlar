# Blog 161: kxserver Phase 0 — A Rust X11 Server for Diagnostics

**Date:** 2026-04-14

## Why build our own X server

Kevlar already runs the real Xorg 21.1.16 out of an Alpine ext2 image
(blog 137). It boots, xterm draws, twm reparents, and the
framebuffer pipeline (Bochs VGA → `/dev/fb0` → xf86-video-fbdev) works
end-to-end. But visual bugs keep cropping up in ways that are hard to
chase through Xorg's 200k-LOC C codebase:

- glyphs mis-positioned or missing (blog 158)
- blank windows and stale content
- 50+ second font-loading bottleneck (blog 144)

Every time we hit one, we end up bisecting Xorg's extension probing or
digging through xfont2 caches.  The underlying X protocol is actually
quite small and well-specified — it's the Xorg *implementation* that's
opaque.

So we're building our own.  **kxserver** is a minimal X11 display
server written in Rust from the wire protocol up.  It is explicitly
**diagnostic infrastructure, not production** — we will never ship it.
The payoff is visibility: when xterm draws wrong, we can read a single
line in our own log and trace it back to a specific Rust function.
Every request, reply, event, and error flows through one module we
wrote ourselves.

The target is twm + xterm running on top of kxserver, installed as a
static musl binary at `/usr/bin/kxserver` in the Alpine image.  We
will not implement RENDER, RANDR, XInput2, XKB, GLX, MIT-SHM, Composite,
XFIXES, DAMAGE, or SHAPE — all return "unknown extension" from
QueryExtension, which forces xterm to the core X11 font path and
bypasses the xft loading bottleneck entirely.

## Phase 0: scaffolding

Phase 0 is just "prove the build pipeline end to end" — no X protocol
yet.  The goal is that `make test-kxserver` boots Kevlar, runs a test
harness, and sees the kxserver binary print its startup banner on
serial.  That's it.  The value of Phase 0 is that every later phase can
focus on the protocol, not on toolchain plumbing.

### A separate Cargo workspace

The main Kevlar workspace at `/home/fw/kevlar/Cargo.toml` is configured
for kernel-space only — no-SSE, custom target spec (`x64.json`),
`panic=abort`, soft-float.  A userspace musl binary cannot live in it.

kxserver therefore has its own independent Cargo workspace at
`tools/kxserver/`, mirroring how `tools/ktrace/` is structured.  It has
its own `Cargo.toml`, its own `Cargo.lock`, and its own `target/`
directory that never touches the main build.  The toolchain is still
nightly (inherited from the parent `rust-toolchain.toml` — we just need
the nightly-musl target installed once via `rustup target add
x86_64-unknown-linux-musl`).

The `tools/kxserver/.cargo/config.toml` sets the default target and
linker:

```toml
[build]
target = "x86_64-unknown-linux-musl"

[target.x86_64-unknown-linux-musl]
linker = "x86_64-linux-musl-gcc"
rustflags = ["-C", "target-feature=+crt-static"]
```

After `cargo build --release`, the result is a 409 KB statically-linked
ELF.  `ldd` reports "statically linked"; the binary runs on any x86_64
Linux without a dynamic loader.

### The log module, day-one foundation

The plan (blog 160's follow-on) calls out one thing above all: the
log module must exist from the start, because the whole point of the
project is visibility.  Every later phase will pump output through it
and add new kinds of entries.

`src/log.rs` implements:

```rust
pub enum Sev { Trace, Req, Rep, Evt, Err, Warn, Fatal }

pub struct OpSet([u64; 4]);   // 256-bit bitset indexed by X11 opcode

pub struct Filter {
    pub min_sev: Sev,
    pub opcodes: OpSet,
    pub clients: Option<Vec<u32>>,
}

pub fn req(client: u32, seq: u16, opcode: u8, name: &str,
           decoded: impl Display, raw: &[u8]);
pub fn rep(client: u32, seq: u16, decoded: impl Display, raw: &[u8]);
pub fn evt(client: u32, evtype: u8, name: &str, target: u32,
           decoded: impl Display);
pub fn err(client: u32, seq: u16, code: u8, bad: u32, reason: &str);
```

Output is one line per entry, greppable, with a consistent prefix:

```
[C1 #0042] REQ  op=53 CreatePixmap    pid=0x200000 drw=0x100 w=80 h=24
[C1 #0042] RAW  0000  35 04 00 00 00 20 00 00 00 01 00 00 50 00 18 00
[C1 #0042] REP  ok
```

`[C1 #0042]` = client 1, request sequence 0x0042.  The severity tag
(`REQ`/`REP`/`EVT`/`ERR`/`TRC`) is next.  At trace severity, every
request also emits a `RAW` hex-dump line so we can diff against
`xtrace` captures from a reference Linux host during later phases.

The `--log=trace,op=53,55,client=1` CLI filter composes severity,
opcode set, and client filter — if we're debugging one specific drawing
op on one specific client, we can turn off everything else.  A
`--dump-to=/tmp/kx.bin` option captures the raw byte stream to a file
for post-mortem byte-exact diffing.

Right now these helpers are mostly `#[allow(dead_code)]` — they're
consumed starting in Phase 1.  That's intentional.  Writing them first
means we can't "forget" to instrument something later.

### Hand-rolled CLI parsing

No `clap` dependency.  The CLI surface is tiny (display number, log
spec, dump path, a few xinit compat flags like `-nolisten`, `vt1`,
`--nocursor`) and a 100-line hand-written parser keeps the binary
small and the error messages tailored.  Adding a big arg library for
four flags would be silly.

### Build integration

Three new Makefile targets:

- `make kxserver-bin` — runs `cd tools/kxserver && cargo build --release`
- `make kxserver-image` — deletes `build/alpine-xorg.img` and rebuilds
  it so the freshly-built kxserver is copied in
- `make test-kxserver` — kxserver-image + kernel build with
  `INIT_SCRIPT=/bin/test-kxserver` + KVM boot + grep for TEST_PASS lines

`tools/build-alpine-xorg.py` was edited to look for
`tools/kxserver/target/x86_64-unknown-linux-musl/release/kxserver` and,
if present, copy it to `alpine_root/usr/bin/kxserver` with a log line:

```
  KXSRV  installed 418312 bytes at /usr/bin/kxserver
```

The lookup is best-effort — if kxserver hasn't been built, existing
`make test-twm` / `make test-xorg` targets still work.

`testing/test_kxserver.c` is a C harness (musl-static, registered in
`tools/build-initramfs.py`) that mounts `/dev/vda` as the Alpine
rootfs, chroots, runs `kxserver --help` and `kxserver :1`, and checks
that the banner text appears in output.  Cloned from the existing
`test_twm.c` pattern so it fits right into the same test
infrastructure.

## Running it

```
$ make test-kxserver
  CARGO  kxserver (static musl)
   Compiling libc v0.2.185
   Compiling kxserver v0.1.0 (/home/fw/kevlar/tools/kxserver)
    Finished `release` profile [optimized] target(s) in 3.82s
  KXSRV  installed 418312 bytes at /usr/bin/kxserver
  MKDISK  512MB ext2
  DONE  build/alpine-xorg.img
  TEST   kxserver Phase-0 scaffolding
...
TEST_PASS mount_rootfs
TEST_PASS kxserver_help
TEST_PASS kxserver_banner
TEST_PASS kxserver_binary
TEST_END 4/4
```

The interesting part is Phase 3 of the harness — `kxserver :1
--log=req` runs inside the Alpine chroot under Kevlar and produces:

```
[--]         REQ alive display=:1 log=configured dump=<none>
[--]         REQ phase=0 scaffolding-only; exiting (phase 1 will bind /tmp/.X11-unix/X1)
```

The `[--]` prefix means "server-wide, no client, no sequence" — we
don't have any clients yet.  In Phase 1 that will change to
`[C1 #0000]` the moment the first connection is accepted.

## Files created and modified

Created:
- `tools/kxserver/Cargo.toml`
- `tools/kxserver/.cargo/config.toml`
- `tools/kxserver/src/main.rs`
- `tools/kxserver/src/log.rs` (the load-bearing foundation)
- `tools/kxserver/src/config.rs`
- `testing/test_kxserver.c`

Edited:
- `tools/build-alpine-xorg.py` — conditional kxserver install step
- `tools/build-initramfs.py` — register `test_kxserver.c`
- `Makefile` — `kxserver-bin`, `kxserver-image`, `test-kxserver` targets

## Next: Phase 1

Phase 1 replaces the `exit(0)` stub with a real AF_UNIX listen loop.
It will:

1. Create `/tmp/.X11-unix/` if needed
2. Bind both the filesystem path `/tmp/.X11-unix/X1` and the abstract
   socket `@/tmp/.X11-unix/X1`
3. Accept clients in a poll loop
4. Parse the initial handshake (byte order byte, protocol version,
   authentication name, cookie data)
5. Emit a fixed `ConnectionSetup` reply: one screen (1024×768), one
   depth (24), one visual (TrueColor class, RGB masks
   0xFF0000/0xFF00/0xFF), two pixmap formats (depth 1 bpp 1, depth 24
   bpp 32)
6. Ignore any MIT-MAGIC-COOKIE-1 the client presents (we are strictly
   more permissive than real Xorg, so xauth-wrapped clients still work)

Success criterion: `xlsclients -display :1` connects and disconnects
cleanly.  The diff is a ~600 LOC addition touching `src/wire.rs`,
`src/setup.rs`, `src/client.rs`, `src/dispatch.rs`, and
`src/server.rs`.

The visual bugs we're chasing are still several phases away — fonts
and text drawing are Phase 6 — but by then every byte crossing the
wire will be logged through the day-one log infrastructure and the
tracing will tell us exactly where things go wrong.
