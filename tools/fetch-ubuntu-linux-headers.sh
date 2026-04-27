#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
# Populate build/linux-src/ with Ubuntu 26.04's Linux 7.0 headers.
#
# We can't run `make modules_prepare` on macOS (Apple clang chokes on
# Linux's host-tool source — scripts/mod/file2alias.c).  Instead, grab
# the prebuilt header artifacts from Ubuntu's `linux-headers-*` debs.
#
# Usage:  tools/fetch-ubuntu-linux-headers.sh
# Output: build/linux-src/ populated with Linux 7.0 source + generated
#         headers ready for K8 module builds.

set -e

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/build/linux-src"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# Mirror the Ubuntu 26.04 LTS (Resolute Raccoon) kernel.
KVER="7.0.0-14"
DEB_REV="14.14"
MIRROR="http://mirrors.kernel.org/ubuntu/pool/main/l/linux"
ALL_DEB="linux-headers-${KVER}_${KVER%-*}-${DEB_REV}_all.deb"
GENERIC_DEB="linux-headers-${KVER}-generic_${KVER%-*}-${DEB_REV}_arm64.deb"

if [ -f "$DEST/include/generated/autoconf.h" ] && \
   [ -f "$DEST/include/generated/bounds.h" ]; then
  echo "linux-src already populated at $DEST"
  exit 0
fi

echo "==> Downloading $ALL_DEB"
curl -fsSL --max-time 180 -o "$TMP/$ALL_DEB" "$MIRROR/$ALL_DEB"
echo "==> Downloading $GENERIC_DEB"
curl -fsSL --max-time 180 -o "$TMP/$GENERIC_DEB" "$MIRROR/$GENERIC_DEB"

echo "==> Extracting all-package"
mkdir -p "$TMP/all"
( cd "$TMP/all" && ar x "$TMP/$ALL_DEB" && \
  zstd -d -f data.tar.zst -o data.tar && \
  tar -xf data.tar )

echo "==> Extracting generic-package"
mkdir -p "$TMP/generic"
( cd "$TMP/generic" && ar x "$TMP/$GENERIC_DEB" && \
  zstd -d -f data.tar.zst -o data.tar && \
  tar -xf data.tar )

mkdir -p "$ROOT/build"
ALL_SRC="$TMP/all/usr/src/linux-headers-${KVER}"
GEN_SRC="$TMP/generic/usr/src/linux-headers-${KVER}-generic"

echo "==> Installing to $DEST (replacing if present)"
rm -rf "$DEST"
cp -R "$ALL_SRC" "$DEST"
# Overlay arch-specific generated artifacts from the generic deb.
# Use rsync if available for a clean overlay; otherwise fall back to
# tar pipe (cp -R has issues with macOS extended attrs on dir overwrite).
if command -v rsync >/dev/null 2>&1; then
  rsync -a "$GEN_SRC/" "$DEST/"
else
  ( cd "$GEN_SRC" && tar c . ) | ( cd "$DEST" && tar xf - )
fi

echo "==> Done"
echo "    autoconf.h: $(test -f "$DEST/include/generated/autoconf.h" && echo OK || echo MISSING)"
echo "    bounds.h:   $(test -f "$DEST/include/generated/bounds.h" && echo OK || echo MISSING)"
echo "    Makefile:   $(test -f "$DEST/Makefile" && echo OK || echo MISSING)"
echo "    arm64 asm:  $(test -d "$DEST/arch/arm64/include/asm" && echo OK || echo MISSING)"
