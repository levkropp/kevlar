#!/usr/bin/env python3
"""Automated GDB debugging for signal delivery issues.

Usage:
    python3 tools/gdb-debug-signal.py [--test TEST_NAME] [--timeout SECS]

Creates a patched kernel ELF, starts QEMU with KVM + GDB stub,
connects GDB, sets breakpoints, and dumps register/memory state
at critical points in the signal delivery path.

Requires: gdb, qemu-system-x86_64 with KVM
"""

import argparse
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

def find_symbol(name: str) -> int:
    """Look up a kernel symbol address from kevlar.x64.symbols."""
    sym_file = ROOT / "kevlar.x64.symbols"
    if not sym_file.exists():
        print(f"ERROR: {sym_file} not found. Run 'make build' first.", file=sys.stderr)
        sys.exit(1)
    for line in sym_file.read_text().splitlines():
        parts = line.strip().split()
        if len(parts) >= 2 and name in parts[-1]:
            return int(parts[0], 16)
    return 0

def find_instruction(addr_start: int, addr_end: int, mnemonic: str) -> int:
    """Find an instruction by mnemonic in an address range using objdump."""
    elf = ROOT / "kevlar.x64.elf"
    result = subprocess.run(
        ["objdump", "-d", str(elf),
         f"--start-address={hex(addr_start)}",
         f"--stop-address={hex(addr_end)}"],
        capture_output=True, text=True, timeout=30
    )
    for line in result.stdout.splitlines():
        line = line.strip()
        if mnemonic in line and ':' in line:
            addr_str = line.split(':')[0].strip()
            try:
                return int(addr_str, 16)
            except ValueError:
                continue
    return 0

def patch_kernel(test_bin: str) -> Path:
    """Patch kernel ELF to set init path and e_machine."""
    elf = ROOT / "kevlar.x64.elf"
    data = bytearray(elf.read_bytes())

    # Patch KEVLAR_INIT slot
    magic = b"KEVLAR_INIT:"
    slot = data.find(magic)
    if slot >= 0:
        path = f"/bin/{test_bin}".encode()[:116]
        data[slot+12:slot+128] = path + b"\x00" * (116 - len(path))

    # Patch e_machine for multiboot
    data[18] = 0x03
    data[19] = 0x00

    out = Path(tempfile.mktemp(suffix=".elf"))
    out.write_bytes(bytes(data))
    return out

def create_gdb_script(breakpoints: dict, output_file: Path) -> Path:
    """Create a GDB Python script for automated debugging."""
    script = Path(tempfile.mktemp(suffix=".py"))

    bp_commands = []
    for name, addr in breakpoints.items():
        bp_commands.append(f'    gdb.execute("break *{hex(addr)}")')
        bp_commands.append(f'    bp_names[{hex(addr)}] = "{name}"')

    script.write_text(f'''\
import gdb
import json
import sys

results = {{}}
bp_names = {{}}

def safe_eval(expr):
    """Evaluate a GDB expression, return int or 0 on error."""
    try:
        return int(gdb.parse_and_eval(expr))
    except:
        return 0

def read_mem(addr, count=1):
    """Read count u64 values from addr."""
    vals = []
    for i in range(count):
        try:
            out = gdb.execute(f"x/1gx {{addr + i*8}}", to_string=True)
            val = int(out.strip().split()[-1], 16)
            vals.append(val)
        except:
            vals.append(0)
    return vals

def dump_ptregs(frame_addr):
    """Dump PtRegs fields from a frame address."""
    names = ["r15","r14","r13","r12","rbp","rbx","r11","r10",
             "r9","r8","rax","rcx","rdx","rsi","rdi",
             "orig_rax","rip","cs","rflags","rsp","ss"]
    vals = read_mem(frame_addr, len(names))
    return dict(zip(names, [hex(v) for v in vals]))

# Set breakpoints
{chr(10).join(bp_commands)}

output = {{
    "breakpoints": {{}},
    "error": None,
}}

try:
    # Set breakpoints
    gdb.execute("set pagination off")

    # --- Breakpoint 1: setup_signal_stack entry ---
    gdb.execute("continue")

    bp_addr = safe_eval("$rip")
    bp_name = bp_names.get(bp_addr, f"unknown_{{hex(bp_addr)}}")

    # If we hit the pop_rcx first (or an unexpected address), the signal
    # delivery path may have been inlined or the breakpoint missed.
    # Record whatever we got.
    if bp_addr == 0:
        # Hit address 0 - this IS the crash. The signal was never delivered
        # OR setup_signal_stack was never called.
        output["breakpoints"]["hit_zero"] = {{
            "rip": hex(bp_addr),
            "rsp": hex(safe_eval("$rsp")),
            "rcx": hex(safe_eval("$rcx")),
            "rdi": hex(safe_eval("$rdi")),
            "rsi": hex(safe_eval("$rsi")),
            "rbp": hex(safe_eval("$rbp")),
            "r11": hex(safe_eval("$r11")),
            "cs": hex(safe_eval("$cs")),
            "ss": hex(safe_eval("$ss")),
            "cr3": hex(safe_eval("$cr3")),
            "note": "CPU reached address 0x0 — this is the crash point",
        }}
        # Check if we're in user mode (CS & 3 != 0) or kernel mode
        cs = safe_eval("$cs")
        output["breakpoints"]["hit_zero"]["mode"] = "user" if (cs & 3) else "kernel"

    elif "setup_signal_stack" in bp_name:
        frame_addr = safe_eval("$rsi")  # frame is 2nd arg
        handler_addr = safe_eval("$rcx")  # handler passed in rcx (4th arg in SysV)
        restorer_addr = safe_eval("$r9")

        output["breakpoints"]["setup_signal_stack_entry"] = {{
            "frame_addr": hex(frame_addr),
            "handler": hex(handler_addr),
            "restorer": hex(restorer_addr),
            "signal": safe_eval("$edx"),
            "ptregs_before": dump_ptregs(frame_addr),
        }}

        # Continue to the pop_rcx breakpoint (sysret path)
        gdb.execute("continue")

        bp_addr2 = safe_eval("$rip")
        bp_name2 = bp_names.get(bp_addr2, f"unknown_{{hex(bp_addr2)}}")

        if "pop_rcx" in bp_name2:
            rsp_val = safe_eval("$rsp")
            # What's at [RSP]? This is what pop rcx will load into RCX
            rcx_will_be = read_mem(rsp_val, 1)[0]

            output["breakpoints"]["pop_rcx_before_sysret"] = {{
                "rsp": hex(rsp_val),
                "value_at_rsp": hex(rcx_will_be),
                "expected_handler": hex(handler_addr),
                "match": rcx_will_be == handler_addr,
                "ptregs_at_sysret": dump_ptregs(frame_addr),
            }}

            if rcx_will_be != handler_addr:
                output["error"] = f"MISMATCH: pop rcx will load {{hex(rcx_will_be)}} but handler is {{hex(handler_addr)}}"
            else:
                output["error"] = None
        else:
            output["breakpoints"]["unexpected_bp2"] = {{
                "addr": hex(bp_addr2),
                "name": bp_name2,
            }}
    else:
        output["breakpoints"]["unexpected_bp1"] = {{
            "addr": hex(bp_addr),
            "name": bp_name,
        }}

except gdb.error as e:
    output["error"] = str(e)
except Exception as e:
    output["error"] = f"{{type(e).__name__}}: {{e}}"

# Write results
with open("{output_file}", "w") as f:
    json.dump(output, f, indent=2)

gdb.execute("quit")
''')
    return script


def run_debug(test_name: str, timeout: int) -> dict:
    """Run the full automated debug session."""
    print(f"[*] Debugging signal delivery for {test_name}")

    # Find symbol addresses
    setup_addr = find_symbol("setup_signal_stack")
    if not setup_addr:
        return {"error": "Could not find setup_signal_stack symbol"}
    print(f"    setup_signal_stack: {hex(setup_addr)}")

    # Find pop rcx before sysretq
    # Search in the syscall_entry function (within 1KB of the entry)
    syscall_entry = find_symbol("syscall_entry")
    pop_rcx_addr = find_instruction(syscall_entry, syscall_entry + 0x200, "pop    %rcx")
    if not pop_rcx_addr:
        return {"error": "Could not find pop rcx instruction"}
    print(f"    pop rcx (sysret): {hex(pop_rcx_addr)}")

    # Patch kernel
    test_bin = f"contract-{test_name.split('.')[-1]}"
    patched_elf = patch_kernel(test_bin)
    print(f"    patched kernel: {patched_elf}")

    # Create output file
    result_file = Path(tempfile.mktemp(suffix=".json"))

    # Create GDB script
    breakpoints = {
        "crash_at_zero": 0x0,  # Break when CPU hits ip=0x0 (the crash)
    }
    gdb_script = create_gdb_script(breakpoints, result_file)

    # Start QEMU with KVM + GDB
    # Run WITHOUT GDB — just capture serial output.
    # The kernel's SIGSEGV handler prints ip, rsp, etc. to serial.
    # We parse that for debugging info.
    serial_log = tempfile.mktemp(suffix='.log')
    qemu_proc = subprocess.Popen(
        ["qemu-system-x86_64",
         "-m", "256",
         "-cpu", "Icelake-Server", "-enable-kvm",
         "-display", "none", "-no-reboot",
         "-chardev", f"file,id=ser0,path={serial_log}",
         "-serial", "chardev:ser0", "-monitor", "none",
         "-device", "isa-debug-exit,iobase=0x501,iosize=2",
         "-kernel", str(patched_elf),
         "-append", f"pci=off init=/bin/{test_bin}"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    time.sleep(0.5)  # Wait for QEMU to start

    print(f"    QEMU PID: {qemu_proc.pid}")

    # Run GDB
    try:
        gdb_result = subprocess.run(
            ["gdb", "-batch", "-nx",
             "-ex", "set pagination off",
             "-ex", "target remote :1234",
             "-ex", f"symbol-file {ROOT / 'kevlar.x64.elf'}",
             "-ex", f"source {gdb_script}"],
            capture_output=True, text=True,
            timeout=timeout,
        )
        print(f"    GDB exit code: {gdb_result.returncode}")
        if gdb_result.stderr:
            # Filter noise
            for line in gdb_result.stderr.splitlines():
                if "warning" not in line.lower() and line.strip():
                    print(f"    GDB stderr: {line}")
    except subprocess.TimeoutExpired:
        print(f"    GDB timed out after {timeout}s")
        qemu_proc.kill()
        return {"error": f"GDB timed out after {timeout}s"}
    finally:
        qemu_proc.terminate()
        try:
            qemu_proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            qemu_proc.kill()

    # Read results
    if result_file.exists():
        with open(result_file) as f:
            return json.load(f)
    else:
        return {
            "error": "No output file produced",
            "gdb_stdout": gdb_result.stdout[-500:] if gdb_result else "",
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--test", default="signals.sigaltstack_xfail",
                       help="Contract test name (default: signals.sigaltstack_xfail)")
    parser.add_argument("--timeout", type=int, default=30,
                       help="GDB timeout in seconds (default: 30)")
    args = parser.parse_args()

    result = run_debug(args.test, args.timeout)
    print()
    print(json.dumps(result, indent=2))

    if result.get("error"):
        print(f"\n[!] ERROR: {result['error']}")
        sys.exit(1)
    else:
        print("\n[+] Debug session completed successfully")


if __name__ == "__main__":
    main()
