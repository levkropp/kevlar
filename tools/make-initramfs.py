#!/usr/bin/env python3
"""
Create a minimal CPIO initramfs for Linux with the benchmark binary.
"""

import gzip
import io
import os
import struct
import sys
from pathlib import Path


def create_cpio_entry(name, content, mode, is_dir=False):
    """Create a CPIO newc entry."""
    if is_dir:
        mode = 0o040755
        content = b""

    name_bytes = name.encode('ascii')
    content_bytes = content if isinstance(content, bytes) else content.encode('ascii')

    # CPIO newc header
    header = b"070701"  # magic
    header += f"{1:08x}".encode('ascii')  # ino
    header += f"{mode:08x}".encode('ascii')  # mode
    header += f"{0:08x}".encode('ascii')  # uid
    header += f"{0:08x}".encode('ascii')  # gid
    header += f"{2 if is_dir else 1:08x}".encode('ascii')  # nlink
    header += f"{0:08x}".encode('ascii')  # mtime
    header += f"{len(content_bytes):08x}".encode('ascii')  # filesize
    header += f"{0:08x}".encode('ascii')  # devmajor
    header += f"{0:08x}".encode('ascii')  # devminor
    header += f"{0:08x}".encode('ascii')  # rdevmajor
    header += f"{0:08x}".encode('ascii')  # rdevminor
    header += f"{len(name_bytes) + 1:08x}".encode('ascii')  # namesize
    header += f"{0:08x}".encode('ascii')  # check

    result = header + name_bytes + b'\x00'

    # Align to 4 bytes
    while len(result) % 4 != 0:
        result += b'\x00'

    result += content_bytes

    # Align to 4 bytes
    while len(result) % 4 != 0:
        result += b'\x00'

    return result


def main():
    if len(sys.argv) < 3:
        print(f"Usage: {sys.argv[0]} <benchmark-binary> <output.cpio.gz>")
        return 1

    bench_binary = Path(sys.argv[1])
    output_file = Path(sys.argv[2])

    if not bench_binary.exists():
        print(f"Error: {bench_binary} not found")
        return 1

    print(f"Creating initramfs from {bench_binary}...")

    # Read benchmark binary
    with open(bench_binary, 'rb') as f:
        bench_content = f.read()

    # Build CPIO archive
    cpio_data = io.BytesIO()

    # Add root directory
    cpio_data.write(create_cpio_entry(".", b"", 0o040755, is_dir=True))

    # Add benchmark binary as /init (runs automatically)
    cpio_data.write(create_cpio_entry("init", bench_content, 0o100755))

    # Add proc and sys directories
    cpio_data.write(create_cpio_entry("proc", b"", 0o040755, is_dir=True))
    cpio_data.write(create_cpio_entry("sys", b"", 0o040755, is_dir=True))

    # Add trailer
    cpio_data.write(create_cpio_entry("TRAILER!!!", b"", 0o100644))

    # Compress with gzip
    with gzip.open(output_file, 'wb', compresslevel=9) as f:
        f.write(cpio_data.getvalue())

    print(f"Created {output_file} ({len(cpio_data.getvalue())} bytes uncompressed)")
    print(f"  Compressed: {output_file.stat().st_size} bytes")

    return 0


if __name__ == "__main__":
    sys.exit(main())
