# Building Kevlar on Windows: A Complete Guide

This document summarizes the work done to enable full Windows build support for Kevlar, a permissively-licensed Rust kernel that runs Linux binaries.

## Overview

Kevlar is traditionally built on Linux/macOS, but we've now added complete Windows support using Git Bash (MINGW64), enabling kernel development on Windows workstations. This work makes Kevlar one of the few kernel projects with first-class Windows build support.

## Key Achievements

- ✅ Complete build system works on Windows 11
- ✅ Docker-based initramfs creation
- ✅ LLVM toolchain integration
- ✅ Pure Python CPIO implementation
- ✅ Cross-platform Makefile abstraction
- ✅ Automated dependency installation

## Quick Start

### Prerequisites
1. Git Bash (MINGW64)
2. Chocolatey package manager
3. Docker Desktop

### Installation (PowerShell as Administrator)

```powershell
# Visual Studio Build Tools (MSVC linker)
choco install -y visualstudio2022buildtools --package-parameters "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive"

# Build tools
choco install -y make qemu docker-desktop rustup.install

# After installation, restart terminal, then:
rustup install nightly
rustup default nightly
rustup component add rust-src --toolchain nightly
cargo install rustfilt
```

### Build and Run

```bash
git clone https://github.com/nramos0/kevlar
cd kevlar
make build
make run
```

## Technical Challenges Solved

### 1. MSVC Linker (link.exe)
Rust build scripts require Visual Studio Build Tools on Windows.

### 2. Cross-Platform Commands
Replaced Unix commands (cp, mkdir -p) with Python equivalents for portability.

### 3. LLVM Tool Discovery
Dynamically locate LLVM tools in Rust sysroot with .exe extensions.

### 4. Path Normalization
Windows backslashes converted to forward slashes for compatibility.

### 5. Docker File Locking
Fixed NamedTemporaryFile locking issues with mkstemp approach.

### 6. Symlink Handling
Used lstat() instead of stat() to avoid resolving Unix symlinks on Windows.

### 7. CPIO Format
Implemented pure Python CPIO newc writer (~60 lines).

### 8. Line Endings
Enforced LF via .gitattributes for consistent shell script execution.

## Build Performance

- Initial build: ~2-3 minutes
- Incremental: ~5-10 seconds
- Kernel size: ~11 MB (stripped)

## Modified Files

- Makefile - Platform detection and tool abstraction
- tools/docker2initramfs.py - Pure Python implementation
- testing/Dockerfile - Updated to latest versions
- pyproject.toml - Removed build-system config
- .gitattributes - Line ending enforcement

## License Note

Uses Linux UAPI headers (GPL + Linux-syscall-note exception), which permits use in any program.

## Conclusion

Kevlar now builds natively on Windows, macOS, and Linux with identical developer experience.

---

**Date**: March 9, 2026  
**Tested**: Windows 11 Pro, Git Bash, Rust nightly-2025-12-06
