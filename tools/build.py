#!/usr/bin/env python3
"""
Cross-platform build script for Kevlar.

Handles all platform-specific logic in one place:
- Tool detection (cargo, qemu, docker, llvm)
- Path handling (Windows vs Unix)
- Build configurations
- Environment setup

Usage:
    python tools/build.py [--arch x64|arm64] [--release] [--profile PROFILE]
    python tools/build.py clean
    python tools/build.py check
"""

import argparse
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path


class BuildEnv:
    """Cross-platform build environment detector."""

    def __init__(self):
        self.is_windows = platform.system() == "Windows"
        self.is_linux = platform.system() == "Linux"
        self.is_macos = platform.system() == "Darwin"
        self.root_dir = Path(__file__).parent.parent.resolve()

    def find_tool(self, name, fallback_paths=None):
        """Find a tool in PATH or fallback locations."""
        # Try PATH first
        tool = shutil.which(name)
        if tool:
            return tool

        # Try fallback paths
        if fallback_paths:
            for path in fallback_paths:
                if os.path.exists(path):
                    return path

        return None

    def detect_cargo(self):
        """Detect cargo executable."""
        if self.is_windows:
            fallbacks = [
                str(Path.home() / ".cargo" / "bin" / "cargo.exe"),
                r"C:\Users\{}\AppData\Local\Programs\Rust\cargo\bin\cargo.exe".format(os.environ.get("USERNAME", ""))
            ]
            return self.find_tool("cargo.exe", fallbacks) or self.find_tool("cargo")
        else:
            return self.find_tool("cargo", [str(Path.home() / ".cargo" / "bin" / "cargo")])

    def detect_qemu(self, arch="x64"):
        """Detect QEMU executable for given architecture."""
        qemu_bin = "qemu-system-x86_64" if arch == "x64" else "qemu-system-aarch64"

        if self.is_windows:
            fallbacks = [
                rf"C:\Program Files\qemu\{qemu_bin}.exe",
                rf"C:\qemu\{qemu_bin}.exe",
            ]
            return self.find_tool(f"{qemu_bin}.exe", fallbacks) or self.find_tool(qemu_bin)
        else:
            return self.find_tool(qemu_bin)

    def detect_docker(self):
        """Detect docker executable."""
        if self.is_windows:
            fallbacks = [
                r"C:\Program Files\Docker\Docker\resources\bin\docker.exe",
            ]
            return self.find_tool("docker.exe", fallbacks) or self.find_tool("docker")
        else:
            return self.find_tool("docker")

    def detect_rustc_sysroot(self):
        """Detect rustc sysroot for LLVM tools."""
        rustc = self.find_tool("rustc")
        if not rustc:
            if self.is_windows:
                rustc = str(Path.home() / ".cargo" / "bin" / "rustc.exe")

        if rustc and os.path.exists(rustc):
            try:
                result = subprocess.run(
                    [rustc, "--print", "sysroot"],
                    capture_output=True,
                    text=True,
                    check=True
                )
                return result.stdout.strip()
            except subprocess.CalledProcessError:
                pass
        return None

    def get_llvm_bin_dir(self):
        """Get LLVM bin directory from rustc sysroot."""
        sysroot = self.detect_rustc_sysroot()
        if not sysroot:
            return None

        if self.is_windows:
            return str(Path(sysroot) / "lib" / "rustlib" / "x86_64-pc-windows-msvc" / "bin")
        else:
            # Linux/macOS might have different paths
            return str(Path(sysroot) / "lib" / "rustlib" / "x86_64-unknown-linux-gnu" / "bin")

    def setup_env(self, args):
        """Setup environment variables for build."""
        env = os.environ.copy()

        # Disable MSYS path conversion on Windows (affects Git Bash)
        if self.is_windows:
            env["MSYS_NO_PATHCONV"] = "1"
            env["MSYS2_ARG_CONV_EXCL"] = "*"

        # Set INIT_SCRIPT
        if args.init_script:
            env["INIT_SCRIPT"] = args.init_script
        elif "INIT_SCRIPT" not in env:
            env["INIT_SCRIPT"] = "/bin/sh"

        # Set architecture
        env["ARCH"] = args.arch

        # Set profile
        env["PROFILE"] = args.profile

        # Set build mode
        if args.release:
            env["RELEASE"] = "1"

        # Set QEMU path if found
        qemu = self.detect_qemu(args.arch)
        if qemu:
            env["QEMU_PATH"] = qemu

        # Add LLVM tools to PATH if found
        llvm_bin = self.get_llvm_bin_dir()
        if llvm_bin and os.path.exists(llvm_bin):
            env["PATH"] = llvm_bin + os.pathsep + env.get("PATH", "")

        return env


def run_cargo_build(env_obj, args):
    """Run cargo build with proper environment."""
    cargo = env_obj.detect_cargo()
    if not cargo:
        print("Error: cargo not found", file=sys.stderr)
        return 1

    env = env_obj.setup_env(args)

    # Determine target spec
    if args.profile in ["fortress", "balanced"]:
        target_spec = f"kernel/arch/{args.arch}/{args.arch}-unwind.json"
    else:
        target_spec = f"kernel/arch/{args.arch}/{args.arch}.json"

    # For now, just use make to build everything (it handles initramfs)
    # This is simpler than reimplementing all the initramfs logic
    print(f"Building Kevlar ({args.profile} profile, {args.arch})...", file=sys.stderr)

    make_cmd = ["make"]
    if env_obj.is_windows:
        # On Windows, make might be in Git Bash or other locations
        make_tool = env_obj.find_tool("make")
        if make_tool:
            make_cmd = [make_tool]

    result = subprocess.run(make_cmd, env=env, cwd=env_obj.root_dir)
    if result.returncode != 0:
        return result.returncode

    return 0

    kernel_elf = env_obj.root_dir / f"kevlar.{args.arch}.elf"
    print(f"Kernel built: {kernel_elf}", file=sys.stderr)
    return 0


def run_cargo_check(env_obj, args):
    """Run cargo check."""
    cargo = env_obj.detect_cargo()
    if not cargo:
        print("Error: cargo not found", file=sys.stderr)
        return 1

    env = env_obj.setup_env(args)

    # Determine target spec
    if args.profile in ["fortress", "balanced"]:
        target_spec = f"kernel/arch/{args.arch}/{args.arch}-unwind.json"
    else:
        target_spec = f"kernel/arch/{args.arch}/{args.arch}.json"

    print(f"Type-checking kernel ({args.arch})...", file=sys.stderr)

    cargo_cmd = [
        cargo,
        "check",
        "--package", "kevlar_kernel",
        "--target", target_spec,
    ]

    result = subprocess.run(cargo_cmd, env=env, cwd=env_obj.root_dir)
    return result.returncode


def run_clean(env_obj):
    """Clean build artifacts."""
    cargo = env_obj.detect_cargo()
    if cargo:
        subprocess.run([cargo, "clean"], cwd=env_obj.root_dir)

    # Clean build directory
    build_dir = env_obj.root_dir / "build"
    if build_dir.exists():
        shutil.rmtree(build_dir)

    # Clean kernel ELF files
    for pattern in ["kevlar.*.elf", "kevlar.*.symbols", "kevlar.*.stripped.elf"]:
        for f in env_obj.root_dir.glob(pattern):
            f.unlink()

    print("Clean complete", file=sys.stderr)
    return 0


def main():
    parser = argparse.ArgumentParser(description="Cross-platform Kevlar build system")
    parser.add_argument("command", nargs="?", default="build",
                       choices=["build", "check", "clean"],
                       help="Build command (default: build)")
    parser.add_argument("--arch", default="x64", choices=["x64", "arm64"],
                       help="Target architecture (default: x64)")
    parser.add_argument("--release", action="store_true",
                       help="Build in release mode")
    parser.add_argument("--profile", default="balanced",
                       choices=["fortress", "balanced", "performance", "ludicrous"],
                       help="Safety profile (default: balanced)")
    parser.add_argument("--init-script",
                       help="Init script to run on boot (default: /bin/sh)")

    args = parser.parse_args()

    # Override from environment if set
    if "ARCH" in os.environ:
        args.arch = os.environ["ARCH"]
    if "PROFILE" in os.environ:
        args.profile = os.environ["PROFILE"]
    if "RELEASE" in os.environ:
        args.release = True
    if "INIT_SCRIPT" in os.environ:
        args.init_script = os.environ["INIT_SCRIPT"]

    env_obj = BuildEnv()

    if args.command == "build":
        return run_cargo_build(env_obj, args)
    elif args.command == "check":
        return run_cargo_check(env_obj, args)
    elif args.command == "clean":
        return run_clean(env_obj)

    return 0


if __name__ == "__main__":
    sys.exit(main())
