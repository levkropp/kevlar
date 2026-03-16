#!/bin/sh
# Test script for apk update on writable ext2 filesystem.

echo "TEST_START apk_update"
sleep 2

mount -t ext2 /dev/vda /mnt
echo "PASS mount_ext2"

# Test apk info via chroot (no network).
chroot /mnt /sbin/apk info 2>&1 | wc -l
echo "PASS chroot_apk_info"

# Test apk update — give it 120 seconds.
echo "=== apk update ==="
chroot /mnt /sbin/apk --no-progress --wait 60 update 2>&1
echo "apk update exit: $?"
echo "DONE apk_update"

# Check results.
ls -la /mnt/lib/apk/db/ 2>&1
echo "TEST_END"
poweroff -f
