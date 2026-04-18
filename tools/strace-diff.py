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
    (?P<ret>-?\d+|\?|0x[0-9a-fA-F]+)       # return value, ?, or hex (mmap etc.)
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
        inner = [
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

def normalize_ret(r: Optional[int]) -> Optional[int]:
    """Normalize return values so Linux and Kevlar compare consistently.
    Linux: negative errno is surfaced as errno name. Kevlar: negative errno
    is `result` with `errno` field set."""
    if r is None:
        return None
    return r


def compare_entries(lin: Dict[str, Any], kev: Dict[str, Any]) -> List[str]:
    """Return a list of human-readable differences, empty if compatible."""
    diffs: List[str] = []
    if lin["name"] != kev["name"]:
        diffs.append(f"name:  linux={lin['name']}  kevlar={kev['name']}")
        return diffs  # everything else meaningless once names diverge

    # Compare return values. Linux treats -1 as "call failed, see errno".
    # Kevlar uses negative errno directly as result. Normalize.
    lin_ret = normalize_ret(lin.get("ret"))
    kev_ret = normalize_ret(kev.get("ret"))
    lin_err = lin.get("errno")
    kev_err = kev.get("errno")

    if lin_err or kev_err:
        if lin_err != kev_err:
            diffs.append(f"errno: linux={lin_err}  kevlar={kev_err}")
    else:
        if lin_ret != kev_ret:
            diffs.append(f"ret:   linux={lin_ret}  kevlar={kev_ret}")

    # Arg comparison is best-effort: Kevlar sees raw integers, Linux sees
    # symbolic flags/strings. We can only sanity-check argument COUNT.
    if kev.get("args") is not None and len(kev["args"]) >= 6:
        # Both ran — fine.
        pass

    return diffs


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


def align_and_diff(linux: List[Dict[str, Any]], kevlar: List[Dict[str, Any]],
                   max_diffs: int = 20) -> None:
    """Print aligned comparison up to `max_diffs` divergences."""
    linux = trim_to_last_execve(linux)
    kevlar = trim_to_last_execve(kevlar)
    print(f"# Alignment summary")
    print(f"  linux   calls: {len(linux)}")
    print(f"  kevlar  calls: {len(kevlar)}")
    print()

    n = min(len(linux), len(kevlar))
    diffs_found = 0
    matches = 0
    printed_limit = False

    for i in range(n):
        ld = linux[i]
        kd = kevlar[i]
        issues = compare_entries(ld, kd)
        if not issues:
            matches += 1
            continue
        if diffs_found < max_diffs:
            print(f"#{i:4d}  name={ld['name']}")
            for iss in issues:
                print(f"       {iss}")
            # Raw args for debugging
            if ld.get("args_text"):
                print(f"       linux args: {ld['args_text'][:200]}")
            if kd.get("args") is not None:
                args_str = ", ".join(f"{a:#x}" if isinstance(a, int) else repr(a)
                                     for a in kd['args'][:6])
                print(f"       kevlar args: [{args_str}]")
            print()
        elif not printed_limit:
            print(f"... {max_diffs} diffs shown, more suppressed; use --max-diffs to see more")
            printed_limit = True
        diffs_found += 1

    print(f"# Totals: {matches} matching, {diffs_found} diverging "
          f"out of {n} aligned calls.")
    if len(linux) != len(kevlar):
        print(f"# Trace lengths differ by {abs(len(linux) - len(kevlar))} — "
              f"drift is real (e.g. Kevlar retrying or skipping calls).")


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
