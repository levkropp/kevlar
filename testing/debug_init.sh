#!/bin/sh
# Debug init: boot OpenRC with process tracing to find the SIGSEGV crash.
echo "DEBUG: init started"

mount -t proc proc /proc
mount -t sysfs sysfs /sys

echo "DEBUG: starting openrc sysinit"
/sbin/openrc sysinit 2>&1
echo "DEBUG: openrc sysinit rc=$?"

echo "DEBUG: starting openrc boot"
/sbin/openrc boot 2>&1
echo "DEBUG: openrc boot rc=$?"

echo "DEBUG: starting openrc default"
/sbin/openrc default 2>&1
echo "DEBUG: openrc default rc=$?"

# List all running processes so we can correlate PIDs
echo "DEBUG: process list:"
ps aux 2>&1 || ps 2>&1
echo "DEBUG: done"

# Start getty in foreground
exec /sbin/getty -L 115200 ttyS0 linux
