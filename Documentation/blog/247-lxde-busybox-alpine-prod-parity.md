# 247 — LXDE works; AF_UNIX matches Linux on disk; busybox suite hits 106/106 against Alpine production busybox

Three things landed this session, each one closing a hard
divergence between Kevlar and Linux:

1. **LXDE on Kevlar arm64**: real busybox-init → real Xorg →
   real openbox → real pcmanfm desktop, no static patches.
   `make ARCH=arm64 test-lxde` is **6/6** and the interactive
   `run-alpine-lxde` boot finishes the session in ~2s.
2. **AF_UNIX is now indexed by inode identity, not path string**.
   `[ -S /tmp/.X11-unix/X0 ]` works.  `mv` of a bound socket
   follows.  `unlink` makes `connect` return ENOENT.  Listener
   identity survives ext2 inode recycling via a per-(dev,ino)
   generation counter.
3. **The busybox applet suite passes 106/106 byte-identically
   on Kevlar and Linux**, against Alpine's *production* busybox
   (full applet set, dynamically linked against Alpine's musl).
   No hand-curated symlink list, no slimmed-down config.  The
   only two SKIPs (`cpio_basic`, `nc_loopback`) skip on Linux
   too — both are userspace/test-design issues, not kernel.

The session was a tour through "Linux as the source of truth":
every time a test failed, run the same suite on Linux first
and let the diff drive the fix.  The first run that uncovered
21 failures turned out to be 21 missing applet symlinks in our
hand-rolled cross-built busybox — Linux failed identically,
which proved Kevlar wasn't the problem.

## Bug 1: busybox init SIGSEGV on arm64 — clone() didn't inherit TPIDR_EL0

The boot-alpine path was crashing with a fault at `0xffffffffffffff5c`
inside musl's `__syscall_ret`.  Adding R8/R9 to the SIGSEGV
register dump revealed x8 = the syscall number = `rt_sigaction`,
and that pid 2 specifically had `TPIDR_EL0 = 0` while everyone
else had it inherited correctly.

`sys_clone()` was the culprit:

```rust
// before: when CLONE_SETTLS is unset, child gets TPIDR_EL0 = 0
let newtls_val = if flags & CLONE_SETTLS != 0 { newtls as u64 } else { 0 };
```

But `clone(2)` is explicit: *"If CLONE_SETTLS is not specified,
the new thread inherits the TLS settings of the calling thread."*
musl's `__init_tp` writes TPIDR_EL0 directly via `msr tpidr_el0`
— outside any syscall — so the kernel needs to capture the live
HW register at clone time, not assume zero.

```rust
let newtls_val = if flags & CLONE_SETTLS != 0 {
    newtls as u64
} else {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, tpidr_el0", out(reg) v); }
    v
};
```

posix_spawn'd children no longer NULL-deref on the first errno
write.

## Bug 2: SIGMAX = 32 was too small

While chasing this, we also tripped over musl's startup calling
`rt_sigaction` on an RT signal (>= 32) during `__init_libc`.
Kevlar's `SIGMAX` was 32, so the syscall returned EINVAL, musl
hit `__syscall_ret`'s error path, and the chain crashed (in this
case via the not-yet-set TPIDR_EL0 — the two bugs reinforced
each other).

Widened `SIGMAX` to 64, with cascading u32 → u64 across:

- `kernel/process/signal.rs` — `pending: u64`, plus 32 RT-signal
  default-action entries.
- `kernel/process/process.rs` — `signal_pending: AtomicU64`.
- `kernel/fs/signalfd.rs` — `mask: u64` (was `u32` cast).
- `kernel/syscalls/rt_sigtimedwait.rs` — `mask: u64`.
- `kernel/main.rs`, `platform/lib.rs` — trait `current_process_signal_pending() -> u64`.

Linux's NSIG is 64.  We match it now.

## Bug 3: AF_UNIX bind didn't create a filesystem node

`make run-alpine-lxde` showed Xorg starting (xdpyinfo connected
fine!) but the start-lxde.sh poll loop on `[ -S /tmp/.X11-unix/X0 ]`
never saw the socket file, even though Xorg had bound it.
Because Kevlar's `AF_UNIX` listener registry was a global
`Vec<(String, Weak<UnixListener>)>` — a path-string lookup table
that bypassed the filesystem entirely.

That's a real Linux divergence.  Linux's bind creates an inode
with mode `S_IFSOCK` at the path, and connect resolves through
that inode.  Effects of the divergence:

- Shell tests like `[ -S /path ]` saw nothing (no inode).
- `stat()` reported ENOENT.
- `mv socket new_path` didn't follow (registry keyed on the old
  string).
- `unlink` left a ghost listener routable by string.

The fix has three layers, each closing one gap:

**Layer 1 — `bind` creates a real on-disk node.**

Extended `tmpfs` and `ext2` `create_file` to honor `S_IFSOCK`
in the `mode` parameter (default still `S_IFREG`).  `unix_socket`
`bind()` for filesystem paths now does:

```rust
let (parent_inode, basename) = root_fs
    .lock()
    .lookup_parent_inode(Path::new(raw_path), true)?;
parent_dir.create_file(basename, FileMode::new(S_IFSOCK | 0o666 & !umask), uid, gid)
```

`stat()` now reports `S_IFSOCK`, `[ -S /path ]` succeeds, and
`readdir` returns `DT_SOCK` (added `FileType::Socket = 12` to
the VFS enum, mapped through both filesystems' readdir paths).

**Layer 2 — `connect` resolves by inode identity, not by string.**

```rust
let pc = root_fs.lookup_path(Path::new(raw_path), true)?;  // returns ENOENT if path gone
let (dev_id, inode_no) = pc.inode.inode_key()?;
let key = ListenerKey::Inode { dev_id, inode_no, generation };
find_listener(&key)
```

Now:
- `mv /tmp/foo /tmp/bar` then `connect("/tmp/bar")` → finds the
  same listener (inode unchanged across rename).
- `unlink("/tmp/foo")` then `connect("/tmp/foo")` → ENOENT
  (lookup_path fails before we even hit the registry).
- Bind-to-existing-path → EADDRINUSE (matches Linux exactly).

**Layer 3 — generation counter for ext2 inode recycling.**

ext2 reuses inode numbers after `unlink + free`.  A long-lived
listener whose path was unlinked could theoretically collide
with a *different* file later bound to the same inode number.
Solution: a `static SOCKET_GENERATIONS: BTreeMap<(dev,ino), u32>`,
bumped on every successful `bind`, peeked on every `connect`,
released by listener `Drop` only when the dropping listener's
generation matches the current one.

```rust
ListenerKey::Inode { dev_id, inode_no, generation }
```

Tmpfs uses a monotonic inode counter so collisions can't happen
there, but the key shape stays uniform across filesystems.

After all three layers, the boot-alpine path's
`[ -S /tmp/.X11-unix/X0 ]` succeeds in 2s on the very first try.

## Bug 4: per-process `cached_utsname` diverged from the namespace

The deepest divergence emerged during the busybox-suite work:
`hostname test-kevlar && hostname` returned `kevlar` (the
original) on Kevlar, but `test-kevlar` on Linux.  rc=0 on both
— `sethostname()` succeeded — but the read-back didn't see the
new value.

Each Process had a `cached_utsname: SpinLock<[u8; 390]>` snapshot
populated at fork.  `sethostname` rebuilt the cache *only on the
calling process*.  Sibling processes (forks of the same parent)
shared the same `Arc<UtsNamespace>` but had stale per-process
caches, so their `uname()` syscalls returned pre-change data.

Linux has no per-process cache — `uname()` reads live from the
namespace, every time.  We now do the same:

```rust
pub fn utsname_copy(&self) -> [u8; 390] {
    let ns = self.namespaces();
    build_cached_utsname(&ns.uts)  // rebuild from live state
}
```

Field removed.  `rebuild_cached_utsname()` removed.  All six
fork/clone/exec sites that copied/initialized the cache now
just don't.  uname becomes one Arc deref + 390-byte buffer
build per call — irrelevant since uname is rare.

## The "Linux as source of truth" workflow

The methodology that drove the busybox work:

1. **`tools/linux-on-hvf/Makefile`** — boots Alpine arm64's
   prebuilt `linux-virt` kernel under QEMU+HVF with our
   initramfs as `-initrd`.  Same userspace, same binary, only
   the kernel differs.
2. **`make test-busybox-alpine`** (new) — boots Kevlar with
   the alpine-lxde disk image, pivot_roots into Alpine, execs
   `/bin/busybox-suite` via `boot_alpine.c`'s new
   `alpine_init=PATH` cmdline arg.  This runs the test against
   Alpine's *production* busybox (full applet symlinks, dynamic
   musl linking, default config).
3. Run on Linux first, get the gold-standard result, diff against
   Kevlar.

First run:
- 79/100 PASS, 21 FAIL on **both** kernels.
- Identical failure sets — proof the test infrastructure (not
  Kevlar) was the issue.

Investigation showed our cross-built initramfs busybox had only
~40 of busybox-1.37's ~350 applet symlinks, because the build
host (macOS) couldn't run the arm64 binary to enumerate via
`busybox --list-full`.  Switching to Alpine's production busybox
(full apk install) resolved 21 of 21.

Second run, real production busybox:
- Kevlar 100/101 (1 SKIP: `hostname_set`)
- Linux 100/101 (1 FAIL: `ifconfig_cmd`)

The single Kevlar SKIP exposed the `cached_utsname` bug above.
The single Linux FAIL was a test-design issue (`ifconfig` with
no args only shows up interfaces, and Linux's init didn't bring
loopback up; switched to `ifconfig -a`).

After both fixes, run three:
- Kevlar 101/101 PASS, 0 FAIL.
- Linux 101/101 PASS, 0 FAIL.
- Byte-identical results.

Then the user asked the obvious question: *why are we still
skipping 7 tests?*  Each one had a comment claiming
"Kevlar fork hang"; every one of those claims was stale.  The
recent ghost-fork / posix_spawn / signal-pending work made all
five of them work.  Enabled one at a time:

| Test | Old claim | Now |
|---|---|---|
| `while_read` | "BusyBox ash subshell survives our timeout kill" | ✅ PASS |
| `file_batch_create` | "50 subprocesses exhaust fork on Kevlar" | ✅ PASS |
| `dev_fd` | "pipe + /dev/fd/0 stdin causes fork hang" | ✅ PASS |
| `many_pipes` | "10 cat subprocesses causes fork hang" | ✅ PASS |
| `rapid_fork` | "20 subshells causes fork hang" | ✅ PASS |
| `cpio_basic` | (conditional skip — `cpio -o` not in busybox config) | SKIP — same on Linux |
| `nc_loopback` | (test design issue — `nc -l` orphans hang `wait`) | SKIP — same hang on Linux |

Kevlar 106/106, Linux 106/106, byte-identical.  Two SKIPs are
genuinely environmental (Alpine busybox config + nc test design)
and reproduce on Linux exactly.

## Quality-of-life: `run-qemu.py` exit code

Surfaced while running tests: `tools/run-qemu.py` printed
`qemu exited with failure status (status=0)` on every successful
arm64 run.  The script only recognized x86's `isa-debug-exit`
magic value `33` as success.  arm64 cleanly exits via PSCI
SYSTEM_OFF, which makes QEMU return 0.  Now both `0` and `33`
are accepted — the misleading message is gone, behavior
unchanged otherwise.

## Files changed

Kernel:
- `kernel/syscalls/clone.rs` — TPIDR_EL0 inheritance when
  `CLONE_SETTLS` is unset.
- `kernel/process/signal.rs` — `SIGMAX = 64`, `pending: u64`,
  RT signal default actions.
- `kernel/process/process.rs` — `signal_pending: AtomicU64`,
  removed per-process `cached_utsname`.
- `kernel/syscalls/sethostname.rs` — drops the no-longer-needed
  cache-rebuild.
- `kernel/fs/signalfd.rs`, `kernel/syscalls/rt_sigtimedwait.rs`,
  `kernel/fs/procfs/proc_self.rs`, `kernel/main.rs`,
  `platform/lib.rs` — u32 → u64 widening cascade.
- `kernel/mm/page_fault.rs` — R8/R9 in fatal SIGSEGV dump.
- `kernel/net/unix_socket.rs` — `ListenerKey::{Abstract, Inode}`,
  inode-keyed registry with generation, fs-node creation on
  bind, path lookup on connect.

VFS:
- `libs/kevlar_vfs/src/inode.rs` — `FileType::Socket = 12` (DT_SOCK).
- `services/kevlar_tmpfs/src/lib.rs` — `create_file` honors
  `S_IFSOCK`; readdir maps to DT_SOCK.
- `services/kevlar_ext2/src/lib.rs` — `EXT2_S_IFSOCK = 0xC000`,
  `EXT2_FT_SOCK = 6`, mode/readdir/file_mode_bits all carry
  through.

Tooling:
- `tools/build-alpine-lxde.py` — kevlar-sysinit.sh consolidates
  the boot script + sync loops; xdpyinfo / `[ -S … ]` poll;
  drops `busybox-suite` into `/bin/busybox-suite`.
- `testing/boot_alpine.c` — honors `alpine_init=PATH` cmdline
  arg (default `/sbin/init`).
- `Makefile` — `test-busybox-alpine` target.
- `tools/run-qemu.py` — accepts exit 0 OR 33 as success.

Tests:
- `testing/busybox_suite.c` — `ifconfig -a`, five skips → real
  test calls; `nc_loopback` skip with explanation.

## What this unlocks

Three things fall out:

1. **LXDE works as a regression target.**  `make test-lxde` is
   6/6 deterministic.  Pixel-difference checks confirm pcmanfm
   draws its desktop background.  We can iterate on real GUI
   programs from here.
2. **AF_UNIX is good enough for D-Bus, X11, systemd's notify
   socket, and any other production stack that does
   `bind`+`connect`+`stat`+`unlink`+`rename`.**  No more
   "userspace expected a real fs node and we had a string."
3. **Linux baseline is the standard testing protocol now.**
   Memory note saved: when a test fails on Kevlar, run it on
   Linux first.  If both fail, fix the test.  If only Kevlar
   fails, fix Kevlar.  This kept us from spending time
   "fixing" 21 imaginary kernel bugs.

## Status

| Surface | Status |
|---|---|
| arm64 openbox | ✅ 5/5 |
| arm64 LXDE | ✅ 6/6 |
| arm64 busybox suite (production Alpine) | ✅ 106/106 |
| arm64 threading SMP | ✅ 14/14 |
| AF_UNIX rename / unlink / S_IFSOCK | ✅ matches Linux |
| `sethostname` visible across siblings | ✅ matches Linux |
| Linux baseline harness | ✅ in tree (`tools/linux-on-hvf/`) |

Next: with LXDE bringing up cleanly under Kevlar, the next
session is about making *programs running inside the LXDE
session* work — file manager interactions, terminal emulator,
text editor, web browser if it'll fit.  Each one will surface
its own divergences, and we now have the methodology to chase
them down without guessing.
