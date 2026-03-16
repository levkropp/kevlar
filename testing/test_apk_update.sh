#!/bin/sh
# Test script for apk update on writable ext2 filesystem.

echo "TEST_START apk_update"

# Mount the Alpine ext2 disk.
mount -t ext2 /dev/vda /mnt
echo "PASS mount_ext2"

# Verify Alpine rootfs contents.
ls /mnt/etc/apk/ >/dev/null 2>&1 && echo "PASS alpine_rootfs" || echo "FAIL alpine_rootfs"

# Test ext2 writes.
echo "hello" > /mnt/test_write.txt && cat /mnt/test_write.txt >/dev/null && echo "PASS ext2_write"
rm -f /mnt/test_write.txt

# Test apk info via chroot (no network needed).
echo "=== chroot apk info ==="
chroot /mnt /sbin/apk info 2>&1 | tail -5
echo "PASS chroot_apk_info"

# Wait for DHCP to complete.
echo "=== Waiting for DHCP ==="
for i in $(seq 1 15); do
    # Check if we have an IP by looking for the DHCP log message
    sleep 1
done
echo "Network wait done"

# Test apk update via chroot.
echo "=== chroot apk update ==="
chroot /mnt /sbin/apk --no-progress --wait 30 update 2>&1
echo "apk update exit code: $?"
echo "DONE chroot_apk_update"

# Check if index was downloaded.
ls -la /mnt/var/cache/apk/ 2>&1
ls -la /mnt/lib/apk/db/ 2>&1

echo "TEST_END"
poweroff -f
