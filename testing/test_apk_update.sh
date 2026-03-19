#!/bin/sh
# Alpine integration test for Kevlar.
# Tests mount namespace sharing across fork and ext2 filesystem access.
#
# NOTE: BusyBox applets (mount, ls, etc.) must be invoked via /bin/busybox
# to force a real fork+exec.  Without this, BusyBox ash runs them in-process
# (NOFORK) which corrupts script-reading state and causes infinite re-execution.

PASS=0
FAIL=0

pass() { echo "TEST_PASS $1"; PASS=$((PASS+1)); }
fail() { echo "TEST_FAIL $1 ($2)"; FAIL=$((FAIL+1)); }

echo "test_alpine: start"

# ─── Layer 1: Mount ext2 disk ─────────────────────────────────────────────
/bin/busybox mkdir -p /mnt
if /bin/busybox mount -t ext2 /dev/vda /mnt 2>&1; then
    pass mount_ext2
else
    fail mount_ext2 "mount failed"
    echo "TEST_END $PASS/$((PASS+FAIL))"
    exit 1
fi

# ─── Layer 2: Verify Alpine rootfs structure (tests mount visibility) ─────
if [ -f /mnt/bin/busybox ]; then pass busybox_exists; else fail busybox_exists "not found"; fi
if [ -f /mnt/lib/ld-musl-x86_64.so.1 ]; then pass musl_ld_exists; else fail musl_ld_exists "not found"; fi
if [ -f /mnt/etc/apk/repositories ]; then pass repositories_exists; else fail repositories_exists "not found"; fi

# ─── Layer 3: apk.static --version (no mount access needed) ──────────────
VER=$(/bin/apk.static --version 2>&1)
case "$VER" in
    *apk-tools*) pass apk_version ;;
    *) fail apk_version "$VER" ;;
esac

# ─── Layer 4: Read files from ext2 mount (proves mount namespace works) ──
# The key test: these commands fork child processes that must see /mnt
CONTENT=$(/bin/busybox cat /mnt/etc/os-release 2>/dev/null)
case "$CONTENT" in
    *Alpine*) pass os_release_readable ;;
    *) fail os_release_readable "ext2 read failed" ;;
esac

# List directories to prove deep traversal works across fork
DIRS=$(/bin/busybox ls /mnt/bin/ 2>/dev/null | /bin/busybox wc -l)
if [ "$DIRS" -gt 0 ]; then
    pass ext2_dir_listing
    echo "  /mnt/bin/ has $DIRS entries"
else
    fail ext2_dir_listing "empty listing"
fi

# ─── Results ──────────────────────────────────────────────────────────────
TOTAL=$((PASS+FAIL))
echo "TEST_END $PASS/$TOTAL"
if [ $FAIL -gt 0 ]; then
    echo "ALPINE TESTS: $FAIL failure(s)"
else
    echo "ALL ALPINE TESTS PASSED"
fi
exit 0
