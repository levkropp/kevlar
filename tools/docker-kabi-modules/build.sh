#!/bin/bash
# Inside the container: fetch Ubuntu's linux-7.0.0-14 source, configure to
# build ext4 / jbd2 / mbcache as modules (default is built-in), compile only
# those three, drop them in /output.
set -euo pipefail

KERNEL_PKG_VERSION="7.0.0-14.14"
KERNEL_VERSION="7.0.0-14"

cd /build

echo "==> apt-get source linux=${KERNEL_PKG_VERSION}"
apt-get update >/dev/null
apt-get source linux="${KERNEL_PKG_VERSION}"

# The unpacked source dir name follows the linux_<upstream>.orig.tar.* pattern.
SRC_DIR="$(ls -d linux-7.0.0/ 2>/dev/null | head -1)"
if [ -z "${SRC_DIR}" ]; then
    SRC_DIR="$(ls -d linux-*/ | head -1)"
fi
echo "==> source dir: ${SRC_DIR}"
cd "${SRC_DIR}"

# Use Ubuntu's stock config for this kernel so the binary ABI matches the
# `7.0.0-14-generic` modules we already extracted (erofs.ko et al).
echo "==> seeding .config from debian.master/config"
ARCH=arm64
KCONFIG_SRC=""
for candidate in \
    debian.master/config/arm64/config.common.arm64 \
    debian/config/arm64/config.common.arm64 \
    debian.master/config/config.common.ubuntu \
    debian/config/config.common.ubuntu; do
    if [ -f "${candidate}" ]; then KCONFIG_SRC="${candidate}"; break; fi
done

if [ -n "${KCONFIG_SRC}" ]; then
    cp "${KCONFIG_SRC}" .config
    # Append arch-specific bits if present.
    for extra in \
        debian.master/config/arm64/config.flavour.generic \
        debian/config/arm64/config.flavour.generic; do
        [ -f "${extra}" ] && cat "${extra}" >> .config
    done
else
    echo "==> no debian config found; falling back to defconfig"
    make ARCH=arm64 defconfig
fi

# Force ext4 / jbd2 / mbcache to module form.  Ubuntu's stock config has these
# as =y; flipping to =m gives us loadable .ko files we can dispatch through
# the kABI loader.
echo "==> setting CONFIG_EXT4_FS / CONFIG_JBD2 / CONFIG_FS_MBCACHE = m"
scripts/config --file .config --module CONFIG_EXT4_FS
scripts/config --file .config --module CONFIG_JBD2
scripts/config --file .config --module CONFIG_FS_MBCACHE
# Disable debug-info — saves a lot of disk + RAM and the kABI side doesn't
# care.
scripts/config --file .config --disable CONFIG_DEBUG_INFO
scripts/config --file .config --disable CONFIG_DEBUG_INFO_BTF
# Disable module signing so the resulting .ko has no embedded signature.
scripts/config --file .config --disable CONFIG_MODULE_SIG_ALL
scripts/config --file .config --disable CONFIG_MODULE_SIG
scripts/config --file .config --disable CONFIG_SYSTEM_TRUSTED_KEYS
scripts/config --file .config --disable CONFIG_SYSTEM_REVOCATION_KEYS

echo "==> make ARCH=arm64 olddefconfig"
make ARCH=arm64 olddefconfig

echo "==> make ARCH=arm64 modules_prepare (-j$(nproc))"
make ARCH=arm64 -j"$(nproc)" modules_prepare

# Phase 12 v7 (kept commented): instrument ext4_fill_super with pr_err
# at every `goto failed_mount`.  When enabled, the instrumented .ko
# changes code layout enough to surface a different fault before
# the instrumentation lands; revisit if we need definitive identification
# of the failing branch.
# python3 /build/instrument-ext4.py fs/ext4/super.c

# Skip the full vmlinux build — modpost would otherwise fail with
# "vmlinux.o is missing".  Our kABI loader does its own symbol
# resolution at load time (same as erofs.ko), so unresolved symbols
# in the .ko don't matter here.
export KBUILD_MODPOST_WARN=1

# Build the three modules we need.  Each lives at a known path in the source
# tree.  `make M=<dir> modules` builds only that subtree's modules.
echo "==> building fs/mbcache.ko"
make ARCH=arm64 -j"$(nproc)" fs/mbcache.ko

echo "==> building fs/jbd2/jbd2.ko"
make ARCH=arm64 -j"$(nproc)" M=fs/jbd2 modules

echo "==> building fs/ext4/ext4.ko"
make ARCH=arm64 -j"$(nproc)" M=fs/ext4 modules

mkdir -p /output
cp -v fs/mbcache.ko fs/jbd2/jbd2.ko fs/ext4/ext4.ko /output/

# Strip debug info — kABI loader doesn't use DWARF and the unstripped
# files are 5–6× larger.
echo "==> stripping debug info"
aarch64-linux-gnu-strip --strip-debug /output/*.ko \
    || strip --strip-debug /output/*.ko \
    || llvm-strip --strip-debug /output/*.ko \
    || echo "(no strip tool found — leaving debug info in place)"

ls -la /output/

echo "==> done"
