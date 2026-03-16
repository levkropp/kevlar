#!/bin/bash
# Automated boot test: verifies the kernel boots to a login prompt.
# Usage: ./tools/test-boot.sh [--rebuild]
# Exit 0 = login prompt found, Exit 1 = not found
set -e

SERIAL_LOG=/tmp/kevlar-boot-test-serial.log
TIMEOUT=${TIMEOUT:-45}
KEVLAR_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$KEVLAR_DIR"

echo "=== Kevlar boot test ==="

if [ "$1" = "--rebuild" ] || [ ! -f kevlar.x64.elf ]; then
    echo "Building kernel..."
    make build 2>&1 | tail -3
fi

echo "Patching ELF for multiboot..."
python3 -c "
data = bytearray(open('kevlar.x64.elf', 'rb').read())
data[18] = 0x03; data[19] = 0x00
open('/tmp/kevlar-boot-test.elf', 'wb').write(data)
"

rm -f "$SERIAL_LOG"

echo "Booting QEMU (${TIMEOUT}s timeout, serial -> $SERIAL_LOG)..."
timeout "$TIMEOUT" qemu-system-x86_64 \
    -no-reboot -m 1024 -cpu Icelake-Server \
    -netdev user,id=net0 -device virtio-net-pci,netdev=net0 \
    -accel kvm \
    -kernel /tmp/kevlar-boot-test.elf \
    -append "init=/sbin/init" \
    -serial file:"$SERIAL_LOG" \
    -display none -monitor none \
    > /dev/null 2>&1 || true

BYTES=$(wc -c < "$SERIAL_LOG" 2>/dev/null || echo 0)
echo "Serial output: $BYTES bytes captured"

if [ "$BYTES" -lt 100 ]; then
    echo "FAIL: kernel did not boot (no serial output)"
    exit 1
fi

echo ""
echo "=== Boot log (filtered) ==="
cat "$SERIAL_LOG" | tr -cd '\11\12\15\40-\176' \
    | grep -av "^dynamic link:" \
    | grep -av "^$" \
    | grep -av "qemu-system" \
    | tail -30
echo ""

# Check for login prompt
if grep -qa "login:" "$SERIAL_LOG"; then
    LOGIN_LINE=$(grep -a "login:" "$SERIAL_LOG" | tr -cd '\40-\176' | head -1)
    echo "=== RESULT ==="
    echo "PASS: login prompt found: '$LOGIN_LINE'"
    exit 0
else
    echo "=== RESULT ==="
    echo "FAIL: no login prompt found in serial output"
    exit 1
fi
