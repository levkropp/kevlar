#!/usr/bin/env python3
"""iterate-lxde — boot Kevlar+Alpine-LXDE in batch mode, extract the
session log + framebuffer screenshot, summarize what's working and
what's broken.  Designed for tight iteration when bringing up the LXDE
session: change a config, rerun, see whether tint2/pcmanfm/xterm
actually drew anything.

Usage:
    python3 tools/iterate-lxde.py [--arch aarch64|x86_64]
"""
import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DBGFS_CANDIDATES = [
    "/opt/homebrew/Cellar/e2fsprogs/1.47.4/sbin/debugfs",
    "/opt/homebrew/opt/e2fsprogs/sbin/debugfs",
    "/usr/local/opt/e2fsprogs/sbin/debugfs",
    "/usr/sbin/debugfs",
    "debugfs",
]


def find_debugfs() -> str:
    for cand in DBGFS_CANDIDATES:
        if cand == "debugfs":
            p = shutil.which(cand)
            if p:
                return p
        elif Path(cand).exists():
            return cand
    print("ERROR: debugfs not found; brew install e2fsprogs", file=sys.stderr)
    sys.exit(1)


def dump_from_disk(dbgfs: str, img: Path, src: str, dst: Path) -> bool:
    """Copy a file out of the ext2 image.  Returns True if extraction
    succeeded and the file is non-empty."""
    r = subprocess.run(
        [dbgfs, "-R", f"dump {src} {dst}", str(img)],
        capture_output=True, text=True,
    )
    return dst.exists() and dst.stat().st_size > 0


def bgra_to_png(bgra: Path, png: Path, w=1024, h=768) -> tuple[int, int]:
    """Convert a BGRA framebuffer dump to PNG.  Returns (nonblack_pixels,
    total_pixels) so callers can summarize how much of the screen is
    actually drawn.

    Uses PIL's split/merge to swap BGRA → RGBA in-place at C speed —
    a Python per-pixel loop over 786k pixels takes ~10 seconds, while
    split/merge takes ~50 ms.
    """
    try:
        from PIL import Image
    except ImportError:
        print("WARNING: Pillow not installed; skipping PNG conversion",
              file=sys.stderr)
        return (0, w * h)
    with open(bgra, "rb") as f:
        data = f.read()
    # Read as RGBA (bytes are actually BGRA), then swap channels.
    img = Image.frombytes("RGBA", (w, h), data)
    b, g, r, _ = img.split()
    a = Image.new("L", (w, h), 255)
    rgba = Image.merge("RGBA", (r, g, b, a))
    rgba.save(png)
    # Count non-black pixels via PIL's getextrema-on-RGB-channels-summed
    # path: build an L image where each pixel is r|g|b, then count > 0.
    rgb_or = Image.eval(r, lambda v: 0).convert("L")  # placeholder
    # Faster: convert RGB → grayscale luminance proxy via point ops.
    # The sum-of-channels gives 0 only when all three are 0.
    rgb_sum = Image.merge("RGB", (r, g, b)).convert("L", dither=Image.Dither.NONE)
    # convert("L") uses Y' = 0.299R + 0.587G + 0.114B; that's 0 iff
    # R==G==B==0 (since coefficients are positive).
    histogram = rgb_sum.histogram()
    black = histogram[0]
    nonblack = w * h - black
    return nonblack, w * h


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--arch", default="arm64", choices=["arm64", "x64"])
    ap.add_argument("--png", default="build/lxde-iteration.png",
                    help="Where to write the PNG screenshot")
    args = ap.parse_args()

    img = ROOT / "build" / f"alpine-lxde.{args.arch}.img"

    # Run test-lxde — its harness already starts Xorg + openbox + tint2
    # + pcmanfm, runs for ~30s, then writes /root/fb-snapshot.bgra.
    print(f"\n=== iterate-lxde: running test-lxde ({args.arch}) ===\n",
          flush=True)
    log_path = Path(f"/tmp/kevlar-test-lxde-{args.arch}-balanced.log")
    r = subprocess.run(
        ["make", f"ARCH={args.arch}", "test-lxde"],
        cwd=ROOT,
    )
    print(f"\n=== iterate-lxde: test-lxde exited rc={r.returncode} ===",
          flush=True)

    # Pull the on-disk session log + framebuffer dump out of the ext2.
    dbgfs = find_debugfs()
    print(f"\n=== iterate-lxde: extracting on-disk artifacts ===", flush=True)
    out_dir = ROOT / "build"
    out_dir.mkdir(exist_ok=True)
    session_log = out_dir / "lxde-session.log"
    xorg_log = out_dir / "Xorg.0.log"
    bgra_path = out_dir / "lxde-iteration.bgra"
    png_path = ROOT / args.png

    have_session = dump_from_disk(dbgfs, img, "/var/log/lxde-session.log",
                                  session_log)
    have_xorg    = dump_from_disk(dbgfs, img, "/var/log/Xorg.0.log",
                                  xorg_log)
    have_fb      = dump_from_disk(dbgfs, img, "/root/fb-snapshot.bgra",
                                  bgra_path)
    # openbox doesn't have its own log — its stderr is redirected
    # into /tmp/lxde-session.log along with tint2 and pcmanfm.

    # Summary table.
    print()
    print(f"  {'session log':20} {('YES' if have_session else 'no'):4} "
          f"{session_log if have_session else ''}")
    print(f"  {'Xorg.0.log':20} {('YES' if have_xorg else 'no'):4} "
          f"{xorg_log if have_xorg else ''}")
    print(f"  {'fb-snapshot.bgra':20} {('YES' if have_fb else 'no'):4} "
          f"{bgra_path if have_fb else ''}")

    # Convert framebuffer to PNG, report pixel coverage.
    if have_fb:
        nonblack, total = bgra_to_png(bgra_path, png_path)
        pct = 100.0 * nonblack / total
        print(f"\n  framebuffer: {nonblack}/{total} non-black pixels "
              f"({pct:.1f}%) → {png_path}")

    # Print recent test results from the boot log.
    if log_path.exists():
        print(f"\n=== test-lxde results (from {log_path}) ===")
        for line in log_path.read_text(errors="replace").splitlines():
            if line.startswith("TEST_PASS") or line.startswith("TEST_FAIL") \
                    or line.startswith("TEST_END"):
                print(f"  {line}")

    # Print session log so failures in start-lxde.sh are visible.
    if have_session and session_log.stat().st_size > 0:
        print(f"\n=== /var/log/lxde-session.log ({session_log.stat().st_size}B) ===")
        for line in session_log.read_text(errors="replace").splitlines():
            print(f"  {line}")

    # Print last 30 lines of Xorg.0.log if present (helpful for
    # configuration mistakes; full log is on disk).
    if have_xorg and xorg_log.stat().st_size > 0:
        lines = xorg_log.read_text(errors="replace").splitlines()
        print(f"\n=== /var/log/Xorg.0.log (last 30 of {len(lines)} lines) ===")
        for line in lines[-30:]:
            print(f"  {line}")

    # One-line verdict.
    print()
    if have_fb and 'nonblack' in dir() and pct > 50:
        print("=== VERDICT: framebuffer is being drawn (>50% non-black). "
              "Open the PNG to see what landed. ===")
    elif have_fb:
        print("=== VERDICT: framebuffer extracted but mostly black "
              "(<50% drawn).  Check session+Xorg logs above. ===")
    else:
        print("=== VERDICT: no framebuffer snapshot produced.  test-lxde "
              "harness probably failed before reaching the pixel-check.  "
              "Check the session log above. ===")


if __name__ == "__main__":
    main()
