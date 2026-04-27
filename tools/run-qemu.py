#!/usr/bin/env python3
import argparse
import signal
import shutil
import socket
import os
import subprocess
import sys
import platform
import threading
import time

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
            # Expose a `ramfb` display device.  Setup is done by
            # `exts/ramfb` from the guest, which writes fb metadata
            # into the QEMU fw_cfg `etc/ramfb` file.  When `-display`
            # is anything other than `none`, QEMU scans out the guest
            # framebuffer.  In `--batch` mode we still pass `-display
            # vnc=:0 -vga none` if the user opts in via --display-vnc.
            "-device",
            "ramfb",
            # virtio-{keyboard,tablet}-device: virtio-mmio input
            # devices that `exts/virtio_input` discovers via the DTB
            # walker and exposes at /dev/input/event0 (keyboard) and
            # /dev/input/event1 (tablet — absolute coords map cleanly
            # from the VNC client without a "first move to register"
            # step that virtio-mouse would need).  Disable event_idx
            # and indirect_desc so the wire protocol stays in plain
            # mode; Kevlar's `Virtio` driver doesn't negotiate either
            # feature.
            "-device",
            "virtio-keyboard-device,event_idx=off,indirect_desc=off",
            "-device",
            "virtio-mouse-device,event_idx=off,indirect_desc=off",
            "-d",
            "guest_errors,unimp",
        ]
    }
}


def _qmp_inject_nmi(sock_path):
    """Send an `inject-nmi` QMP command to the QEMU monitor at sock_path.

    QMP is JSON-over-Unix-socket, one JSON object per line.  Tiny handshake:
    read greeting, send qmp_capabilities, send inject-nmi.  No external deps.
    """
    import json
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(2.0)
    try:
        s.connect(sock_path)
        f = s.makefile("rwb")
        _ = f.readline()  # QMP greeting line
        f.write(b'{"execute":"qmp_capabilities"}\n'); f.flush()
        _ = f.readline()
        f.write(b'{"execute":"inject-nmi"}\n'); f.flush()
        resp = f.readline()
        # Best-effort validation: response should contain "return".
        try:
            obj = json.loads(resp)
            if "return" not in obj:
                raise RuntimeError(f"QMP returned: {resp!r}")
        except json.JSONDecodeError:
            pass
    finally:
        s.close()


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
    parser.add_argument("--display-vnc", metavar="PORT", type=int, default=None,
                        help="When set, replace -nographic with `-display vnc=:PORT` "
                             "so QEMU's display backend stays alive in batch mode. "
                             "Required for the ramfb scan-out path on arm64 to be "
                             "visible during a `--batch` run.  Connect with a VNC "
                             "client at 127.0.0.1:(5900+PORT).")
    parser.add_argument("--gdb", action="store_true")
    parser.add_argument("--kvm", action="store_true")
    parser.add_argument("--ktrace", metavar="FILE", nargs="?", const="ktrace.bin",
                        help="Enable debugcon ktrace output (default: ktrace.bin)")
    parser.add_argument("--append-cmdline", action="append")
    parser.add_argument("--disk", help="VirtIO block device disk image file")
    parser.add_argument("--log-serial")
    parser.add_argument("--batch", action="store_true",
                        help="Use plain stdio serial (no monitor). Works with pipes.")
    parser.add_argument("--save-dump", metavar="FILE",
                        help="Intercept serial output and save any crash dump to FILE")
    parser.add_argument("--nmi-on-stall", metavar="SECONDS", type=float, default=None,
                        help="Inject NMI via QMP after SECONDS of serial silence. "
                             "Implies --batch. The guest's NMI handler then dumps "
                             "per-CPU state (RIP, registers, backtrace, syscall "
                             "latency histogram). Useful for livelocks that the "
                             "kernel's internal LAPIC-HB watchdog can't detect.")
    parser.add_argument("--qemu")
    parser.add_argument("--timeout", metavar="SECONDS", type=float, default=None,
                        help="Hard kill QEMU after SECONDS. Portable replacement "
                             "for `timeout(1)` (which is missing on macOS by "
                             "default). Sends SIGTERM, then SIGKILL after a "
                             "5s grace period.")
    parser.add_argument("kernel_elf", help="The kernel ELF executable.")
    parser.add_argument("qemu_args", nargs="*")
    args = parser.parse_args()

    # Allow GDB attachment via env var (so test targets can pick it up
    # without per-target Makefile changes). Set KEVLAR_GDB=1 to enable.
    if os.environ.get("KEVLAR_GDB") == "1":
        args.gdb = True

    # --nmi-on-stall needs stdout ownership for silence detection AND a
    # QMP socket to inject the NMI.  Auto-imply --batch so stdio routing
    # is deterministic (no mon:stdio multiplexer).
    if args.nmi_on_stall is not None:
        if not args.batch:
            print("run-qemu.py: --nmi-on-stall implies --batch",
                  file=sys.stderr)
            args.batch = True
        if args.gui:
            sys.exit("run-qemu.py: --nmi-on-stall is incompatible with --gui")

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
    if args.batch:
        # Replace mon:stdio with plain stdio serial for non-interactive use.
        argv = [a for a in argv if a not in ("mon:stdio",)]
        # Remove the "-serial" before where "mon:stdio" was
        new_argv = []
        skip = False
        for a in argv:
            if a == "-serial":
                skip = True
                continue
            if skip:
                skip = False
                continue
            new_argv.append(a)
        argv = new_argv
        argv += ["-serial", "stdio", "-monitor", "none"]
    cmdline = []
    if args.display_vnc is not None:
        # VNC backend keeps the QEMU display device alive even in
        # batch mode (no SDL/cocoa needed).  Combined with `-device
        # ramfb` (in the arm64 default args) this lets ramfb scan
        # out the guest's /dev/fb0 backing memory to a remote VNC
        # client.  `-vga none` suppresses any default VGA the
        # machine model would otherwise pull in.
        argv += ["-display", f"vnc=:{args.display_vnc}", "-vga", "none"]
    elif not args.gui:
        argv += ["-nographic"]
    if args.gdb:
        argv += ["-gdb", "tcp::7789", "-S"]
    if args.kvm:
        # --kvm selects the native-speed hypervisor for the host/guest pairing:
        #   Linux x64/arm64 + matching guest → KVM
        #   macOS arm64 + arm64 guest       → Hypervisor.framework (hvf)
        #   macOS x64 + x64 guest           → hvf
        # Fall back to kvm label if the host isn't macOS so CI/Linux flows stay
        # unchanged.
        if platform.system() == "Darwin":
            host_is_arm64 = platform.machine() in ("arm64", "aarch64")
            host_arch_for_qemu = "arm64" if host_is_arm64 else "x64"
            if args.arch != host_arch_for_qemu:
                # Cross-arch on macOS — HVF only accelerates same-arch guests.
                # Fall back to TCG (software emulation).  Slower but works.
                print(f"run-qemu.py: --kvm with cross-arch macOS host "
                      f"(host={host_arch_for_qemu}, guest={args.arch}); "
                      f"falling back to TCG", file=sys.stderr)
                argv += ["-accel", "tcg"]
            else:
                argv += ["-accel", "hvf"]
                # HVF requires -cpu host (it cannot emulate a different ARM CPU
                # model).  Replace the default -cpu cortex-a72 if present.
                if args.arch == "arm64":
                    new_argv = []
                    skip = False
                    for a in argv:
                        if a == "-cpu":
                            skip = True
                            continue
                        if skip:
                            skip = False
                            continue
                        new_argv.append(a)
                    argv = new_argv + ["-cpu", "host"]
        else:
            argv += ["-accel", "kvm"]
    if args.disk:
        disk_path = args.disk.replace('\\', '/') if platform.system() == "Windows" else args.disk
        # cache=writeback (default) keeps host writes in the page cache
        # until the host OS schedules them out, OR until QEMU exits and
        # closes the file.  Under SIGKILL (the wrapper's fallback after
        # SIGTERM timeout) writes that haven't been issued yet are lost,
        # producing 0-byte files when extracted via debugfs.
        # cache=writethrough (synchronous fsync per write) is safer for
        # off-host extraction and the perf cost is irrelevant for our
        # test harness.
        if args.arch == "arm64":
            argv += ["-drive",
                     f"file={disk_path},format=raw,if=none,id=drive0,cache=writethrough",
                     "-device", "virtio-blk-device,drive=drive0"]
        else:
            argv += ["-drive", f"file={disk_path},format=raw,if=virtio,cache=writethrough"]
    if args.ktrace:
        ktrace_path = args.ktrace
        if args.arch == "arm64":
            # ARM64: semihosting SYS_WRITE trap → chardev file.
            # One HLT #0xF000 per ring-buffer dump (single trap for the full
            # 256 KB slice — equivalent throughput to ISA debugcon on x86_64).
            argv += [
                "-chardev", f"file,id=ktrace,path={ktrace_path}",
                "-semihosting-config", "enable=on,target=native,chardev=ktrace",
            ]
            print(f"\x1b[36mktrace: ARM64 semihosting → {ktrace_path}\x1b[0m",
                  file=sys.stderr)
        else:
            # x86_64: ISA debugcon device — writes to host file at ~5 MB/s on KVM.
            argv += [
                "-chardev", f"file,id=ktrace,path={ktrace_path}",
                "-device", "isa-debugcon,chardev=ktrace,iobase=0xe9",
            ]
            print(f"\x1b[36mktrace: ISA debugcon → {ktrace_path}\x1b[0m",
                  file=sys.stderr)
        cmdline.append("debug=ktrace")
    if args.append_cmdline:
        cmdline += args.append_cmdline
    if args.log_serial:
        # Add a second serial port that writes to a file.
        argv += ["-serial", f"file:{args.log_serial}"]
    qmp_sock_path: "str | None" = None
    if args.nmi_on_stall is not None:
        qmp_sock_path = f"/tmp/kevlar-qmp-{os.getpid()}.sock"
        # Clean up any stale socket (e.g. from a prior crashed run).
        try:
            os.unlink(qmp_sock_path)
        except OSError:
            pass
        argv += ["-qmp", f"unix:{qmp_sock_path},server=on,wait=off"]
    if args.qemu_args:
        argv += args.qemu_args

    if cmdline:
        argv += ["-append", " ".join(cmdline)]

    # Print exit hint for interactive sessions.
    if sys.stdout.isatty():
        print("\x1b[36mPress Ctrl-A X to exit QEMU\x1b[0m", file=sys.stderr)

    # Windows doesn't support preexec_fn with os.setsid
    is_windows = platform.system() == "Windows"

    # When --save-dump or --nmi-on-stall is given, intercept stdout both
    # to detect the crash-dump sentinels AND to maintain a last-output
    # timestamp for the stall monitor.
    need_intercept = bool(args.save_dump or args.nmi_on_stall is not None)
    t = None
    stall_t = None
    if need_intercept:
        import base64

        popen_kwargs = {"stdout": subprocess.PIPE, "stderr": subprocess.STDOUT}
        if not is_windows:
            popen_kwargs["preexec_fn"] = os.setsid
        p = subprocess.Popen(argv, **popen_kwargs)

        dump_lines = []
        capturing = False
        saved = False
        # Shared state for stall monitor.
        last_out_lock = threading.Lock()
        last_out_monotonic = [time.monotonic()]
        intercept_running = [True]

        def _intercept_stdout():
            nonlocal capturing, saved
            line_buf = b""
            while True:
                chunk = p.stdout.read1(4096) if hasattr(p.stdout, 'read1') else p.stdout.read(1)
                if not chunk:
                    break
                # Pass through immediately (unbuffered).
                sys.stdout.buffer.write(chunk)
                sys.stdout.buffer.flush()
                # Bump the stall-monitor timestamp on any output.
                with last_out_lock:
                    last_out_monotonic[0] = time.monotonic()
                # Accumulate for crash dump detection.
                if args.save_dump:
                    line_buf += chunk
                    while b"\n" in line_buf:
                        raw_line, line_buf = line_buf.split(b"\n", 1)
                        line = raw_line.decode("utf-8", errors="replace").rstrip("\r")
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
            intercept_running[0] = False

        t = threading.Thread(target=_intercept_stdout, daemon=True)
        t.start()

        if args.nmi_on_stall is not None and qmp_sock_path is not None:
            timeout_secs = args.nmi_on_stall

            def _stall_monitor():
                # Wait for QEMU to create the socket (it's created during
                # startup once QMP is ready).
                deadline = time.monotonic() + 10.0
                while intercept_running[0] and time.monotonic() < deadline:
                    if os.path.exists(qmp_sock_path):
                        break
                    time.sleep(0.05)
                else:
                    # QEMU never opened the QMP socket; give up silently.
                    return
                # Initial baseline: reset last_out to now so startup
                # latency doesn't count as "silence".
                with last_out_lock:
                    last_out_monotonic[0] = time.monotonic()

                while intercept_running[0]:
                    time.sleep(0.5)
                    with last_out_lock:
                        silent = time.monotonic() - last_out_monotonic[0]
                    if silent >= timeout_secs:
                        try:
                            _qmp_inject_nmi(qmp_sock_path)
                            print(f"\nrun-qemu.py: injected NMI after "
                                  f"{silent:.1f}s of silence",
                                  file=sys.stderr)
                        except Exception as e:
                            print(f"\nrun-qemu.py: QMP NMI inject failed: {e}",
                                  file=sys.stderr)
                        # Re-arm — avoid NMI storm while the handler's
                        # output drains.
                        with last_out_lock:
                            last_out_monotonic[0] = time.monotonic()

            stall_t = threading.Thread(target=_stall_monitor, daemon=True)
            stall_t.start()
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

    deadline_t = None
    if args.timeout is not None:
        def _deadline_kill():
            time.sleep(args.timeout)
            if p.poll() is None:
                print(f"\nrun-qemu.py: --timeout {args.timeout}s reached, "
                      f"terminating QEMU", file=sys.stderr)
                _forward_signal(signal.SIGTERM, None)
                # Grace period before SIGKILL.
                time.sleep(5)
                if p.poll() is None:
                    print("run-qemu.py: SIGTERM ignored, sending SIGKILL",
                          file=sys.stderr)
                    _forward_signal(signal.SIGKILL, None)
        deadline_t = threading.Thread(target=_deadline_kill, daemon=True)
        deadline_t.start()

    try:
        p.wait()
        if t is not None:
            t.join(timeout=5)
        if stall_t is not None:
            stall_t.join(timeout=2)
    finally:
        if tmp_elf_path:
            try:
                os.unlink(tmp_elf_path)
            except OSError:
                pass
        if qmp_sock_path is not None:
            try:
                os.unlink(qmp_sock_path)
            except OSError:
                pass

    # Recognized clean-exit codes:
    #   33  — x86_64 isa-debug-exit Success ((0x10 << 1) | 1)
    #   0   — arm64 PSCI SYSTEM_OFF (carries no exit code) and QEMU's own
    #         clean shutdown after a graceful guest halt.
    if p.returncode not in (0, 33):
        sys.exit(
            f"\nrun-qemu.py: qemu exited with failure status (status={p.returncode})"
        )


if __name__ == "__main__":
    main()
