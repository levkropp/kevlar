#!/usr/bin/env python3
"""Autonomous GDB crash investigator for Kevlar.

Starts QEMU with the kernel + an Alpine disk image, connects GDB,
sets breakpoints (hardware or software), and dumps full register/memory
state when they fire.  Outputs structured JSON for post-mortem analysis.

Usage:
    # Investigate the OpenRC INVALID_OPCODE crash at a specific IP:
    python3 tools/gdb-investigate.py --break-user 0xa000411f1 --init /bin/test-openssl-boot --disk build/alpine.img

    # Break at a kernel function:
    python3 tools/gdb-investigate.py --break-sym handle_user_fault --init /bin/test-openssl-boot --disk build/alpine.img

    # Break before the crash IP (set hw breakpoint 2 instructions before):
    python3 tools/gdb-investigate.py --break-user 0xa000411e7 --break-user 0xa000411ef --break-user 0xa000411f1 --init /bin/test-openssl-boot --disk build/alpine.img

    # Trace execution up to N instructions at a specific address range:
    python3 tools/gdb-investigate.py --break-user 0xa000411ea --single-step 20 --init /bin/test-openssl-boot --disk build/alpine.img

Requires: gdb, qemu-system-x86_64 with KVM
"""

import argparse
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GDB_PORT = 7789


def find_symbol(name: str) -> int:
    """Look up a kernel symbol address from kevlar.x64.symbols."""
    sym_file = ROOT / "kevlar.x64.symbols"
    if not sym_file.exists():
        return 0
    best = 0
    for line in sym_file.read_text().splitlines():
        parts = line.strip().split()
        if len(parts) >= 2 and parts[-1] == name:
            return int(parts[0], 16)
        if len(parts) >= 2 and name in parts[-1]:
            best = int(parts[0], 16)
    return best


def patch_kernel_init(init_path: str) -> Path:
    """Patch kernel ELF's KEVLAR_INIT slot and e_machine for QEMU multiboot."""
    elf = ROOT / "kevlar.x64.elf"
    if not elf.exists():
        print(f"ERROR: {elf} not found. Run 'make build' first.", file=sys.stderr)
        sys.exit(1)
    data = bytearray(elf.read_bytes())

    # Patch KEVLAR_INIT slot
    magic = b"KEVLAR_INIT:"
    slot = data.find(magic)
    if slot >= 0:
        path = init_path.encode()[:116]
        data[slot + 12:slot + 128] = path + b"\x00" * (116 - len(path))

    # Patch e_machine for multiboot
    data[18] = 0x03
    data[19] = 0x00

    out = Path(tempfile.mktemp(suffix=".elf"))
    out.write_bytes(bytes(data))
    return out


def create_gdb_script(
    user_breakpoints: list[int],
    sym_breakpoints: list[str],
    single_step_count: int,
    output_file: Path,
) -> Path:
    """Generate a GDB Python script for automated breakpoint-driven investigation."""
    script = Path(tempfile.mktemp(suffix=".py"))

    # Build breakpoint setup commands
    bp_lines = []
    for addr in user_breakpoints:
        # Hardware breakpoint for user addresses (works even before page is mapped)
        bp_lines.append(f'gdb.execute("hbreak *{hex(addr)}")')
        bp_lines.append(f'bp_addrs.append({hex(addr)})')
    for sym in sym_breakpoints:
        bp_lines.append(f'gdb.execute("break {sym}")')
        bp_lines.append(f'bp_addrs.append("{sym}")')

    # Build the script using % formatting to avoid f-string brace issues
    bp_addrs_list = ", ".join(hex(a) for a in user_breakpoints)
    bp_setup = "\n".join(bp_lines)

    script_body = '''\
import gdb
import json

bp_addrs = []
results = {"hits": [], "error": None}

def safe_eval(expr):
    try:
        return int(gdb.parse_and_eval(expr))
    except:
        return 0

def read_mem_u64(addr, count):
    vals = []
    for i in range(count):
        try:
            out = gdb.execute("x/1gx " + str(addr + i*8), to_string=True)
            val = int(out.strip().split()[-1], 16)
            vals.append(val)
        except:
            vals.append(0)
    return vals

def read_mem_bytes(addr, count):
    try:
        out = gdb.execute("x/" + str(count) + "bx " + str(addr), to_string=True)
        vals = []
        for line in out.strip().splitlines():
            for tok in line.split():
                if tok.startswith("0x") and len(tok) <= 4:
                    try: vals.append(int(tok, 16))
                    except: pass
        return vals
    except:
        return []

def dump_regs():
    regs = {}
    for name in ["rip","rsp","rbp","rax","rbx","rcx","rdx","rsi","rdi",
                 "r8","r9","r10","r11","r12","r13","r14","r15",
                 "rflags","cs","ss"]:
        regs[name] = hex(safe_eval("$" + name))
    return regs

def dump_stack(rsp, count=16):
    return [hex(v) for v in read_mem_u64(rsp, count)]

def dump_code(rip, before=16, after=32):
    bytez = read_mem_bytes(rip - before, before + after)
    return {
        "start": hex(rip - before),
        "bytes": " ".join("%02x" % b for b in bytez),
        "rip_offset": before,
    }

def disassemble(rip, count=10):
    try:
        out = gdb.execute("x/" + str(count) + "i " + str(rip), to_string=True)
        return out.strip().splitlines()
    except:
        return []

def single_step_trace(count):
    trace = []
    for i in range(count):
        rip = safe_eval("$rip")
        cs = safe_eval("$cs")
        if cs & 3 == 0:
            trace.append({"step": i, "rip": hex(rip), "note": "entered kernel mode"})
            break
        try:
            inst = gdb.execute("x/1i " + str(rip), to_string=True).strip()
        except:
            inst = "???"
        trace.append({
            "step": i,
            "rip": hex(rip),
            "rax": hex(safe_eval("$rax")),
            "rcx": hex(safe_eval("$rcx")),
            "inst": inst,
        })
        try:
            gdb.execute("stepi")
        except:
            trace.append({"step": i+1, "note": "stepi failed"})
            break
    return trace

gdb.execute("set pagination off")
gdb.execute("set confirm off")

''' + bp_setup + '''

max_hits = 5
single_step = ''' + str(single_step_count) + '''

for hit_idx in range(max_hits):
    try:
        gdb.execute("continue")
    except gdb.error as e:
        results["error"] = "continue failed: " + str(e)
        break

    rip = safe_eval("$rip")
    hit = {
        "hit": hit_idx,
        "registers": dump_regs(),
        "stack": dump_stack(safe_eval("$rsp")),
        "code": dump_code(rip),
        "disasm": disassemble(rip - 8, 12),
    }

    if single_step > 0:
        hit["single_step_trace"] = single_step_trace(single_step)
        single_step = 0

    results["hits"].append(hit)

    if rip in [''' + bp_addrs_list + ''']:
        break

OUTPUT_FILE = "''' + str(output_file) + '''"
with open(OUTPUT_FILE, "w") as f:
    json.dump(results, f, indent=2)

gdb.execute("kill")
gdb.execute("quit")
'''
    script.write_text(script_body)
    return script


def run_investigation(args) -> dict:
    """Run the full automated GDB investigation."""
    # Patch kernel
    patched_elf = patch_kernel_init(args.init)
    print(f"[*] Patched kernel: {patched_elf}")

    # Create output file
    result_file = Path(tempfile.mktemp(suffix=".json"))

    # Create GDB script
    user_bps = [int(a, 0) for a in (args.break_user or [])]
    sym_bps = args.break_sym or []
    gdb_script = create_gdb_script(user_bps, sym_bps, args.single_step, result_file)
    print(f"[*] GDB script: {gdb_script}")

    # Build QEMU command
    qemu_cmd = [
        "qemu-system-x86_64",
        "-m", "1024",
        "-cpu", "Icelake-Server",
        "-enable-kvm",
        "-display", "none",
        "-no-reboot",
        "-serial", "stdio",
        "-monitor", "none",
        "-device", "isa-debug-exit,iobase=0x501,iosize=2",
        "-gdb", f"tcp::{GDB_PORT}",
        "-S",  # Stop at start, wait for GDB
        "-kernel", str(patched_elf),
    ]

    if args.disk:
        qemu_cmd += [
            "-device", "virtio-blk-pci,drive=hd0",
            "-drive", f"file={args.disk},format=raw,if=none,id=hd0",
        ]

    qemu_cmd += [
        "-netdev", "user,id=net0",
        "-device", "virtio-net-pci,netdev=net0,mac=52:54:00:12:34:56",
    ]

    if args.mem_prealloc:
        qemu_cmd += ["-mem-prealloc"]

    print(f"[*] Starting QEMU (port {GDB_PORT})...")
    qemu_proc = subprocess.Popen(
        qemu_cmd,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    time.sleep(1)  # Wait for QEMU to start

    if qemu_proc.poll() is not None:
        print(f"ERROR: QEMU exited immediately (rc={qemu_proc.returncode})")
        return {"error": "QEMU failed to start"}

    print(f"[*] QEMU PID: {qemu_proc.pid}")

    # Build GDB command
    gdb_cmd = [
        "gdb", "-batch", "-nx",
        "-ex", "set pagination off",
        "-ex", f"target remote :{GDB_PORT}",
        "-ex", f"symbol-file {ROOT / 'kevlar.x64.elf'}",
        "-ex", f"source {gdb_script}",
    ]

    print(f"[*] Running GDB (timeout={args.timeout}s)...")
    try:
        gdb_result = subprocess.run(
            gdb_cmd,
            capture_output=True, text=True,
            timeout=args.timeout,
        )
        print(f"[*] GDB exit code: {gdb_result.returncode}")
        # Print key GDB output
        for line in gdb_result.stdout.splitlines():
            if any(k in line.lower() for k in ['breakpoint', 'signal', 'stop', 'thread']):
                print(f"    GDB: {line.strip()}")
        for line in gdb_result.stderr.splitlines():
            if 'error' in line.lower() or 'fail' in line.lower():
                print(f"    GDB ERR: {line.strip()}")
    except subprocess.TimeoutExpired:
        print(f"[*] GDB timed out after {args.timeout}s")
    finally:
        qemu_proc.kill()
        qemu_proc.wait()
        patched_elf.unlink(missing_ok=True)

    # Read results
    if result_file.exists():
        results = json.loads(result_file.read_text())
        result_file.unlink()
    else:
        results = {"error": "No output from GDB script"}

    return results


def main():
    parser = argparse.ArgumentParser(description="Autonomous GDB crash investigator for Kevlar")
    parser.add_argument("--break-user", action="append",
                        help="User-space address to break at (hex, e.g., 0xa000411f1)")
    parser.add_argument("--break-sym", action="append",
                        help="Kernel symbol to break at (e.g., handle_user_fault)")
    parser.add_argument("--single-step", type=int, default=0,
                        help="Single-step N instructions after first breakpoint hit")
    parser.add_argument("--init", default="/bin/test-openssl-boot",
                        help="Init binary path in initramfs")
    parser.add_argument("--disk", default=None,
                        help="Disk image to attach (e.g., build/alpine.img)")
    parser.add_argument("--timeout", type=int, default=120,
                        help="GDB timeout in seconds")
    parser.add_argument("--mem-prealloc", action="store_true",
                        help="Pre-allocate QEMU memory")
    parser.add_argument("--json", default=None,
                        help="Write results to JSON file")
    args = parser.parse_args()

    results = run_investigation(args)

    # Pretty-print results
    print("\n=== Investigation Results ===")
    print(json.dumps(results, indent=2))

    if args.json:
        Path(args.json).write_text(json.dumps(results, indent=2))
        print(f"\nResults written to {args.json}")

    # Summary
    hits = results.get("hits", [])
    if hits:
        print(f"\n{len(hits)} breakpoint hit(s)")
        for h in hits:
            regs = h.get("registers", {})
            print(f"  RIP={regs.get('rip','?')} RSP={regs.get('rsp','?')} "
                  f"RCX={regs.get('rcx','?')}")
            if "single_step_trace" in h:
                print(f"  Single-step trace ({len(h['single_step_trace'])} steps):")
                for step in h["single_step_trace"][:10]:
                    print(f"    {step.get('rip','?')}: {step.get('inst','?')}")
    elif results.get("error"):
        print(f"\nError: {results['error']}")

    return 0 if not results.get("error") else 1


if __name__ == "__main__":
    sys.exit(main())
