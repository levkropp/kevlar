#!/usr/bin/env python3
"""GDB investigation runner for Kevlar.

Executes a GDB investigation plan against QEMU+KVM.  The plan is a JSON
file (or inline JSON) describing breakpoints, conditions, and data to
collect at each hit.

Usage:
    # Run from a plan file:
    python3 tools/gdb-run.py plan.json --disk build/alpine.img

    # Inline plan (break at kernel symbol, dump registers):
    python3 tools/gdb-run.py '{"steps":[{"break":"hbreak *0xffff80000016ee90","collect":["regs","stack 8"]}]}' --disk build/alpine.img

    # Use the Makefile:
    make gdb-run PLAN=tools/plans/openrc-sigchld.json

Plan format:
    {
        "init": "/bin/test-openssl-boot",       # init binary (default)
        "steps": [
            {
                "name": "catch_fault",           # human label
                "break": "hbreak *0xffff...",    # GDB breakpoint command
                "condition": "$rax == 13",       # optional: only stop if true
                "collect": [                     # what to dump at this hit
                    "regs",                      # all GPRs
                    "stack 16",                  # 16 qwords from RSP
                    "mem $rsi 32",               # 32 bytes from address in RSI
                    "disasm $rip 10",            # 10 instructions from RIP
                    "expr $rdi",                 # evaluate expression
                    "bt 10",                     # backtrace 10 frames
                    "x/4gx $rsp+0x80"            # raw GDB command
                ],
                "then": "continue",             # after collecting: continue | stop | delete-and-continue
                "max_hits": 50000               # skip up to N non-matching hits
            }
        ]
    }

Requires: gdb, qemu-system-x86_64, KVM
"""

import argparse
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GDB_PORT = 7789


def find_symbol(name: str) -> int:
    sym_file = ROOT / "kevlar.x64.symbols"
    if not sym_file.exists():
        return 0
    for line in sym_file.read_text().splitlines():
        parts = line.strip().split()
        if len(parts) >= 2 and parts[-1] == name:
            return int(parts[0], 16)
    return 0


def patch_kernel(init_path: str) -> Path:
    elf = ROOT / "kevlar.x64.elf"
    if not elf.exists():
        print(f"ERROR: {elf} not found. Run 'make build' first.", file=sys.stderr)
        sys.exit(1)
    data = bytearray(elf.read_bytes())
    magic = b"KEVLAR_INIT:"
    slot = data.find(magic)
    if slot >= 0:
        path = init_path.encode()[:116]
        data[slot + 12:slot + 128] = path + b"\x00" * (116 - len(path))
    data[18] = 0x03
    data[19] = 0x00
    out = Path(tempfile.mktemp(suffix=".elf"))
    out.write_bytes(bytes(data))
    return out


def generate_gdb_script(plan: dict, output_file: Path) -> Path:
    """Generate a GDB Python script from a plan."""
    script = Path(tempfile.mktemp(suffix=".py"))

    steps = plan.get("steps", [])

    # Build the GDB Python script
    lines = [
        'import gdb',
        'import json',
        'import traceback',
        '',
        'gdb.execute("set pagination off")',
        'gdb.execute("set confirm off")',
        '',
        '# --- Helper functions ---',
        'def safe_eval(expr):',
        '    try: return int(gdb.parse_and_eval(expr))',
        '    except: return 0',
        '',
        'def read_u64(addr):',
        '    try:',
        '        out = gdb.execute("x/1gx " + str(addr), to_string=True)',
        '        return int(out.strip().split()[-1], 16)',
        '    except: return 0',
        '',
        'def read_bytes(addr, count):',
        '    try:',
        '        out = gdb.execute("x/" + str(count) + "bx " + str(addr), to_string=True)',
        '        vals = []',
        '        for line in out.strip().splitlines():',
        '            for tok in line.split():',
        '                if tok.startswith("0x") and len(tok) <= 4:',
        '                    try: vals.append(int(tok, 16))',
        '                    except: pass',
        '        return vals',
        '    except: return []',
        '',
        'def collect_regs():',
        '    regs = {}',
        '    for name in ["rip","rsp","rbp","rax","rbx","rcx","rdx","rsi","rdi",',
        '                 "r8","r9","r10","r11","r12","r13","r14","r15","rflags","cs","ss"]:',
        '        regs[name] = hex(safe_eval("$" + name))',
        '    return regs',
        '',
        'def collect_stack(count):',
        '    rsp = safe_eval("$rsp")',
        '    return [hex(read_u64(rsp + i*8)) for i in range(count)]',
        '',
        'def collect_mem(addr_expr, count):',
        '    addr = safe_eval(addr_expr) if isinstance(addr_expr, str) else addr_expr',
        '    bs = read_bytes(addr, count)',
        '    return {"addr": hex(addr), "hex": " ".join("%02x" % b for b in bs)}',
        '',
        'def collect_disasm(addr_expr, count):',
        '    addr = safe_eval(addr_expr) if isinstance(addr_expr, str) else addr_expr',
        '    try:',
        '        out = gdb.execute("x/" + str(count) + "i " + str(addr), to_string=True)',
        '        return out.strip().splitlines()',
        '    except: return []',
        '',
        'def collect_item(spec):',
        '    """Collect one item from a spec string."""',
        '    parts = spec.split()',
        '    cmd = parts[0]',
        '    if cmd == "regs":',
        '        return ("regs", collect_regs())',
        '    elif cmd == "stack":',
        '        n = int(parts[1]) if len(parts) > 1 else 8',
        '        return ("stack", collect_stack(n))',
        '    elif cmd == "mem":',
        '        addr = parts[1] if len(parts) > 1 else "$rsp"',
        '        n = int(parts[2]) if len(parts) > 2 else 32',
        '        return ("mem", collect_mem(addr, n))',
        '    elif cmd == "disasm":',
        '        addr = parts[1] if len(parts) > 1 else "$rip"',
        '        n = int(parts[2]) if len(parts) > 2 else 10',
        '        return ("disasm", collect_disasm(addr, n))',
        '    elif cmd == "expr":',
        '        expr = " ".join(parts[1:])',
        '        return ("expr:" + expr, hex(safe_eval(expr)))',
        '    elif cmd == "bt":',
        '        n = int(parts[1]) if len(parts) > 1 else 10',
        '        try:',
        '            out = gdb.execute("bt " + str(n), to_string=True)',
        '            return ("backtrace", out.strip().splitlines())',
        '        except: return ("backtrace", [])',
        '    elif cmd.startswith("x/"):',
        '        try:',
        '            out = gdb.execute(spec, to_string=True)',
        '            return ("raw:" + spec, out.strip().splitlines())',
        '        except: return ("raw:" + spec, "error")',
        '    else:',
        '        return ("unknown:" + spec, None)',
        '',
        '# --- Main investigation ---',
        'results = {"steps": [], "error": None}',
        '',
        'try:',
    ]

    for step_idx, step in enumerate(steps):
        name = step.get("name", f"step_{step_idx}")
        bp_cmd = step["break"]
        condition = step.get("condition")
        collect_specs = step.get("collect", ["regs"])
        then_action = step.get("then", "stop")
        max_hits = step.get("max_hits", 10000)

        lines.append(f'    # --- Step {step_idx}: {name} ---')
        lines.append(f'    gdb.execute("{bp_cmd}")')

        if condition:
            # Loop: continue until condition is met
            lines.append(f'    for _iter_{step_idx} in range({max_hits}):')
            lines.append(f'        gdb.execute("continue")')
            lines.append(f'        if safe_eval("{condition}"):')
            lines.append(f'            break')
            lines.append(f'    else:')
            lines.append(f'        results["steps"].append({{"name": "{name}", "error": "condition never met in {max_hits} hits"}})')
        else:
            lines.append(f'    gdb.execute("continue")')

        # Collect data
        lines.append(f'    step_data = {{"name": "{name}", "data": {{}}}}')
        for spec in collect_specs:
            lines.append(f'    key, val = collect_item("{spec}")')
            lines.append(f'    step_data["data"][key] = val')

        lines.append(f'    results["steps"].append(step_data)')

        if then_action == "delete-and-continue":
            lines.append(f'    gdb.execute("delete")')
        elif then_action == "stop":
            pass  # Don't continue — next step will set new breakpoint

    lines += [
        'except gdb.error as e:',
        '    results["error"] = str(e)',
        'except Exception as e:',
        '    results["error"] = traceback.format_exc()',
        '',
        f'with open("{output_file}", "w") as f:',
        '    json.dump(results, f, indent=2)',
        '',
        'gdb.execute("kill")',
        'gdb.execute("quit")',
    ]

    script.write_text("\n".join(lines))
    return script


def start_qemu(kernel_elf: Path, disk: str = None, mem_prealloc: bool = True) -> subprocess.Popen:
    cmd = [
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
        "-S",
        "-kernel", str(kernel_elf),
        "-netdev", "user,id=net0",
        "-device", "virtio-net-pci,netdev=net0,mac=52:54:00:12:34:56",
    ]
    if disk:
        cmd += [
            "-device", "virtio-blk-pci,drive=hd0",
            "-drive", f"file={disk},format=raw,if=none,id=hd0",
        ]
    if mem_prealloc:
        cmd += ["-mem-prealloc"]
    return subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


def run_gdb(script: Path, timeout: int) -> tuple:
    cmd = [
        "gdb", "-batch", "-nx",
        "-ex", "set pagination off",
        "-ex", f"target remote :{GDB_PORT}",
        "-ex", f"symbol-file {ROOT / 'kevlar.x64.elf'}",
        "-ex", f"source {script}",
    ]
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
        return result.returncode, result.stdout, result.stderr
    except subprocess.TimeoutExpired:
        return -1, "", "timeout"


def run_plan(plan: dict, args) -> dict:
    init_path = plan.get("init", args.init)
    patched = patch_kernel(init_path)
    output_file = Path(tempfile.mktemp(suffix=".json"))
    script = generate_gdb_script(plan, output_file)

    print(f"[*] Init: {init_path}")
    print(f"[*] GDB script: {script}")

    qemu = start_qemu(patched, args.disk)
    time.sleep(1.5)

    if qemu.poll() is not None:
        patched.unlink(missing_ok=True)
        return {"error": "QEMU failed to start"}

    print(f"[*] QEMU PID {qemu.pid}, running GDB (timeout={args.timeout}s)...")
    rc, stdout, stderr = run_gdb(script, args.timeout)

    qemu.kill()
    qemu.wait()
    patched.unlink(missing_ok=True)
    script.unlink(missing_ok=True)

    if rc == -1:
        print(f"[!] GDB timed out")

    for line in stderr.splitlines():
        if "error" in line.lower() and "warning" not in line.lower():
            print(f"    GDB: {line.strip()}")

    if output_file.exists():
        results = json.loads(output_file.read_text())
        output_file.unlink()
    else:
        results = {"error": f"No output (rc={rc})"}

    return results


def main():
    parser = argparse.ArgumentParser(description="GDB investigation runner for Kevlar")
    parser.add_argument("plan", help="Plan JSON file or inline JSON string")
    parser.add_argument("--disk", default=None, help="Disk image")
    parser.add_argument("--init", default="/bin/test-openssl-boot", help="Init binary")
    parser.add_argument("--timeout", type=int, default=120, help="GDB timeout (seconds)")
    parser.add_argument("--json", default=None, help="Write results to file")
    args = parser.parse_args()

    # Parse plan
    plan_arg = args.plan
    if os.path.isfile(plan_arg):
        plan = json.loads(Path(plan_arg).read_text())
    else:
        plan = json.loads(plan_arg)

    results = run_plan(plan, args)

    print("\n=== Results ===")
    print(json.dumps(results, indent=2))

    if args.json:
        Path(args.json).write_text(json.dumps(results, indent=2))
        print(f"\nWritten to {args.json}")

    return 0 if not results.get("error") else 1


if __name__ == "__main__":
    sys.exit(main())
