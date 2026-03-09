#!/bin/bash
# Verify Windows build environment for Kevlar
# Run this in Git Bash after installing dependencies

echo "=== Kevlar Windows Build Environment Verification ==="
echo

FAILED=0

check_command() {
    local cmd=$1
    local name=$2
    local version_arg=${3:---version}

    echo -n "Checking for $name... "
    if command -v "$cmd" &> /dev/null; then
        version=$($cmd $version_arg 2>&1 | head -1)
        echo "✓ found: $version"
    else
        echo "✗ NOT FOUND"
        FAILED=1
    fi
}

check_command make "GNU Make"
check_command uv "uv (Python manager)" "--version"
check_command cargo "Cargo (Rust)" "--version"
check_command rustc "rustc (Rust compiler)" "--version"
check_command rustfilt "rustfilt (symbol demangler)" "--version"
check_command qemu-system-x86_64 "QEMU x86_64" "--version"
check_command docker "Docker" "--version"

echo
echo -n "Checking for MSVC linker (link.exe)... "
if command -v link.exe &> /dev/null; then
    # Check if it's the MSVC linker, not Unix link command
    if link.exe 2>&1 | grep -q "Microsoft"; then
        echo "✓ found (Visual Studio Build Tools)"
    else
        echo "⚠ WARNING - Found 'link' command but not MSVC linker"
        echo "  Install Visual Studio Build Tools via Chocolatey"
        FAILED=1
    fi
else
    echo "✗ NOT FOUND"
    echo "  Run: choco install -y visualstudio2022buildtools --package-parameters \"--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive\""
    FAILED=1
fi

echo
echo -n "Checking Docker daemon... "
if docker info &> /dev/null; then
    echo "✓ running"
else
    echo "✗ NOT RUNNING - Start Docker Desktop"
    FAILED=1
fi

echo
echo -n "Checking Rust nightly... "
if rustc --version | grep -q nightly; then
    echo "✓ using nightly"
else
    echo "⚠ WARNING - Not using nightly toolchain"
    echo "  Run: rustup install nightly && rustup default nightly"
    FAILED=1
fi

echo
echo -n "Checking rust-src component... "
if rustup component list | grep -q "rust-src.*installed"; then
    echo "✓ installed"
else
    echo "✗ NOT INSTALLED"
    echo "  Run: rustup component add rust-src --toolchain nightly"
    FAILED=1
fi

echo
if [ $FAILED -eq 0 ]; then
    echo "=== ✓ All checks passed! ==="
    echo "You can now run: make run"
else
    echo "=== ✗ Some checks failed ==="
    echo "Please install missing dependencies and try again"
    echo "See WINDOWS-SETUP.md for installation instructions"
    exit 1
fi
