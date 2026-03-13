#!/usr/bin/env python3
import argparse
import signal
import shutil
import socket
import os
import subprocess
import sys
import platform

COMMON_ARGS = [
    "-serial",
    "mon:stdio",
    "-no-reboot",
]

# Ports used by QEMU for host forwarding.
FORWARDED_PORTS = [20022, 20080]

ARCHS = {
    "x64": {
        "bin":
        "qemu-system-x86_64",
        "args":
        COMMON_ARGS + [
            "-m",
            "1024",
            "-cpu",
            "Icelake-Server",
            "-device",
            "virtio-net,netdev=net0,disable-legacy=on,disable-modern=off",
            "-netdev",
            "user,id=net0,hostfwd=tcp:127.0.0.1:20022-:22,hostfwd=tcp:127.0.0.1:20080-:80",
            "-object",
            "filter-dump,id=fiter0,netdev=net0,file=virtio-net.pcap",
            "-device",
            "isa-debug-exit,iobase=0x501,iosize=2",
            "-d",
            "guest_errors,unimp",
        ]
    },
    "arm64": {
        "bin":
        "qemu-system-aarch64",
        "args":
        COMMON_ARGS + [
            "-machine",
            "virt",
            "-cpu",
            "cortex-a72",
            "-m",
            "1024",
            "-global",
            "virtio-mmio.force-legacy=false",
            "-device",
            "virtio-net-device,netdev=net0",
            "-netdev",
            "user,id=net0,hostfwd=tcp:127.0.0.1:20022-:22,hostfwd=tcp:127.0.0.1:20080-:80",
            "-d",
            "guest_errors,unimp",
        ]
    }
}


def kill_stale_qemu_on_ports(ports):
    """Kill any QEMU processes holding our forwarded ports."""
    is_windows = platform.system() == "Windows"

    for port in ports:
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            s.bind(("127.0.0.1", port))
            s.close()
        except OSError:
            s.close()
            # Port is in use. Try to find and kill the holder.
            try:
                if is_windows:
                    # Windows: use netstat to find the process
                    result = subprocess.run(
                        ["netstat", "-ano"],
                        capture_output=True, text=True,
                    )
                    for line in result.stdout.splitlines():
                        if f":{port}" in line and "LISTENING" in line:
                            parts = line.split()
                            if parts:
                                pid = int(parts[-1])
                                # Check if it's a QEMU process
                                try:
                                    tasklist = subprocess.run(
                                        ["tasklist", "/FI", f"PID eq {pid}", "/NH"],
                                        capture_output=True, text=True,
                                    )
                                    if "qemu" in tasklist.stdout.lower():
                                        print(
                                            f"run-qemu.py: killing stale QEMU (pid={pid}) "
                                            f"holding port {port}",
                                            file=sys.stderr,
                                        )
                                        subprocess.run(["taskkill", "/F", "/PID", str(pid)], check=False)
                                except Exception:
                                    pass
                else:
                    # Linux: use ss command
                    result = subprocess.run(
                        ["ss", "-tlnp", f"sport = :{port}"],
                        capture_output=True, text=True,
                    )
                    for line in result.stdout.splitlines():
                        if "qemu" in line:
                            # Extract pid from users:(("qemu-...",pid=12345,fd=10))
                            import re
                            m = re.search(r"pid=(\d+)", line)
                            if m:
                                pid = int(m.group(1))
                                print(
                                    f"run-qemu.py: killing stale QEMU (pid={pid}) "
                                    f"holding port {port}",
                                    file=sys.stderr,
                                )
                                os.kill(pid, signal.SIGTERM)
            except Exception:
                pass
    # Brief wait for ports to free up.
    import time
    time.sleep(0.5)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--arch", choices=["x64", "arm64"])
    parser.add_argument("--gui", action="store_true")
    parser.add_argument("--gdb", action="store_true")
    parser.add_argument("--kvm", action="store_true")
    parser.add_argument("--append-cmdline", action="append")
    parser.add_argument("--disk", help="VirtIO block device disk image file")
    parser.add_argument("--log-serial")
    parser.add_argument("--save-dump", metavar="FILE",
                        help="Intercept serial output and save any crash dump to FILE")
    parser.add_argument("--qemu")
    parser.add_argument("kernel_elf", help="The kernel ELF executable.")
    parser.add_argument("qemu_args", nargs="*")
    args = parser.parse_args()

    # Kill any stale QEMU sessions that might be holding our ports.
    kill_stale_qemu_on_ports(FORWARDED_PORTS)

    kernel_path_arg = args.kernel_elf

    # For x64: QEMU's multiboot loader requires an EM_386 ELF.  The bzImage flat
    # binary (kevlar.x64.img) uses the Linux/x86 Boot Protocol which goes through
    # SeaBIOS's linuxboot.rom option ROM — that path is unreliable in QEMU 10.x
    # (the ROM loads the kernel but never jumps to code32_start).  Instead we
    # patch e_machine from EM_X86_64 (0x003E) to EM_386 (0x0003) in a temp copy
    # of the ELF so QEMU uses its built-in multiboot loader, which works
    # reliably.  The bzImage is still produced by the build for real hardware
    # (GRUB2, SYSLINUX, UEFI Linux EFI stub).
    tmp_elf_path = None
    if args.arch == "x64":
        import tempfile
        with open(kernel_path_arg, 'rb') as f:
            elf_data = bytearray(f.read())
        elf_data[18] = 0x03  # e_machine low byte: EM_386
        elf_data[19] = 0x00  # e_machine high byte
        tmp_fd, tmp_elf_path = tempfile.mkstemp(suffix=".elf")
        try:
            os.write(tmp_fd, elf_data)
        finally:
            os.close(tmp_fd)
        kernel_path_arg = tmp_elf_path

    qemu = ARCHS[args.arch]
    if args.qemu:
        qemu_bin = args.qemu
    else:
        qemu_bin = qemu["bin"]
        # On Windows, try multiple detection methods
        if platform.system() == "Windows":
            # 1. Check QEMU_PATH environment variable
            if "QEMU_PATH" in os.environ and os.path.exists(os.environ["QEMU_PATH"]):
                qemu_bin = os.environ["QEMU_PATH"]
            else:
                # 2. Try shutil.which (searches PATH)
                which_qemu = shutil.which(qemu["bin"])
                if which_qemu:
                    qemu_bin = which_qemu
                else:
                    # 3. Check common install locations
                    common_paths = [
                        rf"C:\Program Files\qemu\{qemu['bin']}.exe",
                        rf"C:\qemu\{qemu['bin']}.exe",
                    ]
                    for path in common_paths:
                        if os.path.exists(path):
                            qemu_bin = path
                            break

    # On Windows, convert paths to forward slashes for QEMU
    kernel_path = kernel_path_arg.replace('\\', '/') if platform.system() == "Windows" else kernel_path_arg

    argv = [qemu_bin] + qemu["args"] + ["-kernel", kernel_path]
    cmdline = []
    if not args.gui:
        argv += ["-nographic"]
    if args.gdb:
        argv += ["-gdb", "tcp::7789", "-S"]
    if args.kvm:
        argv += ["-accel", "kvm"]
    if args.disk:
        disk_path = args.disk.replace('\\', '/') if platform.system() == "Windows" else args.disk
        if args.arch == "arm64":
            argv += ["-drive", f"file={disk_path},format=raw,if=none,id=drive0",
                     "-device", "virtio-blk-device,drive=drive0"]
        else:
            argv += ["-drive", f"file={disk_path},format=raw,if=virtio"]
    if args.append_cmdline:
        cmdline += args.append_cmdline
    if args.log_serial:
        argv += ["-serial", args.log_serial]
        cmdline += ["serial1=on"]
    if args.qemu_args:
        argv += args.qemu_args

    if cmdline:
        argv += ["-append", " ".join(cmdline)]

    # Windows doesn't support preexec_fn with os.setsid
    is_windows = platform.system() == "Windows"

    # When --save-dump is given, intercept stdout to detect and save the
    # base64-encoded crash dump emitted by the panic handler.
    if args.save_dump:
        import base64
        import threading

        popen_kwargs = {"stdout": subprocess.PIPE, "stderr": subprocess.STDOUT}
        if not is_windows:
            popen_kwargs["preexec_fn"] = os.setsid
        p = subprocess.Popen(argv, **popen_kwargs)

        dump_lines = []
        capturing = False
        saved = False

        def _intercept_stdout():
            nonlocal capturing, saved
            for raw in p.stdout:
                sys.stdout.buffer.write(raw)
                sys.stdout.buffer.flush()
                line = raw.decode("utf-8", errors="replace").rstrip("\r\n")
                if line == "===KEVLAR_CRASH_DUMP_BEGIN===":
                    capturing = True
                    dump_lines.clear()
                elif line == "===KEVLAR_CRASH_DUMP_END===":
                    capturing = False
                    try:
                        data = base64.b64decode("".join(dump_lines))
                        with open(args.save_dump, "wb") as f:
                            f.write(data)
                        print(f"\nrun-qemu.py: crash dump saved to {args.save_dump} "
                              f"({len(data)} bytes)", file=sys.stderr)
                        saved = True
                    except Exception as e:
                        print(f"\nrun-qemu.py: failed to decode crash dump: {e}",
                              file=sys.stderr)
                elif capturing:
                    dump_lines.append(line)

        t = threading.Thread(target=_intercept_stdout, daemon=True)
        t.start()
    else:
        if is_windows:
            p = subprocess.Popen(argv)
        else:
            p = subprocess.Popen(argv, preexec_fn=os.setsid)

    def _forward_signal(signum, _frame):
        """Forward signal to QEMU's process group so it shuts down cleanly."""
        try:
            if is_windows:
                # Windows: just terminate the process
                p.terminate()
            else:
                # Unix: kill the entire process group
                os.killpg(p.pid, signum)
        except (ProcessLookupError, OSError):
            pass

    signal.signal(signal.SIGTERM, _forward_signal)
    signal.signal(signal.SIGINT, _forward_signal)

    try:
        p.wait()
        if args.save_dump:
            t.join(timeout=5)
    finally:
        if tmp_elf_path:
            try:
                os.unlink(tmp_elf_path)
            except OSError:
                pass

    if p.returncode != 33:
        sys.exit(
            f"\nrun-qemu.py: qemu exited with failure status (status={p.returncode})"
        )


if __name__ == "__main__":
    main()
