#!/usr/bin/env python3
"""Test boot with a real PTY (simulates interactive `make run`).

Spawns QEMU with mon:stdio connected to a PTY, captures all output,
and checks for the login prompt. This reproduces the exact same
environment as `make run`.
"""
import os
import pty
import select
import subprocess
import sys
import time

TIMEOUT = 45
KEVLAR_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# Patch ELF
elf_path = os.path.join(KEVLAR_DIR, "kevlar.x64.elf")
with open(elf_path, "rb") as f:
    data = bytearray(f.read())
data[18] = 0x03
data[19] = 0x00
patched = "/tmp/kevlar-boot-pty-test.elf"
with open(patched, "wb") as f:
    f.write(data)

# Create a PTY pair
master_fd, slave_fd = pty.openpty()

# Launch QEMU with the slave side as stdin/stdout
cmd = [
    "qemu-system-x86_64",
    "-serial", "mon:stdio",
    "-no-reboot", "-nographic",
    "-m", "1024", "-cpu", "Icelake-Server",
    "-netdev", "user,id=net0",
    "-device", "virtio-net-pci,netdev=net0",
    "-accel", "kvm",
    "-kernel", patched,
    "-append", "init=/sbin/init",
]

proc = subprocess.Popen(
    cmd,
    stdin=slave_fd,
    stdout=slave_fd,
    stderr=subprocess.DEVNULL,
    preexec_fn=os.setsid,
)
os.close(slave_fd)

# Read from master_fd with timeout
output = b""
deadline = time.time() + TIMEOUT
try:
    while time.time() < deadline:
        ready, _, _ = select.select([master_fd], [], [], 1.0)
        if ready:
            try:
                chunk = os.read(master_fd, 4096)
                if chunk:
                    output += chunk
                else:
                    break
            except OSError:
                break

        # Check if we already found login prompt
        if b"login:" in output:
            # Give a moment for any trailing output
            time.sleep(0.5)
            ready, _, _ = select.select([master_fd], [], [], 0.5)
            if ready:
                try:
                    output += os.read(master_fd, 4096)
                except OSError:
                    pass
            break
finally:
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()
    os.close(master_fd)

# Analyze output
text = output.decode("utf-8", errors="replace")
lines = [l for l in text.split("\n")
         if l.strip()
         and "dynamic link:" not in l
         and "qemu-system" not in l
         and "\x1b" not in l]  # skip escape sequences

print(f"captured {len(output)} bytes, {len(lines)} clean lines")

if "login:" in text:
    # Find the login line
    for line in text.split("\n"):
        if "login:" in line:
            clean = line.replace("\r", "").strip()
            # Remove escape sequences
            import re
            clean = re.sub(r'\x1b\[[0-9;]*[a-zA-Z]', '', clean)
            if clean:
                print(f"PASS: {clean}")
                break
    sys.exit(0)
else:
    print("FAIL: no login prompt found")
    print("=== last 15 clean lines ===")
    for line in lines[-15:]:
        print(f"  {line.rstrip()}")
    sys.exit(1)
