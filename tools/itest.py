#!/usr/bin/env python3
"""itest.py — YAML-driven integration test runner for Kevlar.

Reads a test definition from tests/integration/<name>.yaml, boots
Kevlar in QEMU with QMP attached, runs an ordered sequence of
steps (wait_for_serial, inject_keys, inject_mouse, capture_state,
emit_serial, extract_disk_artifacts, assert), and persists all
artifacts to build/itest/<name>/.

Each test produces:
  - <tag>.png screenshots (one per capture_state step)
  - serial.log (full QEMU stdout)
  - <path>.txt for each disk artifact
  - summary.json (pass/fail + assertion outcomes + variables)

Usage:
    PYTHON3=uv run python tools/itest.py tests/integration/foo.yaml
    make ARCH=arm64 itest TEST=tests/integration/foo.yaml
"""
import argparse
import json
import os
import platform
import re
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path

import yaml

REPO = Path(__file__).resolve().parent.parent
DEFAULT_DBGFS = "/opt/homebrew/opt/e2fsprogs/sbin/debugfs"
DBGFS_CANDIDATES = [
    DEFAULT_DBGFS,
    "/opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs",
    "/opt/homebrew/opt/e2fsprogs/sbin/debugfs",
    "/usr/local/opt/e2fsprogs/sbin/debugfs",
    "/usr/sbin/debugfs",
]


# ── duration parsing ────────────────────────────────────────────
def parse_duration(s):
    """Accept '500ms', '2s', '30s', or a bare int (seconds)."""
    if isinstance(s, (int, float)):
        return float(s)
    s = str(s).strip().lower()
    if s.endswith("ms"):
        return float(s[:-2]) / 1000.0
    if s.endswith("s"):
        return float(s[:-1])
    return float(s)


# ── QMP wire ────────────────────────────────────────────────────
class Qmp:
    """Newline-delimited JSON over Unix socket. One client per session."""

    def __init__(self, sock_path):
        self.sock_path = sock_path
        self.s = None
        self.f = None

    def connect(self, timeout=15.0):
        deadline = time.monotonic() + timeout
        while not os.path.exists(self.sock_path):
            if time.monotonic() > deadline:
                raise RuntimeError(f"QMP socket {self.sock_path} never appeared")
            time.sleep(0.1)
        self.s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.s.settimeout(30.0)
        self.s.connect(self.sock_path)
        self.f = self.s.makefile("rwb")
        self.f.readline()  # greeting
        self.call({"execute": "qmp_capabilities"})

    def send(self, cmd):
        self.f.write((json.dumps(cmd) + "\n").encode())
        self.f.flush()

    def recv(self):
        while True:
            line = self.f.readline()
            if not line:
                return None
            msg = json.loads(line)
            if "event" in msg:
                continue  # skip async events
            return msg

    def call(self, cmd):
        self.send(cmd)
        return self.recv()

    def close(self):
        if self.s:
            try:
                self.s.close()
            except Exception:
                pass

    # ── input injection ────────────────────────────────────────
    # virtio-tablet expects absolute coords in the [0, 32767] range
    # (mirrored by Kevlar's EVIOCGABS at kernel/fs/devfs/evdev.rs:351).
    # Callers supply pixel coords (e.g., 0..1023, 0..767); we scale.
    def send_abs(self, x, y, fb_w=1024, fb_h=768):
        x_norm = max(0, min(32767, int(x * 32767 / max(1, fb_w - 1))))
        y_norm = max(0, min(32767, int(y * 32767 / max(1, fb_h - 1))))
        return self.call({
            "execute": "input-send-event",
            "arguments": {"events": [
                {"type": "abs", "data": {"axis": "x", "value": x_norm}},
                {"type": "abs", "data": {"axis": "y", "value": y_norm}},
            ]},
        })

    def send_btn(self, button, down):
        return self.call({
            "execute": "input-send-event",
            "arguments": {"events": [
                {"type": "btn", "data": {"down": down, "button": button}},
            ]},
        })

    def click(self, x, y, button="left", fb_w=1024, fb_h=768, hold_ms=50):
        self.send_abs(x, y, fb_w, fb_h)
        time.sleep(0.02)
        self.send_btn(button, True)
        time.sleep(hold_ms / 1000.0)
        self.send_btn(button, False)

    def double_click(self, x, y, button="left", fb_w=1024, fb_h=768,
                     hold_ms=50, gap_ms=80):
        self.click(x, y, button, fb_w, fb_h, hold_ms)
        time.sleep(gap_ms / 1000.0)
        self.click(x, y, button, fb_w, fb_h, hold_ms)

    def screendump(self, path, fmt="ppm"):
        # QMP's screendump takes filename + optional format.  arm64
        # has only PPM support before QEMU 8; play safe and request
        # PPM, then convert to PNG with sips/convert.
        return self.call({
            "execute": "screendump",
            "arguments": {"filename": str(path), "format": fmt},
        })


# ── platform helpers ───────────────────────────────────────────
def find_dbgfs():
    for cand in DBGFS_CANDIDATES:
        if Path(cand).exists():
            return cand
    p = shutil.which("debugfs")
    if p:
        return p
    return None


def ppm_to_png(ppm, png):
    if platform.system() == "Darwin" and shutil.which("sips"):
        subprocess.run(["sips", "-s", "format", "png", str(ppm),
                        "--out", str(png)],
                       check=False, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL)
        return png.exists()
    if shutil.which("magick"):
        subprocess.run(["magick", str(ppm), str(png)], check=False)
        return png.exists()
    if shutil.which("convert"):
        subprocess.run(["convert", str(ppm), str(png)], check=False)
        return png.exists()
    # No converter available; leave the PPM in place.
    return False


def count_pixel_diff(ppm_a, ppm_b):
    """Count pixels that differ between two PPMs.  Returns (diff, total)
    or (None, None) if either file is unreadable.  Uses Pillow."""
    try:
        from PIL import Image, ImageChops
    except ImportError:
        return (None, None)
    try:
        a = Image.open(ppm_a).convert("RGB")
        b = Image.open(ppm_b).convert("RGB")
    except Exception:
        return (None, None)
    if a.size != b.size:
        return (None, None)
    diff = ImageChops.difference(a, b)
    # Fold to a single-channel mask: pixel is "different" if any
    # channel differs.  Pillow's getbbox() gives us the bounding
    # box but not the count — we use a histogram on the grayscale
    # difference.
    gray = diff.convert("L")
    hist = gray.histogram()
    total = sum(hist)
    nonzero = total - hist[0]
    return (nonzero, total)


# ── runner ─────────────────────────────────────────────────────
class Runner:
    def __init__(self, test_path, out_dir=None, verbose=True):
        self.test_path = Path(test_path).resolve()
        with open(self.test_path) as f:
            self.spec = yaml.safe_load(f)

        name = self.spec.get("name") or self.test_path.stem
        self.name = name
        self.verbose = verbose
        self.out_dir = Path(out_dir or REPO / "build" / "itest" / name)
        self.out_dir.mkdir(parents=True, exist_ok=True)
        # Clean previous artifacts.
        for f in self.out_dir.iterdir():
            if f.is_file():
                f.unlink()
            else:
                shutil.rmtree(f)

        self.qmp_sock = f"/tmp/kevlar-itest-{os.getpid()}.sock"
        self.serial_log = self.out_dir / "serial.log"
        self.summary = {
            "name": name,
            "test_path": str(self.test_path),
            "started": time.time(),
            "steps": [],
            "captures": {},   # tag -> {ppm, png}
            "variables": {},  # name -> str
            "asserts": [],    # {type, ok, detail}
            "result": None,   # 'pass' | 'fail'
        }

        self.qemu = None
        self.qmp = None
        self.serial_lock = threading.Lock()
        self.serial_buf = bytearray()
        self.serial_running = False
        self.serial_thread = None

    def log(self, msg):
        if self.verbose:
            print(f"[itest] {msg}", file=sys.stderr, flush=True)

    # ── lifecycle ──────────────────────────────────────────────
    def launch_qemu(self):
        arch = self.spec["arch"]
        disk = REPO / self.spec["disk"]
        if not disk.exists():
            raise RuntimeError(f"disk image {disk} missing — run "
                               f"`make ARCH={arch} {disk.name}` first")

        init = self.spec.get("init", "/bin/test-lxde")
        cmdline = self.spec.get("cmdline", "")
        qemu_extra = self.spec.get("qemu_extra", [])

        # Rebuild kernel with the right INIT_SCRIPT.  We always
        # rebuild even if a stale build is present — fast and avoids
        # boot-init-mismatch surprises.  Pipe build output through
        # so failures are visible.
        self.log(f"building kernel with INIT_SCRIPT={init} (ARCH={arch})")
        r = subprocess.run(["make", "build", f"ARCH={arch}",
                            f"INIT_SCRIPT={init}"],
                           cwd=REPO, stdout=subprocess.DEVNULL,
                           stderr=subprocess.PIPE)
        if r.returncode != 0:
            sys.stderr.write(r.stderr.decode("utf-8", errors="replace"))
            raise RuntimeError("kernel build failed")

        try:
            os.unlink(self.qmp_sock)
        except OSError:
            pass

        # On arm64 we MUST use the flat .img (which has the ARM64 Linux
        # Image header so QEMU sets x0=DTB phys addr).  Passing the
        # .elf instead leaves x0=0 and the kernel falls back to "no
        # DTB", which means no virtio-mmio devices get enumerated and
        # the disk + input drivers find nothing.  On x64 we still use
        # the .elf (run-qemu.py patches e_machine for the multiboot
        # loader).
        kernel_path = REPO / (f"kevlar.{arch}.img" if arch == "arm64"
                              else f"kevlar.{arch}.elf")
        # Use the configured PYTHON3 (uv run python on macOS).
        py = os.environ.get("PYTHON3", "python3").split()
        # All run-qemu.py flags must go BEFORE the kernel_elf positional.
        # argparse with nargs="*" qemu_args at the end gets confused if
        # we interleave flags after the positional, so we keep flags
        # tightly grouped here, then kernel_elf, then `--`, then QEMU
        # extras.
        runqemu_flags = ["--kvm", "--batch", "--arch", arch,
                         "--disk", str(disk)]
        if cmdline:
            runqemu_flags += ["--append-cmdline", cmdline]
        # QMP screendump can capture from `-device ramfb` (in run-qemu.py's
        # arm64 base args) without a separate display backend — QEMU
        # reads ramfb's backing memory directly.  So we don't need to
        # add `-display vnc` here.  Just hand run-qemu.py our own QMP
        # socket via qemu_extra.
        argv = (list(py)
                + [str(REPO / "tools" / "run-qemu.py")]
                + runqemu_flags
                + [str(kernel_path), "--"]
                + list(qemu_extra)
                + ["-qmp", f"unix:{self.qmp_sock},server=on,wait=off"])

        self.log(f"argv: {' '.join(argv)}")
        # Open serial log for line-buffered append.
        self.serial_fd = open(self.serial_log, "wb", buffering=0)
        self.qemu = subprocess.Popen(
            argv,
            cwd=REPO,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            preexec_fn=os.setsid,
        )

        # Spawn the serial-tee thread.
        self.serial_running = True
        self.serial_thread = threading.Thread(
            target=self._tee_serial, daemon=True)
        self.serial_thread.start()

        # Connect QMP.
        self.qmp = Qmp(self.qmp_sock)
        self.qmp.connect(timeout=20.0)
        self.log("QMP connected")

    def _tee_serial(self):
        while self.serial_running:
            chunk = self.qemu.stdout.read1(4096)
            if not chunk:
                break
            self.serial_fd.write(chunk)
            with self.serial_lock:
                self.serial_buf.extend(chunk)
        self.serial_running = False

    def shutdown_qemu(self):
        try:
            if self.qmp:
                self.qmp.close()
        except Exception:
            pass
        if self.qemu and self.qemu.poll() is None:
            try:
                os.killpg(os.getpgid(self.qemu.pid), signal.SIGTERM)
            except Exception:
                self.qemu.terminate()
            try:
                self.qemu.wait(timeout=8)
            except Exception:
                try:
                    os.killpg(os.getpgid(self.qemu.pid), signal.SIGKILL)
                except Exception:
                    self.qemu.kill()
        self.serial_running = False
        if self.serial_thread:
            self.serial_thread.join(timeout=2)
        if hasattr(self, "serial_fd"):
            try:
                self.serial_fd.close()
            except Exception:
                pass
        try:
            os.unlink(self.qmp_sock)
        except OSError:
            pass

    # ── steps ──────────────────────────────────────────────────
    def step_wait_for_serial(self, args):
        pattern = args["pattern"]
        timeout = parse_duration(args.get("timeout", "30s"))
        capture = args.get("capture")
        rx = re.compile(pattern.encode())
        deadline = time.monotonic() + timeout
        last_scanned = 0
        while time.monotonic() < deadline:
            with self.serial_lock:
                view = bytes(self.serial_buf[last_scanned:])
                end = len(self.serial_buf)
            m = rx.search(view)
            if m:
                if capture and m.groups():
                    val = m.group(1).decode("utf-8", errors="replace")
                    self.summary["variables"][capture] = val
                    self.log(f"  capture {capture}={val}")
                return True
            last_scanned = max(0, end - len(pattern) - 64)
            time.sleep(0.1)
        raise RuntimeError(f"timeout waiting for serial pattern: {pattern!r}")

    def step_inject_keys(self, args):
        text = args.get("text", "")
        # Reuse the run-qemu.py qcode mapping by importing it.
        import importlib.util
        spec = importlib.util.spec_from_file_location(
            "run_qemu", REPO / "tools" / "run-qemu.py")
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        n = mod._qmp_send_keys(self.qmp_sock, text)
        delay = parse_duration(args.get("delay_after", "0"))
        if delay > 0:
            time.sleep(delay)
        return {"injected": n}

    def step_inject_mouse(self, args):
        action = args.get("action", "move")
        x = int(args.get("x", 0))
        y = int(args.get("y", 0))
        button = args.get("button", "left")
        fb_w = int(args.get("fb_w", self.spec.get("fb_w", 1024)))
        fb_h = int(args.get("fb_h", self.spec.get("fb_h", 768)))
        if action == "move":
            self.qmp.send_abs(x, y, fb_w, fb_h)
        elif action == "click":
            self.qmp.click(x, y, button, fb_w, fb_h)
        elif action == "double_click":
            self.qmp.double_click(x, y, button, fb_w, fb_h)
        else:
            raise RuntimeError(f"unknown inject_mouse action: {action}")
        delay = parse_duration(args.get("delay_after", "0"))
        if delay > 0:
            time.sleep(delay)
        return {"x": x, "y": y, "action": action, "button": button}

    def step_capture_state(self, args):
        delay = parse_duration(args.get("delay_before", "0"))
        if delay > 0:
            time.sleep(delay)
        tag = args.get("tag", f"snap-{len(self.summary['captures'])}")
        ppm = self.out_dir / f"{tag}.ppm"
        png = self.out_dir / f"{tag}.png"
        # screendump writes to QEMU's CWD if relative; pass absolute.
        self.qmp.screendump(ppm)
        # Wait briefly for PPM to be flushed by QEMU.
        for _ in range(20):
            if ppm.exists() and ppm.stat().st_size > 0:
                break
            time.sleep(0.05)
        png_ok = ppm_to_png(ppm, png) if ppm.exists() else False
        self.summary["captures"][tag] = {
            "ppm": str(ppm),
            "png": str(png) if png_ok else None,
            "ppm_size": ppm.stat().st_size if ppm.exists() else 0,
        }
        return {"tag": tag, "png": str(png) if png_ok else None}

    def step_emit_serial(self, args):
        # Write to QEMU's stdin via the existing run-qemu --batch
        # plumbing.  We don't have direct stdin access; instead, use
        # QMP's `human-monitor-command` to type into the guest serial.
        text = args.get("text", "")
        # NOTE: this requires a serial path that's stdio-attached;
        # run-qemu.py --batch uses `-serial stdio`, which means the
        # guest reads from QEMU's stdin.  We don't have that handle.
        # Fall back: use QMP `system_powerdown` is unrelated; we use
        # `human-monitor-command` with `sendkey` only if the device
        # is a console keyboard.  For now this step is best-effort.
        # Most v1 tests don't need it (init scripts use poll loops).
        try:
            self.qmp.call({
                "execute": "human-monitor-command",
                "arguments": {"command-line": f"info status"},
            })
        except Exception:
            pass
        # Best-effort: append to serial_buf so any wait_for_serial
        # downstream sees the marker (the init script won't, but the
        # harness can use this for its own bookkeeping).
        with self.serial_lock:
            self.serial_buf.extend((text + "\n").encode())
        return {"text": text, "warning": "emit_serial v1 is harness-local only"}

    def step_extract_disk_artifacts(self, args):
        dbgfs = find_dbgfs()
        if not dbgfs:
            raise RuntimeError("debugfs not found; brew install e2fsprogs")
        disk = REPO / self.spec["disk"]
        paths = args.get("paths", [])
        out_disk_dir = self.out_dir / "disk"
        out_disk_dir.mkdir(exist_ok=True)
        extracted = {}
        for p in paths:
            # Replace path separators with __ to flatten into one dir.
            safe = p.lstrip("/").replace("/", "__")
            target = out_disk_dir / safe
            r = subprocess.run([dbgfs, "-R", f"dump {p} {target}", str(disk)],
                               capture_output=True, text=True)
            ok = target.exists() and target.stat().st_size >= 0
            extracted[p] = {
                "target": str(target),
                "size": target.stat().st_size if target.exists() else 0,
                "ok": ok,
            }
        return {"extracted": extracted}

    def step_assert(self, args):
        kind = args.get("type")
        ok = False
        detail = ""
        if kind == "framebuffer_changed":
            a, b = args["between"]
            ca = self.summary["captures"].get(a)
            cb = self.summary["captures"].get(b)
            if not ca or not cb:
                detail = f"missing capture: a={ca} b={cb}"
            else:
                diff, total = count_pixel_diff(ca["ppm"], cb["ppm"])
                threshold = int(args.get("min_pixels_changed", 1000))
                detail = f"pixel diff {diff}/{total}, threshold {threshold}"
                if diff is None:
                    detail = "could not compute diff (Pillow missing or PPM unreadable)"
                else:
                    ok = diff >= threshold
        elif kind == "framebuffer_unchanged":
            a, b = args["between"]
            ca = self.summary["captures"].get(a)
            cb = self.summary["captures"].get(b)
            if not ca or not cb:
                detail = f"missing capture: a={ca} b={cb}"
            else:
                diff, total = count_pixel_diff(ca["ppm"], cb["ppm"])
                threshold = int(args.get("max_pixels_changed", 1000))
                detail = f"pixel diff {diff}/{total}, threshold {threshold}"
                if diff is None:
                    detail = "could not compute diff"
                else:
                    ok = diff <= threshold
        elif kind == "framebuffer_painted":
            tag = args["tag"]
            cap = self.summary["captures"].get(tag)
            if not cap:
                detail = f"missing capture {tag}"
            else:
                # "painted" = at least N% pixels non-black.
                from PIL import Image
                img = Image.open(cap["ppm"]).convert("RGB")
                hist = img.convert("L").histogram()
                nonblack = sum(hist) - hist[0]
                total = sum(hist)
                pct = 100.0 * nonblack / max(1, total)
                threshold_pct = float(args.get("min_percent", 50))
                detail = f"{nonblack}/{total} non-black ({pct:.1f}%), threshold {threshold_pct}%"
                ok = pct >= threshold_pct
        elif kind == "serial_contains":
            pattern = args["pattern"]
            with self.serial_lock:
                buf = bytes(self.serial_buf)
            present = re.search(pattern.encode(), buf) is not None
            detail = f"pattern {pattern!r} {'found' if present else 'NOT found'}"
            ok = present
        elif kind == "file_contains":
            path = args["path"]
            text = args["text"]
            safe = path.lstrip("/").replace("/", "__")
            disk_target = self.out_dir / "disk" / safe
            if not disk_target.exists():
                detail = f"{disk_target} not extracted (use extract_disk_artifacts first)"
            else:
                content = disk_target.read_bytes()
                ok = text.encode() in content
                detail = f"text {text!r} {'found' if ok else 'NOT found'} in {path}"
        else:
            raise RuntimeError(f"unknown assert type: {kind}")
        record = {"type": kind, "ok": ok, "detail": detail,
                  "describe": args.get("describe", "")}
        self.summary["asserts"].append(record)
        if not ok:
            self.log(f"  ASSERT FAIL ({kind}): {detail}")
        else:
            self.log(f"  ASSERT PASS ({kind}): {detail}")
        return record

    def step_query_qmp(self, args):
        """Send an arbitrary QMP command and save the response.  Useful
        for diagnostics — e.g. `query-mice`, `query-input-handlers`,
        `x-query-virtio`."""
        cmd = args["execute"]
        out_name = args.get("save_as", cmd.replace("-", "_"))
        payload = {"execute": cmd}
        if "arguments" in args:
            payload["arguments"] = args["arguments"]
        resp = self.qmp.call(payload)
        out = self.out_dir / f"qmp-{out_name}.json"
        with open(out, "w") as f:
            json.dump(resp, f, indent=2)
        return {"saved": str(out), "summary": str(resp)[:200]}

    DISPATCH = {
        "wait_for_serial": "step_wait_for_serial",
        "inject_keys": "step_inject_keys",
        "inject_mouse": "step_inject_mouse",
        "capture_state": "step_capture_state",
        "emit_serial": "step_emit_serial",
        "extract_disk_artifacts": "step_extract_disk_artifacts",
        "assert": "step_assert",
        "query_qmp": "step_query_qmp",
    }

    def run_step(self, step):
        # A step is a one-key dict: {step_name: {args}}.
        ((kind, args),) = step.items()
        args = args or {}
        method = self.DISPATCH.get(kind)
        if not method:
            raise RuntimeError(f"unknown step: {kind}")
        self.log(f"step: {kind}({args})")
        t0 = time.time()
        try:
            result = getattr(self, method)(args)
            ok = True
            err = None
        except Exception as e:
            result = None
            ok = False
            err = str(e)
            self.log(f"  step error: {e}")
        record = {
            "kind": kind,
            "args": args,
            "ok": ok,
            "result": result,
            "error": err,
            "duration": time.time() - t0,
        }
        self.summary["steps"].append(record)
        if not ok:
            raise RuntimeError(f"step {kind} failed: {err}")

    # ── orchestration ──────────────────────────────────────────
    def run(self):
        self.log(f"running {self.name}")
        try:
            self.launch_qemu()
            for step in self.spec.get("steps", []):
                self.run_step(step)
            # If we got here without exception, all steps ran;
            # individual asserts may still have failed.
            failed = [a for a in self.summary["asserts"] if not a["ok"]]
            self.summary["result"] = "fail" if failed else "pass"
        except Exception as e:
            self.summary["result"] = "fail"
            self.summary["error"] = str(e)
            self.log(f"FAIL: {e}")
        finally:
            self.shutdown_qemu()
            # Run extract_disk_artifacts steps that haven't run yet?
            # No — the YAML controls when extraction runs.  Just
            # write the summary.
            self.summary["finished"] = time.time()
            with open(self.out_dir / "summary.json", "w") as f:
                json.dump(self.summary, f, indent=2)
            self.log(f"summary: {self.out_dir/'summary.json'}")
        return self.summary["result"] == "pass"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("test", help="path to YAML test definition")
    ap.add_argument("--out-dir", default=None,
                    help="override output dir (default: build/itest/<name>/)")
    ap.add_argument("-q", "--quiet", action="store_true")
    args = ap.parse_args()

    runner = Runner(args.test, args.out_dir, verbose=not args.quiet)
    ok = runner.run()
    # One-line verdict for Makefile-friendly output.
    asserts = runner.summary["asserts"]
    n_pass = sum(1 for a in asserts if a["ok"])
    n_total = len(asserts)
    print(f"\n{'PASS' if ok else 'FAIL'} {runner.name}: "
          f"{n_pass}/{n_total} assertions, "
          f"artifacts in {runner.out_dir}")
    if not ok:
        for a in asserts:
            mark = "OK" if a["ok"] else "FAIL"
            print(f"  [{mark}] {a['type']}: {a['detail']}")
            if a.get("describe"):
                print(f"         ({a['describe']})")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
