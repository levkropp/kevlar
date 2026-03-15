# M10 Phase 4: Writable ext4

**Goal:** Read-write ext4 filesystem on virtio-blk, suitable as root or
data partition.

## Why ext4

ext4 is the default filesystem for every major distro. Without ext4 write
support, the system can't persist configuration, install packages to disk,
or maintain logs. This is the single biggest gap between "kernel demo" and
"real OS."

## Scope

### ext4 read-write driver

Our ext2 driver (`services/kevlar_ext2/`) handles:
- Superblock, block groups, inode table
- Direct + indirect block pointers
- Directory listing, symlinks, regular files
- Read-only operations

ext4 adds:
- **Extents** (replaces indirect blocks for large files) — more efficient
  but different on-disk format. Can fall back to ext2-compatible mode
  for small files.
- **Journal** (jbd2) — crash consistency. For initial support, ignore the
  journal (mount with `-o norecovery`). Implement journal replay later.
- **Write support** — allocate blocks, update inodes, write data pages,
  update directory entries, create/delete files.
- **fsync** — flush dirty pages and metadata to disk.

### Minimal write operations needed

1. `write()` to existing file — allocate blocks, write data, update inode size
2. `create()` — allocate inode, add directory entry
3. `unlink()` — remove directory entry, free inode + blocks
4. `mkdir()` / `rmdir()` — directory create/remove
5. `rename()` — move directory entry
6. `truncate()` — shrink/grow file
7. `chmod()` / `chown()` — update inode metadata

### Partition table

Real disks have GPT or MBR partition tables. For QEMU, we can:
- Use a raw disk image (no partition table) — simplest
- Parse MBR partition table — 512 bytes at sector 0
- Parse GPT — more complex but standard

Start with raw images, add MBR later.

## Approach

1. Start with ext2 write support (no journal, no extents)
2. Add extent read support (ext4 backward-compat)
3. Add journaling (crash recovery)
4. Add extent write support

This gets us a working writable filesystem quickly, then hardens it.

## Verification

```
# Create ext4 disk image, mount read-write, create files
mkfs.ext4 /tmp/test.img
# Boot with disk attached
qemu ... -drive file=/tmp/test.img,if=virtio,format=raw
# In guest:
mount /dev/vda /mnt
echo "hello" > /mnt/test.txt
cat /mnt/test.txt  # should print "hello"
sync
umount /mnt
# Reboot, remount — file should persist
```
