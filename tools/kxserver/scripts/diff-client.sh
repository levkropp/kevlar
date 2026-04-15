#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#
# Run an X11 client against Xvfb (as the reference server) and then
# against kxserver, and report whether each succeeded.  Print
# kxserver's request-level log so divergences point directly at the
# opcode(s) that need attention.
#
# Usage: diff-client.sh xdpyinfo
#        diff-client.sh xsetroot -solid red
#        diff-client.sh xterm -fn fixed -e /bin/true
#
# Env: KXLOG=req|trace|warn  default req
#      KXKEEP=1              keep servers running after client exits
set -uo pipefail

if [ $# -lt 1 ]; then
    echo "usage: $0 CLIENT [args...]" >&2
    exit 2
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/x86_64-unknown-linux-musl/release/kxserver"
if [ ! -x "$BIN" ]; then
    echo "kxserver binary not built at $BIN" >&2
    exit 1
fi

REF_DISPLAY=91
KX_DISPLAY=92
KXLOG="${KXLOG:-req}"
KXLOG_FILE="/tmp/kx-diff-$KX_DISPLAY.log"
XVFB_LOG="/tmp/xvfb-diff-$REF_DISPLAY.log"

cleanup() {
    [ -n "${XVFB_PID:-}" ] && kill "$XVFB_PID" 2>/dev/null
    [ -n "${KX_PID:-}" ]   && kill "$KX_PID"   2>/dev/null
    wait 2>/dev/null
}
[ -z "${KXKEEP:-}" ] && trap cleanup EXIT INT TERM

# Clean up any stragglers from a previous run.
pkill -9 -f "Xvfb :$REF_DISPLAY" 2>/dev/null
pkill -9 -f "kxserver :$KX_DISPLAY" 2>/dev/null
rm -f "/tmp/.X11-unix/X$REF_DISPLAY" "/tmp/.X11-unix/X$KX_DISPLAY"

# ── Start Xvfb ────────────────────────────────────────────────────
Xvfb ":$REF_DISPLAY" -screen 0 1024x768x24 -noreset > "$XVFB_LOG" 2>&1 &
XVFB_PID=$!
sleep 0.3
if ! kill -0 "$XVFB_PID" 2>/dev/null; then
    echo "Xvfb failed to start; log tail:" >&2
    tail "$XVFB_LOG" >&2
    exit 1
fi

# ── Start kxserver ────────────────────────────────────────────────
"$BIN" ":$KX_DISPLAY" --log="$KXLOG" > "$KXLOG_FILE" 2>&1 &
KX_PID=$!
sleep 0.3
if ! kill -0 "$KX_PID" 2>/dev/null; then
    echo "kxserver failed to start; log tail:" >&2
    tail "$KXLOG_FILE" >&2
    exit 1
fi

run_client() {
    local display="$1"
    shift
    DISPLAY=":$display" timeout 10 "$@" > "/tmp/diff-client.$display.out" 2>&1
    echo $?
}

CLIENT_CMD=("$@")
echo "=== ${CLIENT_CMD[*]} against Xvfb (:$REF_DISPLAY) ==="
REF_RC=$(run_client "$REF_DISPLAY" "${CLIENT_CMD[@]}")
echo "  exit=$REF_RC  output:"
sed 's/^/    /' "/tmp/diff-client.$REF_DISPLAY.out" | head -30

echo ""
echo "=== ${CLIENT_CMD[*]} against kxserver (:$KX_DISPLAY) ==="
KX_RC=$(run_client "$KX_DISPLAY" "${CLIENT_CMD[@]}")
echo "  exit=$KX_RC  output:"
sed 's/^/    /' "/tmp/diff-client.$KX_DISPLAY.out" | head -30

echo ""
echo "=== kxserver log (last 80 lines) ==="
tail -80 "$KXLOG_FILE"

echo ""
if [ "$REF_RC" = "0" ] && [ "$KX_RC" = "0" ]; then
    echo "== BOTH SUCCEEDED =="
elif [ "$REF_RC" = "0" ] && [ "$KX_RC" != "0" ]; then
    echo "== DIVERGENCE: ref=PASS kx=FAIL (exit=$KX_RC) =="
    exit 1
elif [ "$REF_RC" != "0" ] && [ "$KX_RC" = "0" ]; then
    echo "== ODD: ref=FAIL kx=PASS (client probably broken) =="
else
    echo "== BOTH FAILED (ref=$REF_RC kx=$KX_RC) =="
    exit 1
fi
