#!/usr/bin/env python3
"""strace-diff — compare one binary's syscall behaviour on Kevlar vs Linux.

Runs the same command on both kernels and shows the first divergence
(syscall name, args, or return value). The harness captures:

  - **Linux reference**: runs the command under `strace -f -o ...` on the
    host. Default is the host's own binary (Arch glibc here); pass
    `--linux-chroot PATH` to chroot into an extracted Alpine rootfs first
    (requires sudo), or `--linux-trace FILE` to supply a pre-recorded trace.
  - **Kevlar**:          boots Kevlar with `strace-pid=1` on the cmdline and
    the target command as PID 1. The kernel emits `DBG {...}\\n` JSONL lines
    to serial for every syscall PID 1 makes. Parsed directly.

Both traces are normalized to the same schema:

    {"name": "openat", "args": [-100, "/etc/passwd", 0], "ret": 3, "errno": null}

Alignment is by sequence number (Nth call on Kevlar ↔ Nth call on Linux).
Prints diffs grouped by first divergence.

Usage:
    tools/strace-diff.py -- /bin/ls /etc
    tools/strace-diff.py --kevlar-only -- /bin/true   # only Kevlar trace
    tools/strace-diff.py --linux-only  -- /bin/true   # only Linux trace
    tools/strace-diff.py --max-diffs 20 -- /bin/cat /etc/hostname

Requires: strace, qemu-system-x86_64 + KVM.
"""
import argparse
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

REPO = Path(__file__).resolve().parent.parent


# ─── strace text-format parser (Linux reference) ────────────────────────

# Matches a standard strace line like:
#   openat(AT_FDCWD, "/etc/passwd", O_RDONLY|O_CLOEXEC) = 3
#   openat(AT_FDCWD, "/missing", O_RDONLY)               = -1 ENOENT (No such ...)
#   +++ exited with 0 +++
# With `-f` the pid prefix can be either `[pid 42] call(...)` or the bare
# number followed by a space: `42 call(...)`.
_STRACE_LINE_RE = re.compile(
    r"""
    ^
    (?:\[\s*pid\s+(?P<pid1>\d+)\s*\]\s*     # [pid N] form
     | (?P<pid2>\d+)\s+                     # bare-number form
    )?
    (?P<name>[a-zA-Z_][a-zA-Z0-9_]*)       # syscall name
    \(
    (?P<args>.*)                           # args (may contain escapes/strings)
    \)
    \s*=\s*
    (?P<ret>0x[0-9a-fA-F]+|-?\d+|\?)       # return value: hex first so "0x7f..." doesn't
                                           # match as literal "0"
    (?:\s*(?P<errno>[A-Z_]+)\s*\([^)]*\))? # optional errno + description
    """,
    re.VERBOSE,
)


def parse_strace_text(path: Path, target_pid: Optional[int] = None) -> List[Dict[str, Any]]:
    """Parse strace text output.  Returns list of syscall records."""
    records = []
    with path.open() as f:
        for line in f:
            line = line.rstrip("\n")
            if not line or line.startswith("+++"):
                continue
            # Skip SIG*** stop/continue markers and unfinished lines.
            if line.startswith("---") or "unfinished" in line or "resumed" in line:
                continue
            m = _STRACE_LINE_RE.match(line)
            if not m:
                continue
            pid_str = m.group("pid1") or m.group("pid2")
            pid = int(pid_str) if pid_str else target_pid
            if target_pid is not None and pid is not None and pid != target_pid:
                continue
            name = m.group("name")
            args_text = m.group("args")
            ret = m.group("ret")
            errno = m.group("errno")
            ret_i: Optional[int] = None
            if ret != "?":
                try:
                    ret_i = int(ret, 0)  # 0 = auto-detect base (handles 0x hex)
                except ValueError:
                    pass
            records.append({
                "name": name,
                "args_text": args_text,
                "ret": ret_i,
                "errno": errno,
                "pid": pid,
                "source": "linux",
            })
    return records


# ─── Kevlar JSONL parser ────────────────────────────────────────────────

def parse_kevlar_jsonl(path: Path, target_pid: Optional[int] = None) -> List[Dict[str, Any]]:
    """Parse Kevlar serial log.  Each `DBG {...}` line is one event; we keep
    only `syscall_entry`/`syscall_exit` records for the target PID and pair
    them into a single record per call.

    We run the dispatcher in sequence on a given CPU so entry is always
    followed by the matching exit for that PID; if a later entry arrives
    before we see the exit (SMP / nested), we treat the open entry as the
    call currently in flight (syscalls on one PID are serialized since a
    thread only makes one syscall at a time).
    """
    entries: List[Dict[str, Any]] = []
    pending: Optional[Dict[str, Any]] = None

    ansi = re.compile(r"\x1b\[[0-9;]*m")  # strip ANSI color codes
    with path.open("rb") as f:
        for raw in f:
            try:
                line = raw.decode("utf-8", errors="replace")
            except Exception:
                continue
            line = ansi.sub("", line).rstrip("\r\n")
            # Accept either `DBG {...}` or just `{...}` in case prefix was stripped.
            idx = line.find("DBG ")
            if idx >= 0:
                payload = line[idx + 4:]
            elif line.startswith("{"):
                payload = line
            else:
                continue
            try:
                evt = json.loads(payload)
            except json.JSONDecodeError:
                continue
            t = evt.get("type")
            if t not in ("syscall_entry", "syscall_exit"):
                continue
            pid = evt.get("pid")
            if target_pid is not None and pid != target_pid:
                continue
            if t == "syscall_entry":
                # If there's an in-flight call with no exit, flush it as "?".
                if pending is not None:
                    entries.append(pending)
                args = list(evt.get("args") or [])
                pending = {
                    "name": evt.get("name"),
                    "number": evt.get("nr"),
                    "args": args,
                    "ret": None,
                    "errno": None,
                    "pid": pid,
                    "source": "kevlar",
                }
            else:  # syscall_exit
                if pending is None or pending.get("number") != evt.get("nr"):
                    # Orphan exit — emit a standalone record so alignment
                    # doesn't go haywire.
                    rec = {
                        "name": evt.get("name"),
                        "number": evt.get("nr"),
                        "args": [],
                        "ret": evt.get("result"),
                        "errno": evt.get("errno"),
                        "pid": pid,
                        "source": "kevlar",
                    }
                    entries.append(rec)
                    pending = None
                    continue
                pending["ret"] = evt.get("result")
                pending["errno"] = evt.get("errno")
                entries.append(pending)
                pending = None
    if pending is not None:
        entries.append(pending)
    return entries


# ─── Runners ────────────────────────────────────────────────────────────

def run_linux_native(cmd: List[str], out_path: Path,
                     rootfs: Optional[Path] = None,
                     timeout: int = 60) -> int:
    """Run `cmd` under `strace -f` on the host and write the trace text to
    `out_path`.  Returns the command's exit code.

    If `rootfs` points at an extracted Alpine rootfs, uses `bwrap` to mount
    it as / in a user namespace before running the command — so we trace the
    SAME binary (musl, Alpine version) that Kevlar runs.  No sudo needed.

    If `rootfs=None`, runs the command against the host's libc (useful for
    sanity checks but won't match Kevlar's Alpine-musl syscall pattern).
    """
    # Erase the output file so a failed run doesn't leave stale trace data.
    if out_path.exists():
        out_path.unlink()

    if rootfs is not None:
        # bwrap: unprivileged namespace.  We run `strace` OUTSIDE bwrap,
        # tracing bwrap and all descendants (-f follows forks/execs into the
        # namespace).  This avoids needing strace inside the Alpine rootfs.
        #
        # We explicitly clear the environment (bwrap preserves host env by
        # default) and set ONLY what the Kevlar `strace-target` wrapper also
        # sets — HOME, PATH, TERM, LANG, LC_ALL.  Otherwise host vars like
        # DISPLAY=:1, DBUS_SESSION_BUS_ADDRESS, XDG_RUNTIME_DIR leak through
        # and cause the Linux trace to make X11 / D-Bus syscalls that Kevlar
        # would never attempt.  Matching envs is essential for a fair diff.
        inner = [
            "env", "-i",  # start with NO env
            "bwrap",
            "--bind", str(rootfs), "/",
            "--dev", "/dev",
            "--proc", "/proc",
            "--tmpfs", "/tmp",
            "--tmpfs", "/run",
            "--unshare-user",
            "--unshare-uts",
            "--hostname", "linux-ref",
            "--die-with-parent",
            "--setenv", "PATH", "/usr/bin:/usr/sbin:/bin:/sbin",
            "--setenv", "HOME", "/root",
            "--setenv", "TERM", "vt100",
            "--setenv", "LANG", "C",
            "--setenv", "LC_ALL", "C",
            "--",
        ] + cmd
        argv = ["strace", "-f", "-o", str(out_path), "--"] + inner
    else:
        argv = ["strace", "-f", "-o", str(out_path), "--"] + cmd
    proc = subprocess.run(argv, capture_output=True, timeout=timeout)
    if not out_path.exists() or out_path.stat().st_size == 0:
        sys.stderr.write(proc.stderr.decode("utf-8", errors="replace"))
    return proc.returncode


def run_kevlar(init_path: str, out_path: Path, disk: Path, cmd: List[str],
               timeout: int = 90, smp: int = 2, profile: str = "balanced") -> int:
    """Boot Kevlar with `/bin/strace-target` as PID 1 (via INIT_SCRIPT) and
    `strace-pid=1 strace-exec=cmd[0],cmd[1],...` on the kernel cmdline.
    The init wrapper reads `strace-exec=` from /proc/cmdline, mounts the
    virtio-blk rootfs, chroots, and execs the command.  Every syscall it
    makes is emitted as a `DBG {...}` JSONL line to the serial log.
    """
    # Build the kernel with the requested init.
    r = subprocess.run(
        ["make", "build", f"PROFILE={profile}", f"INIT_SCRIPT={init_path}"],
        cwd=REPO, capture_output=True,
    )
    if r.returncode != 0:
        sys.stderr.write(r.stderr.decode("utf-8", errors="replace"))
        return r.returncode

    # Patch e_machine so QEMU's multiboot loader picks up the ELF. Same
    # trick run-qemu.py uses; replicated here so we can control the full
    # argv (need -append strace-pid=1).
    src_elf = REPO / "kevlar.x64.elf"
    with src_elf.open("rb") as f:
        elf_data = bytearray(f.read())
    elf_data[18] = 0x03  # EM_386 low byte
    elf_data[19] = 0x00
    tmp_fd, tmp_elf = tempfile.mkstemp(suffix=".elf")
    os.write(tmp_fd, elf_data)
    os.close(tmp_fd)

    try:
        qemu = [
            "qemu-system-x86_64",
            "-cpu", "Icelake-Server",
            "-m", "1024",
            "-smp", str(smp),
            "-accel", "kvm",
            "-no-reboot",
            "-mem-prealloc",
            "-display", "none",
            "-serial", f"file:{out_path}",
            "-monitor", "none",
            "-kernel", tmp_elf,
            "-append", f"strace-pid=1 init={init_path} "
                       f"strace-exec={','.join(cmd)}",
            "-drive", f"file={disk},format=raw,if=virtio",
            "-device", "isa-debug-exit,iobase=0x501,iosize=2",
            "-device", "virtio-net,netdev=net0,disable-legacy=on,disable-modern=off",
            "-netdev", "user,id=net0",
        ]
        r = subprocess.run(qemu, timeout=timeout, capture_output=True)
        # Exit code 0x7f in isa-debug-exit = (val << 1) | 1. Kernel uses
        # this for clean shutdown; don't treat as failure.
        return 0
    finally:
        try:
            os.unlink(tmp_elf)
        except FileNotFoundError:
            pass


# ─── Alignment + diff ───────────────────────────────────────────────────

# Syscalls whose return value is a *memory pointer* — different between runs
# by design (ASLR, kernel layout). As long as both succeeded, divergence is
# noise.
POINTER_RET_SYSCALLS = {
    "mmap", "mmap2", "brk", "mremap", "shmat", "mmap_pgoff",
}

# Syscalls whose return value is a *PID-like kernel identifier* — varies by
# process numbering. Noise if both return positive.
PID_RET_SYSCALLS = {
    "set_tid_address", "getpid", "gettid", "getppid", "fork", "vfork",
    "clone", "clone3", "getpgid", "getpgrp", "getsid",
}

# Syscalls whose return value is a user/group ID — the test runs as uid=0 on
# Kevlar and as the host user on Linux by design.
UID_GID_SYSCALLS = {
    "getuid", "geteuid", "getgid", "getegid", "getresuid", "getresgid",
}

# Syscalls where timing / random data is expected to differ.
TIMING_SYSCALLS = {
    "clock_gettime", "clock_gettime64", "gettimeofday", "time",
    "getrandom", "getentropy",
}

# Divergence categories.
MATCH           = "MATCH"
NOISE_POINTER   = "NOISE_POINTER"
NOISE_PID       = "NOISE_PID"
NOISE_UID       = "NOISE_UID"
NOISE_TIMING    = "NOISE_TIMING"
BUG_NAME        = "BUG_NAME"
BUG_ERRNO       = "BUG_ERRNO"
BUG_RETVAL      = "BUG_RETVAL"

IS_NOISE = {NOISE_POINTER, NOISE_PID, NOISE_UID, NOISE_TIMING}
IS_BUG   = {BUG_NAME, BUG_ERRNO, BUG_RETVAL}


def classify(lin: Dict[str, Any], kev: Dict[str, Any]) -> Tuple[str, Optional[str]]:
    """Classify a single (linux, kevlar) call pair into MATCH / NOISE_* / BUG_*.

    Returns (category, detail).  The detail is a human-readable summary for
    bug categories; None for MATCH/NOISE.
    """
    name_l, name_k = lin["name"], kev["name"]
    if name_l != name_k:
        return BUG_NAME, f"linux={name_l} kevlar={name_k}"

    errno_l = lin.get("errno")
    errno_k = kev.get("errno")
    ret_l   = lin.get("ret")
    ret_k   = kev.get("ret")

    # On Kevlar a failed syscall has ret<0 and errno set.  On Linux strace,
    # the ret is -1 and errno is the name.  Normalize to "error vs ok".
    lin_failed = errno_l is not None or (isinstance(ret_l, int) and ret_l == -1)
    kev_failed = errno_k is not None

    if lin_failed != kev_failed:
        detail = (f"linux={'FAIL ' + (errno_l or '?') if lin_failed else 'ok('+str(ret_l)+')'}"
                  f"  kevlar={'FAIL ' + (errno_k or '?') if kev_failed else 'ok('+str(ret_k)+')'}")
        return BUG_ERRNO, detail

    if lin_failed:
        # Both failed.  Only the errno name matters — Linux strace normalizes
        # the ret to -1 while Kevlar uses the raw -errno, so the *number*
        # will differ but both represent the same error.
        if errno_l != errno_k:
            return BUG_ERRNO, f"linux_errno={errno_l} kevlar_errno={errno_k}"
        return MATCH, None

    # Both succeeded.  If returns literally match, we're done.
    if ret_l == ret_k:
        return MATCH, None

    # Same name, same outcome, different return values — classify as noise
    # where the kernel ABI says variance is expected.
    if name_l in POINTER_RET_SYSCALLS:
        if (isinstance(ret_l, int) and isinstance(ret_k, int)
                and ret_l > 0x10000 and ret_k > 0x10000):
            return NOISE_POINTER, None
    if name_l in PID_RET_SYSCALLS:
        if (isinstance(ret_l, int) and isinstance(ret_k, int)
                and ret_l >= 0 and ret_k >= 0):
            return NOISE_PID, None
    if name_l in UID_GID_SYSCALLS:
        return NOISE_UID, None
    if name_l in TIMING_SYSCALLS:
        return NOISE_TIMING, None

    return BUG_RETVAL, f"linux={ret_l} kevlar={ret_k}"


def trim_to_last_execve(records: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Kevlar's PID 1 is the `strace-target` wrapper, which makes a dozen
    syscalls (mount, chroot, etc.) and then execve's the target command.
    Similarly, when Linux runs via bwrap, strace captures bwrap's syscalls
    plus the target's.  Both need trimming to the LAST successful execve,
    which marks the moment the target binary actually starts.
    """
    last_execve = -1
    for i, r in enumerate(records):
        if r["name"] == "execve" and (r.get("ret") is None or r["ret"] == 0):
            last_execve = i
    if last_execve < 0:
        return records
    return records[last_execve:]  # keep execve itself so both sides start aligned


def greedy_align(linux: List[Dict[str, Any]], kevlar: List[Dict[str, Any]],
                 lookahead: int = 6
                 ) -> List[Tuple[Optional[Dict[str, Any]], Optional[Dict[str, Any]]]]:
    """Walk both sequences in lockstep; when names diverge, look ahead a few
    entries on each side to find a match and skip the side that's behind.
    Entries with no counterpart become (entry, None) or (None, entry).
    """
    out: List[Tuple[Optional[Dict[str, Any]], Optional[Dict[str, Any]]]] = []
    i = 0
    j = 0
    while i < len(linux) and j < len(kevlar):
        if linux[i]["name"] == kevlar[j]["name"]:
            out.append((linux[i], kevlar[j]))
            i += 1
            j += 1
            continue
        # Names differ.  Try to find the nearest match within lookahead.
        # Prefer the smaller skip (minimum total i+j advance).
        best = None
        for di in range(0, lookahead + 1):
            for dj in range(0, lookahead + 1):
                if di == 0 and dj == 0:
                    continue
                if i + di >= len(linux) or j + dj >= len(kevlar):
                    continue
                if linux[i + di]["name"] == kevlar[j + dj]["name"]:
                    cost = di + dj
                    if best is None or cost < best[0]:
                        best = (cost, di, dj)
        if best is None:
            # No realignment found — emit both as unpaired and advance.
            out.append((linux[i], None))
            out.append((None, kevlar[j]))
            i += 1
            j += 1
            continue
        # Emit skipped entries unpaired, then the matching pair.
        _, di, dj = best
        for k in range(di):
            out.append((linux[i + k], None))
        for k in range(dj):
            out.append((None, kevlar[j + k]))
        out.append((linux[i + di], kevlar[j + dj]))
        i += di + 1
        j += dj + 1
    while i < len(linux):
        out.append((linux[i], None))
        i += 1
    while j < len(kevlar):
        out.append((None, kevlar[j]))
        j += 1
    return out


def align_and_diff(linux: List[Dict[str, Any]], kevlar: List[Dict[str, Any]],
                   max_diffs: int = 20) -> None:
    """Print aligned comparison.  Groups by classification: MATCH, NOISE
    (expected divergence — pointer/PID/UID/timing), and BUG (real contract
    gap).  Only BUG lines block the goal of "semantic ABI compatibility"."""
    linux = trim_to_last_execve(linux)
    kevlar = trim_to_last_execve(kevlar)

    pairs = greedy_align(linux, kevlar)

    counts: Dict[str, int] = {}
    bugs: List[Tuple[int, Dict[str, Any], Dict[str, Any], str, str]] = []
    n = 0

    for i, (lp, kp) in enumerate(pairs):
        if lp is None or kp is None:
            # Unpaired entry — one side ran this call and the other didn't.
            # Classify as BUG_NAME with a special detail.
            missing = "kevlar" if lp else "linux"
            ran = lp or kp
            counts[BUG_NAME] = counts.get(BUG_NAME, 0) + 1
            bugs.append((i, lp or ran, kp or ran, BUG_NAME,
                         f"{missing} did not make this call: {ran['name']}"))
            continue
        n += 1
        category, detail = classify(lp, kp)
        counts[category] = counts.get(category, 0) + 1
        if category in IS_BUG:
            bugs.append((i, lp, kp, category, detail or ""))

    print(f"# Kevlar/Linux syscall diff — {n} aligned calls")
    print(f"  linux  trace : {len(linux)} calls  kevlar trace : {len(kevlar)} calls")
    print()
    print(f"## Classification")
    match   = counts.get(MATCH, 0)
    noise   = sum(counts.get(c, 0) for c in IS_NOISE)
    bug_ct  = sum(counts.get(c, 0) for c in IS_BUG)
    print(f"  MATCH           {match:5d}  — Linux and Kevlar agreed")
    print(f"  NOISE (total)   {noise:5d}  — allowed variance (pointers/PIDs/UIDs/time)")
    for c in (NOISE_POINTER, NOISE_PID, NOISE_UID, NOISE_TIMING):
        v = counts.get(c, 0)
        if v: print(f"    {c:<16} {v:5d}")
    print(f"  BUG   (total)   {bug_ct:5d}  — real contract gap")
    for c in (BUG_NAME, BUG_ERRNO, BUG_RETVAL):
        v = counts.get(c, 0)
        if v: print(f"    {c:<16} {v:5d}")
    print()

    if bugs:
        print(f"## First {min(len(bugs), max_diffs)} contract bugs")
        for idx, (i, ld, kd, cat, detail) in enumerate(bugs[:max_diffs]):
            print(f"#{i:4d}  [{cat}] {ld['name']}")
            print(f"       {detail}")
            if ld.get("args_text"):
                print(f"       linux args : {ld['args_text'][:180]}")
            if kd.get("args") is not None:
                args_str = ", ".join(f"{a:#x}" if isinstance(a, int) and a > 255 else repr(a)
                                     for a in kd['args'][:6])
                print(f"       kevlar args: [{args_str}]")
            print()
        if len(bugs) > max_diffs:
            print(f"... {len(bugs) - max_diffs} more bugs; use --max-diffs to see")
    else:
        print(f"## 🟢 No contract bugs in the first {n} aligned calls.")
        print(f"   All divergences are in the ABI's allowed variance set.")

    if len(linux) != len(kevlar):
        delta = abs(len(linux) - len(kevlar))
        longer = "linux" if len(linux) > len(kevlar) else "kevlar"
        print()
        print(f"# Trace length differs by {delta} ({longer} is longer).")
        print(f"# This means one side ran more syscalls than the other — if the")
        print(f"# first N matched, the divergence starts around call #{n}.")


# ─── Main ───────────────────────────────────────────────────────────────

def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--disk", default="build/alpine-xfce.img",
                    help="Kevlar disk image (default: build/alpine-xfce.img)")
    ap.add_argument("--linux-rootfs", default=None,
                    help="extracted Alpine rootfs directory; strace runs inside "
                         "via bwrap so we trace the same musl binary Kevlar runs. "
                         "See tools/extract-alpine-rootfs.py. Omit to run against "
                         "the host's libc.")
    ap.add_argument("--linux-trace", default=None,
                    help="Skip running on Linux; parse this pre-recorded strace "
                         "text file instead.")
    ap.add_argument("--kevlar-only", action="store_true")
    ap.add_argument("--linux-only", action="store_true")
    ap.add_argument("--init", default="/bin/strace-target",
                    help="PID 1 on Kevlar (default: /bin/strace-target, a wrapper that "
                         "chroots into /mnt and execs the command in argv)")
    ap.add_argument("--max-diffs", type=int, default=20)
    ap.add_argument("--out-dir", default="build/strace-diff",
                    help="Where to write linux.trace / kevlar.jsonl")
    ap.add_argument("--timeout", type=int, default=120)
    ap.add_argument("--smp", type=int, default=2)
    ap.add_argument("--profile", default="balanced")
    ap.add_argument("cmd", nargs="+", help="Command to trace (after --)")
    args = ap.parse_args()

    out_dir = REPO / args.out_dir
    out_dir.mkdir(parents=True, exist_ok=True)
    linux_trace = out_dir / "linux.trace"
    kevlar_serial = out_dir / "kevlar.serial"

    linux_records: List[Dict[str, Any]] = []
    kevlar_records: List[Dict[str, Any]] = []

    if not args.kevlar_only:
        if args.linux_trace:
            linux_trace = Path(args.linux_trace)
            print(f"[linux] using pre-recorded {linux_trace}", file=sys.stderr)
        else:
            rootfs = Path(args.linux_rootfs) if args.linux_rootfs else None
            print(f"[linux] strace -f -- {' '.join(args.cmd)}"
                  + (f" (bwrap {rootfs})" if rootfs else " (host)"),
                  file=sys.stderr)
            rc = run_linux_native(args.cmd, linux_trace, rootfs=rootfs,
                                  timeout=args.timeout)
            if rc != 0:
                print(f"[linux] command exited {rc}, trace may be partial",
                      file=sys.stderr)
        linux_records = parse_strace_text(linux_trace)
        print(f"[linux] captured {len(linux_records)} syscalls -> {linux_trace}",
              file=sys.stderr)

    if not args.linux_only:
        print(f"[kevlar] booting with strace-pid=1 and init={args.init}...",
              file=sys.stderr)
        rc = run_kevlar(args.init, kevlar_serial,
                        disk=REPO / args.disk, cmd=args.cmd,
                        timeout=args.timeout, smp=args.smp, profile=args.profile)
        kevlar_records = parse_kevlar_jsonl(kevlar_serial, target_pid=1)
        print(f"[kevlar] captured {len(kevlar_records)} syscalls -> {kevlar_serial}",
              file=sys.stderr)

    if args.kevlar_only:
        print(json.dumps(kevlar_records, indent=2))
        return 0
    if args.linux_only:
        print(json.dumps(linux_records, indent=2))
        return 0

    align_and_diff(linux_records, kevlar_records, max_diffs=args.max_diffs)
    return 0


if __name__ == "__main__":
    sys.exit(main())
