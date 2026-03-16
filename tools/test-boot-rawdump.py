#!/usr/bin/env python3
"""Dump raw bytes around the login prompt to diagnose terminal rendering."""
import os, pty, select, subprocess, time, re

KEVLAR_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
elf_path = os.path.join(KEVLAR_DIR, "kevlar.x64.elf")
with open(elf_path, "rb") as f:
    data = bytearray(f.read())
data[18] = 0x03; data[19] = 0x00
patched = "/tmp/kevlar-rawdump.elf"
with open(patched, "wb") as f:
    f.write(data)

master_fd, slave_fd = pty.openpty()
proc = subprocess.Popen(
    ["qemu-system-x86_64", "-serial", "mon:stdio", "-no-reboot", "-nographic",
     "-m", "1024", "-cpu", "Icelake-Server",
     "-netdev", "user,id=net0", "-device", "virtio-net-pci,netdev=net0",
     "-accel", "kvm", "-kernel", patched, "-append", "init=/sbin/init"],
    stdin=slave_fd, stdout=slave_fd, stderr=subprocess.DEVNULL,
    preexec_fn=os.setsid,
)
os.close(slave_fd)

output = b""
deadline = time.time() + 45
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
    if b"login:" in output:
        time.sleep(1)
        ready, _, _ = select.select([master_fd], [], [], 0.5)
        if ready:
            try: output += os.read(master_fd, 4096)
            except OSError: pass
        break

proc.terminate()
try: proc.wait(timeout=5)
except: proc.kill(); proc.wait()
os.close(master_fd)

# Find "login:" and dump surrounding bytes
idx = output.find(b"login:")
if idx >= 0:
    start = max(0, idx - 200)
    end = min(len(output), idx + 100)
    snippet = output[start:end]
    print(f"login: found at byte offset {idx}")
    print(f"\n=== raw hex dump (offset {start}-{end}) ===")
    for i in range(0, len(snippet), 32):
        hexpart = " ".join(f"{b:02x}" for b in snippet[i:i+32])
        ascpart = "".join(chr(b) if 32 <= b < 127 else "." for b in snippet[i:i+32])
        print(f"  {start+i:4d}: {hexpart:<96s} {ascpart}")

    print(f"\n=== decoded text around login: ===")
    text = snippet.decode("utf-8", errors="replace")
    # Show escape sequences explicitly
    text = text.replace("\x1b", "\\e").replace("\r", "\\r").replace("\n", "\\n\n")
    print(text)
else:
    print("login: NOT found in output")
    print(f"total output: {len(output)} bytes")
    # Show last 500 bytes
    tail = output[-500:]
    print("=== last 500 bytes hex ===")
    for i in range(0, len(tail), 32):
        hexpart = " ".join(f"{b:02x}" for b in tail[i:i+32])
        ascpart = "".join(chr(b) if 32 <= b < 127 else "." for b in tail[i:i+32])
        print(f"  {i:4d}: {hexpart:<96s} {ascpart}")
