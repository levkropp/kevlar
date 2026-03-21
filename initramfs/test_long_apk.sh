#!/bin/sh
# Minimal apk update test with timeout for ktrace diagnosis.
# apk update hangs — this script ensures PID 1 exits after 30s
# so the ktrace dump fires.

echo "ktrace_apk: mounting disk"
/bin/busybox mkdir -p /mnt
/bin/busybox mount -t ext2 /dev/vda /mnt

echo "ktrace_apk: setting up chroot"
/bin/busybox mkdir -p /mnt/proc /mnt/sys /mnt/dev /mnt/tmp
/bin/busybox mount -t proc proc /mnt/proc 2>/dev/null
echo "nameserver 10.0.2.3" > /mnt/etc/resolv.conf

echo "ktrace_apk: starting apk update with 30s timeout"
/bin/busybox timeout 120 /bin/apk.static --root /mnt --no-progress update 2>&1
RC=$?
echo "ktrace_apk: apk exited with code $RC"

echo "ktrace_apk: done, exiting"
exit $RC
