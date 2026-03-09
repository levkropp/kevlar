#!/usr/bin/env python3
import argparse
import signal
import shutil
import socket
from tempfile import NamedTemporaryFile
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
    parser.add_argument("--log-serial")
    parser.add_argument("--qemu")
    parser.add_argument("kernel_elf", help="The kernel ELF executable.")
    parser.add_argument("qemu_args", nargs="*")
    args = parser.parse_args()

    # Kill any stale QEMU sessions that might be holding our ports.
    kill_stale_qemu_on_ports(FORWARDED_PORTS)

    if args.arch == "x64":
        #  Because QEMU denies a x86_64 multiboot ELF file (GRUB2 accept it, btw),
        #  modify `em_machine` to pretend to be an x86 (32-bit) ELF image,
        #
        #  https://github.com/qemu/qemu/blob/950c4e6c94b15cd0d8b63891dddd7a8dbf458e6a/hw/i386/multiboot.c#L197
        # Set EM_386 (0x0003) to em_machine.
        # On Windows, use delete=False to avoid permission issues
        elf = NamedTemporaryFile(delete=False)
        shutil.copyfileobj(open(args.kernel_elf, "rb"), elf.file)
        elf.seek(18)
        elf.write(bytes([0x03, 0x00]))
        elf.flush()
        elf.close()  # Close before QEMU opens it (important on Windows)
        kernel_elf = elf.name
    else:
        kernel_elf = args.kernel_elf

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
    kernel_path = kernel_elf.replace('\\', '/') if platform.system() == "Windows" else kernel_elf

    argv = [qemu_bin] + qemu["args"] + ["-kernel", kernel_path]
    cmdline = []
    if not args.gui:
        argv += ["-nographic"]
    if args.gdb:
        argv += ["-gdb", "tcp::7789", "-S"]
    if args.kvm:
        argv += ["-accel", "kvm"]
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

    p.wait()

    # Clean up temp file if we created one (multiboot mode on x64)
    if args.arch == "x64":
        try:
            os.unlink(kernel_elf)
        except OSError:
            pass

    if p.returncode != 33:
        sys.exit(
            f"\nrun-qemu.py: qemu exited with failure status (status={p.returncode})"
        )


if __name__ == "__main__":
    main()
