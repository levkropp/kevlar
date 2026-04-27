#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
# Populate build/linux-modules/ with Ubuntu 26.04's prebuilt
# Linux 7.0 .ko files (linux-modules-7.0.0-14-generic.deb).
#
# Used by K9+: load these binaries directly through Kevlar's K1
# loader to validate kABI compatibility with Canonical's actual
# build artifacts.
#
# Usage:  tools/fetch-ubuntu-linux-modules.sh
# Output: build/linux-modules/lib/modules/7.0.0-14-generic/...

set -e

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/build/linux-modules"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

KVER="7.0.0-14"
DEB_REV="14.14"
MIRROR="http://mirrors.kernel.org/ubuntu/pool/main/l/linux"
DEB="linux-modules-${KVER}-generic_${KVER%-*}-${DEB_REV}_arm64.deb"

if [ -d "$DEST/lib/modules/${KVER}-generic" ]; then
  echo "linux-modules already populated at $DEST"
  exit 0
fi

echo "==> Downloading $DEB (~285 MB)"
curl -fL --max-time 1800 --retry 3 --retry-delay 5 \
  -C - -o "$TMP/$DEB" "$MIRROR/$DEB"

echo "==> Extracting deb"
mkdir -p "$TMP/extract"
( cd "$TMP/extract" && ar x "$TMP/$DEB" )
# Ubuntu modules debs use plain `data.tar` (no zstd inner compression) —
# unlike the headers deb which uses data.tar.zst.
if [ -f "$TMP/extract/data.tar.zst" ]; then
  zstd -d -f "$TMP/extract/data.tar.zst" -o "$TMP/extract/data.tar"
fi
( cd "$TMP/extract" && tar -xf data.tar )

# Modern Ubuntu (24.04+) installs modules under /usr/lib/modules/.
SRC="$TMP/extract/usr/lib/modules/${KVER}-generic"
if [ ! -d "$SRC" ]; then
  SRC="$TMP/extract/lib/modules/${KVER}-generic"
fi
if [ ! -d "$SRC" ]; then
  echo "ERROR: couldn't locate modules dir in extracted deb" >&2
  exit 1
fi

mkdir -p "$ROOT/build"
rm -rf "$DEST"
mkdir -p "$DEST/lib/modules"
mv "$SRC" "$DEST/lib/modules/${KVER}-generic"

# Decompress .ko.zst — our K1 loader doesn't speak zstd; decompress
# at fetch time so the cpio just contains plain ELF .ko bytes.
echo "==> Decompressing .ko.zst (this is the bulk of the time)"
ZST_COUNT=$(find "$DEST/lib/modules/${KVER}-generic" -name '*.ko.zst' | wc -l | tr -d ' ')
find "$DEST/lib/modules/${KVER}-generic" -name '*.ko.zst' | while read f; do
  zstd -d -q -f "$f" -o "${f%.zst}"
  rm "$f"
done

KO_COUNT=$(find "$DEST/lib/modules/${KVER}-generic" -name '*.ko' 2>/dev/null | wc -l | tr -d ' ')
echo "==> Done"
echo "    decompressed $ZST_COUNT .ko.zst → $KO_COUNT .ko"
echo "    e.g. $DEST/lib/modules/${KVER}-generic/kernel/drivers/soc/fsl/qbman/bman-test.ko"
