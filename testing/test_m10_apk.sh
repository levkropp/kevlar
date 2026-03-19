#!/bin/sh
# M10 Phase A: apk add integration test for Kevlar.
# Tests that `apk update && apk add curl` completes on an Alpine disk.
#
# NOTE: BusyBox applets must be invoked via /bin/busybox to force fork+exec.

PASS=0
FAIL=0

pass() { echo "TEST_PASS $1"; PASS=$((PASS+1)); }
fail() { echo "TEST_FAIL $1 ($2)"; FAIL=$((FAIL+1)); }

echo "test_m10_apk: start"

# ─── Layer 1: Mount ext2 disk ─────────────────────────────────────────────
/bin/busybox mkdir -p /mnt
if /bin/busybox mount -t ext2 /dev/vda /mnt 2>&1; then
    pass mount_ext2
else
    fail mount_ext2 "mount failed"
    echo "TEST_END $PASS/$((PASS+FAIL))"
    exit 1
fi

# ─── Layer 2: Set up chroot-like environment ─────────────────────────────
# Mount proc/sys inside the Alpine root for apk to work
/bin/busybox mkdir -p /mnt/proc /mnt/sys /mnt/dev /mnt/tmp
/bin/busybox mount -t proc proc /mnt/proc 2>/dev/null
/bin/busybox mount -t sysfs sysfs /mnt/sys 2>/dev/null

# Copy DNS config from initramfs (QEMU user-mode DNS at 10.0.2.3)
/bin/busybox mkdir -p /mnt/etc
echo "nameserver 10.0.2.3" > /mnt/etc/resolv.conf 2>/dev/null

# Verify Alpine rootfs
if [ -f /mnt/bin/busybox ]; then pass alpine_rootfs; else fail alpine_rootfs "not found"; fi

# ─── Layer 3: apk.static --version ──────────────────────────────────────
VER=$(/bin/apk.static --version 2>&1)
case "$VER" in
    *apk-tools*) pass apk_version ;;
    *) fail apk_version "$VER" ;;
esac

# ─── Layer 4: apk update ────────────────────────────────────────────────
# Use apk.static with --root pointing at our mounted Alpine disk
echo "Running apk update..."
if /bin/apk.static --root /mnt --no-progress update 2>&1; then
    pass apk_update
else
    fail apk_update "apk update failed"
fi

# ─── Layer 5: apk add curl ──────────────────────────────────────────────
echo "Running apk add curl..."
if /bin/apk.static --root /mnt --no-progress --no-cache add curl 2>&1; then
    pass apk_add_curl
else
    fail apk_add_curl "apk add curl failed"
fi

# ─── Layer 6: Verify curl was installed ──────────────────────────────────
if [ -f /mnt/usr/bin/curl ]; then
    pass curl_binary_exists
    # Try to run curl --version via chroot
    CURL_VER=$(/bin/busybox chroot /mnt /usr/bin/curl --version 2>&1)
    case "$CURL_VER" in
        *curl*) pass curl_version ;;
        *) fail curl_version "$CURL_VER" ;;
    esac
else
    fail curl_binary_exists "curl not installed"
    fail curl_version "skipped"
fi

# ─── Results ──────────────────────────────────────────────────────────────
TOTAL=$((PASS+FAIL))
echo "TEST_END $PASS/$TOTAL"
if [ $FAIL -gt 0 ]; then
    echo "M10 APK TESTS: $FAIL failure(s)"
else
    echo "ALL M10 APK TESTS PASSED"
fi
exit 0
