#!/usr/bin/env python3
import argparse
import os
import shlex
import struct
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path


def create_cpio_archive(root_dir, output_file):
    """Create a cpio archive in newc format using pure Python."""
    root_dir = Path(root_dir)

    def write_cpio_header(f, name, mode, uid, gid, nlink, mtime, filesize, dev_major, dev_minor, rdev_major, rdev_minor, ino):
        """Write a CPIO newc format header."""
        # CPIO newc format uses 32-bit fields - mask all values to fit
        namesize = len(name) + 1  # +1 for null terminator
        header = f"070701{ino & 0xFFFFFFFF:08x}{mode & 0xFFFFFFFF:08x}{uid & 0xFFFFFFFF:08x}{gid & 0xFFFFFFFF:08x}{nlink & 0xFFFFFFFF:08x}{mtime & 0xFFFFFFFF:08x}{filesize & 0xFFFFFFFF:08x}{dev_major & 0xFFFFFFFF:08x}{dev_minor & 0xFFFFFFFF:08x}{rdev_major & 0xFFFFFFFF:08x}{rdev_minor & 0xFFFFFFFF:08x}{namesize & 0xFFFFFFFF:08x}00000000"
        f.write(header.encode('ascii'))
        f.write(name.encode('ascii'))
        f.write(b'\x00')
        # Align to 4-byte boundary
        padding = (4 - ((len(header) + namesize) % 4)) % 4
        f.write(b'\x00' * padding)

    def collect_all_entries(root):
        """Recursively collect all files, directories, and symlinks.
        On Windows, Path.rglob() doesn't include symlinks, so we walk manually."""
        entries = []
        for item in root.iterdir():
            entries.append(item)
            # Recurse into directories (but not symlinks to avoid loops)
            if item.is_dir() and not item.is_symlink():
                entries.extend(collect_all_entries(item))
        return entries

    with open(output_file, 'wb') as f:
        # Collect all files and directories (including symlinks on Windows)
        entries = sorted(collect_all_entries(root_dir), key=lambda p: str(p))
        print(f"Collected {len(entries)} entries to write to CPIO", file=sys.stderr)

        ino = 1
        entries_written = 0
        for entry in entries:
            try:
                # Use Unix-style forward slashes for CPIO paths (even on Windows)
                rel_path = str(entry.relative_to(root_dir)).replace('\\', '/')
                # Use lstat to not follow symlinks (important on Windows where Unix paths can't be resolved)
                stat = entry.lstat()

                mode = stat.st_mode
                uid = stat.st_uid if hasattr(stat, 'st_uid') else 0
                gid = stat.st_gid if hasattr(stat, 'st_gid') else 0
                nlink = stat.st_nlink if hasattr(stat, 'st_nlink') else 1
                mtime = int(stat.st_mtime)
            except Exception as e:
                print(f"ERROR processing entry {entry}: {e}", file=sys.stderr)
                continue

            try:
                if entry.is_file() and not entry.is_symlink():
                    filesize = stat.st_size
                    write_cpio_header(f, rel_path, mode, uid, gid, nlink, mtime, filesize, 0, 0, 0, 0, ino)

                    # Write file contents
                    with open(entry, 'rb') as src:
                        f.write(src.read())

                    # Align to 4-byte boundary
                    padding = (4 - (filesize % 4)) % 4
                    f.write(b'\x00' * padding)
                elif entry.is_symlink():
                    target = os.readlink(entry)
                    filesize = len(target)
                    write_cpio_header(f, rel_path, mode, uid, gid, nlink, mtime, filesize, 0, 0, 0, 0, ino)
                    f.write(target.encode('utf-8'))
                    # Align to 4-byte boundary
                    padding = (4 - (filesize % 4)) % 4
                    f.write(b'\x00' * padding)
                else:  # directory or other
                    write_cpio_header(f, rel_path, mode, uid, gid, nlink, mtime, 0, 0, 0, 0, 0, ino)

                entries_written += 1
                ino += 1
            except Exception as e:
                print(f"ERROR writing entry {rel_path}: {e}", file=sys.stderr)
                import traceback
                traceback.print_exc(file=sys.stderr)

        print(f"Successfully wrote {entries_written} entries to CPIO", file=sys.stderr)

        # Write trailer
        write_cpio_header(f, "TRAILER!!!", 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0)


def main():
    parser = argparse.ArgumentParser(
        description="Converts a Docker image into cpio (initramfs format).")
    parser.add_argument("outfile", help="The output path.")
    parser.add_argument("image",
                        help="The docker image name (e.g. python:slim).")
    args = parser.parse_args()

    # Use DOCKER environment variable if set (for Windows path with spaces)
    docker_env = os.environ.get('DOCKER', 'docker')
    # If it's a quoted path, strip quotes
    if docker_env.startswith('"') and docker_env.endswith('"'):
        docker_cmd = docker_env.strip('"')
    else:
        docker_cmd = docker_env

    # Convert Git Bash path (/c/...) to Windows path (C:/...)
    if docker_cmd.startswith('/') and len(docker_cmd) > 2 and docker_cmd[2] == '/':
        docker_cmd = docker_cmd[1].upper() + ':' + docker_cmd[2:]

    # On Windows, if docker isn't found, try standard location
    if sys.platform == 'win32' and docker_cmd == 'docker':
        standard_path = 'C:/Program Files/Docker/Docker/resources/bin/docker.exe'
        if os.path.exists(standard_path):
            docker_cmd = standard_path

    container_id = f"docker-initramfs-tmp"
    try:
        subprocess.run([docker_cmd, "rm", container_id],
                       stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL,
                       check=False)
        subprocess.run(
            [docker_cmd, "create", "--name", container_id, "-t", args.image],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            check=True)
        # Create temp file and close it immediately so docker can write to it
        temp_fd, temp_file_name = tempfile.mkstemp(suffix=".tar")
        os.close(temp_fd)  # Close the file descriptor so docker can write to it
        temp_file_path = Path(temp_file_name)
        try:
            subprocess.run(
                [docker_cmd, "export", f"--output={temp_file_path}", container_id],
                stderr=subprocess.STDOUT,
                check=True)
            with tempfile.TemporaryDirectory() as temp_dir:
                temp_dir = Path(temp_dir)
                print(f"Extracting to: {temp_dir}", file=sys.stderr)
                # Extract using Python's tarfile for cross-platform compatibility
                with tarfile.open(temp_file_path, 'r') as tar:
                    # Use fully_trusted filter since this is our own Docker image
                    # The 'data' filter would reject absolute symlinks like bin/arch -> /bin/busybox
                    try:
                        tar.extractall(path=temp_dir, filter='fully_trusted')
                    except TypeError:
                        # Older Python versions don't support filter parameter
                        tar.extractall(path=temp_dir)
                print("Extraction complete, creating resolv.conf", file=sys.stderr)

                # XXX: This is a hack to get around the fact that the Docker overrides
                #      the /etc/resolv.conf file.
                (temp_dir / "etc" /
                 "resolv.conf").write_text("nameserver 1.1.1.1")

                print("Creating CPIO archive", file=sys.stderr)
                # Create cpio archive (newc format)
                create_cpio_archive(temp_dir, args.outfile)
                print("CPIO archive created successfully", file=sys.stderr)
        finally:
            # Clean up temp file
            if temp_file_path.exists():
                temp_file_path.unlink()
    except subprocess.CalledProcessError as e:
        error_msg = ""
        if e.stdout:
            error_msg = e.stdout.decode('utf-8', 'backslashreplace')
        elif e.stderr:
            error_msg = e.stderr.decode('utf-8', 'backslashreplace')
        sys.exit(
            f"{error_msg}\n\nError: failed to export {args.image}"
        )
    finally:
        subprocess.run([docker_cmd, "rm", container_id],
                       stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL,
                       check=False)


if __name__ == "__main__":
    main()
