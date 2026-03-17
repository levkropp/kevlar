# Building Kevlar

This guide covers building Kevlar on different platforms.

## Prerequisites

### All Platforms

- Rust nightly toolchain
- Python 3.8+
- QEMU (for running/testing)

```bash
rustup install nightly
rustup override set nightly
rustup component add llvm-tools-preview rust-src
```

### Linux

Native build without Docker:

```bash
# Arch Linux
pacman -S musl gcc e2fsprogs

# Ubuntu/Debian
apt install musl-tools build-essential linux-libc-dev e2fsprogs
```

### Windows

Kevlar uses WSL2 (Windows Subsystem for Linux) for building on Windows. The Makefile automatically detects Windows and uses WSL.

**IMPORTANT:** Always use the root user in WSL, not `sudo`. WSL's root user doesn't require password prompts and works better with the build system.

#### Setup Steps

1. Install WSL2 (if not already installed):
   ```powershell
   wsl --install
   ```

2. Install build dependencies in WSL as root:
   ```powershell
   wsl -u root apt update
   wsl -u root apt install -y musl-tools build-essential linux-libc-dev e2fsprogs
   ```

3. Build from Windows terminal (PowerShell, CMD, or Git Bash):
   ```bash
   make build
   ```

The Makefile will automatically use WSL for build steps that require Linux tools.

#### Troubleshooting

- If you get "sudo" prompts or permission errors, always use `wsl -u root` instead of `wsl sudo`
- Make sure Docker Desktop is **not** running if you don't want to use Docker (native WSL build is faster)
- The build process converts Windows paths to WSL paths automatically

**WSL Networking Issues:**

If the build fails with network timeouts when downloading BusyBox or other dependencies, this is a common WSL networking issue. Workarounds:

1. Use an existing `build/testing.initramfs` if available:
   ```bash
   touch build/testing.initramfs
   make build
   ```

2. Fix WSL networking (requires Administrator PowerShell):
   ```powershell
   # Restart WSL
   wsl --shutdown
   # Check DNS resolution
   wsl -u root cat /etc/resolv.conf
   ```

3. If network issues persist, you may need to configure WSL networking in `C:\Users\<username>\.wslconfig`:
   ```ini
   [wsl2]
   networkingMode=mirrored
   ```

## Basic Build Commands

```bash
# Build kernel
make build

# Build and run in QEMU (x86_64)
make run

# Build for ARM64
make ARCH=arm64 build

# Build with release optimizations
make RELEASE=1 build

# Clean build artifacts
make clean
```

## Build Profiles

Kevlar supports four safety profiles:

- `fortress` (default) - Maximum safety checks
- `balanced` - Good balance of safety and performance
- `performance` - Fewer runtime checks
- `ludicrous` - Minimal checks, maximum speed

```bash
make PROFILE=performance build
```

## Running Tests

```bash
# Unit tests
make test-unit

# Integration tests
make test-integration

# Full test suite
make test
```

## Build System Architecture

The build system works differently on Windows vs Linux:

**Linux:** Uses native `musl-gcc` and Python build scripts directly

**Windows:**
1. Makefile runs in Git Bash/PowerShell/CMD
2. Detects `OS=Windows_NT`
3. Automatically invokes WSL for Linux-specific tools
4. Uses LLVM tools from Rust toolchain for `nm`, `strip`, `objcopy`

The Windows path contains spaces (`C:\Program Files\...`), so all tool paths are quoted in the Makefile.
