# M10 Phase 4 + 4.5: Userspace Networking and ext4

Two phases in one session: wiring userspace tools to our existing smoltcp
network stack, and extending the ext2 driver to handle ext4 images.

## Phase 4: Userspace Networking

The kernel already had a fully functional TCP/UDP/DHCP stack (smoltcp +
virtio-net), but userspace couldn't see it. `ifconfig` failed, DNS didn't
resolve, `wget` couldn't connect. The problem wasn't the network stack — it
was the missing glue between userspace tools and kernel state.

### Network interface ioctls

BusyBox `ifconfig` doesn't use netlink or `/proc/net/` — it opens a socket
and fires ioctl commands. A new `net_ioctl.rs` handles the full set:

```rust
if (cmd & 0xFF00) == 0x8900 {
    return self.sys_net_ioctl(cmd, arg);
}
```

This intercepts the 0x89xx ioctl range before it reaches `FileLike::ioctl()`.
The handler reads `ifr_name` from the `struct ifreq` (16 bytes), validates
"eth0" or "lo", and dispatches:

| ioctl | What we return |
|-------|---------------|
| SIOCGIFFLAGS | IFF_UP\|IFF_RUNNING\|IFF_BROADCAST (eth0) or IFF_LOOPBACK (lo) |
| SIOCGIFADDR | IP from `INTERFACE.lock().ip_addrs()` as sockaddr_in |
| SIOCGIFNETMASK | Derived from CIDR prefix length |
| SIOCGIFHWADDR | MAC from virtio-net driver |
| SIOCGIFCONF | List of both interfaces (for `ifconfig -a`) |
| SIOCSIF* | Accept silently — kernel manages state |

The IP address and netmask come directly from smoltcp's `Interface`, which
is already configured via boot params or DHCP. No new state needed.

### AF_NETLINK and AF_PACKET

Some tools try netlink first, then fall back to ioctls. Returning
EAFNOSUPPORT (a new errno, value 97) from `socket(AF_NETLINK, ...)` triggers
this fallback cleanly:

```rust
(AF_NETLINK, _, _) | (AF_PACKET, _, _) => {
    Err(Errno::EAFNOSUPPORT.into())
}
```

### /proc/net/ stubs

`/proc/net/dev` returns a two-header-line + eth0/lo table with zero counters.
`/proc/net/if_inet6` is empty (no IPv6). Tools like `ifconfig` and `ip` check
these to discover interfaces.

### OpenRC networking

With ioctls working, OpenRC's networking service can run. Config files:

```
# /etc/network/interfaces
auto eth0
iface eth0 inet static
    address 10.0.2.15
    netmask 255.255.255.0
    gateway 10.0.2.2
```

```
# /etc/resolv.conf
nameserver 10.0.2.3
```

Boot output now shows `* Starting networking ... [ ok ]`.

## Phase 4.5: ext4 Read-Only Support

The ext2 driver was 667 lines handling superblock, block groups, inode tables,
direct/indirect block pointers, directories, and symlinks. ext4 extends this
format with three key features we need to handle for read-only mounting.

### Feature flags

ext4 puts three bitmasks in the superblock: compatible, incompatible, and
read-only compatible features. The critical rule: if the `feature_incompat`
field has bits we don't understand, we **must not mount**. This prevents
silently misinterpreting on-disk structures.

```rust
const INCOMPAT_SUPPORTED: u32 = INCOMPAT_FILETYPE
    | INCOMPAT_RECOVER | INCOMPAT_JOURNAL_DEV
    | INCOMPAT_EXTENTS | INCOMPAT_64BIT
    | INCOMPAT_FLEX_BG | INCOMPAT_MMP
    | INCOMPAT_LARGEDIR | INCOMPAT_CSUM_SEED;

if sb.feature_incompat & !INCOMPAT_SUPPORTED != 0 {
    return None;  // refuse to mount
}
```

For read-only, we can ignore compatible and read-only-compatible features
entirely. The journal (COMPAT_HAS_JOURNAL) is just another inode we skip.
Checksums (RO_COMPAT_METADATA_CSUM) don't affect data reads. HTree
directory indexing stores a hash tree *alongside* the standard linear
directory entries, so our existing linear scan still works.

### Extent trees

This is the core new data structure. ext2 uses 15 block pointers per inode
(12 direct + 3 indirect). ext4 replaces this with an extent tree stored in
the same 60-byte `i_block` area.

Each node has a 12-byte header followed by 12-byte entries:

```
ExtentHeader (12B): magic=0xF30A, entries, max, depth
```

At depth 0 (leaf), entries are `Extent` structs mapping contiguous ranges:

```
Extent (12B): logical_block, len, start_hi:start_lo
```

A single extent can cover thousands of contiguous blocks — much more
efficient than one-pointer-per-block. At depth > 0, entries are `ExtentIdx`
structs pointing to child blocks in a B-tree.

The resolution path:

```rust
fn resolve_extent_in_node(&self, node_data: &[u8], logical_block: u32, depth_limit: u16) -> Result<u64> {
    let header = ExtentHeader::parse(node_data);
    if header.depth == 0 {
        // Leaf: scan extents for one covering logical_block
        for i in 0..header.entries {
            let ext = Extent::parse(&node_data[12 + i * 12..]);
            if logical_block >= ext.logical_block
               && logical_block < ext.logical_block + ext.block_count() {
                return Ok(ext.physical_start() + offset_within);
            }
        }
        Ok(0)  // sparse hole
    } else {
        // Internal: find child, recurse
        // ...
    }
}
```

The dispatch in `read_file_data` checks inode flags:

```rust
let block_num = if inode.uses_extents() {
    self.resolve_extent(inode, block_index)?
} else {
    self.resolve_block_ptr(inode, block_index, ptrs_per_block)? as u64
};
```

### 64-bit group descriptors

When `INCOMPAT_64BIT` is set, group descriptors grow from 32 to 64 bytes,
and the `inode_table` field becomes 48-bit (low 32 at offset 8, high 16 at
offset 40). The superblock's `desc_size` field (offset 254) gives the exact
stride.

### What didn't change

The directory entry format is identical between ext2 and ext4. Symlink
storage is the same (inline for <= 60 bytes, block-based otherwise — though
ext4 symlinks with the extents flag need block-based reads even when small).
The mount syscall now accepts "ext2", "ext3", and "ext4" — all routed to
the same code path.

Total ext2 crate delta: +150 lines (667 -> ~810). Still `#![forbid(unsafe_code)]`.

## Files changed

| File | Change |
|------|--------|
| `kernel/syscalls/net_ioctl.rs` | New: network interface ioctls |
| `kernel/syscalls/ioctl.rs` | Intercept 0x89xx range + FIONBIO |
| `kernel/syscalls/socket.rs` | AF_NETLINK/AF_PACKET stubs |
| `kernel/fs/procfs/mod.rs` | /proc/net/ directory |
| `kernel/fs/procfs/system.rs` | ProcNetDevFile |
| `services/kevlar_ext2/src/lib.rs` | ext4 extents, feature flags, 64-bit |
| `kernel/syscalls/mount.rs` | Accept "ext3"/"ext4" |
| `kernel/syscalls/statfs.rs` | Accept "ext3"/"ext4" |
| `libs/kevlar_vfs/src/result.rs` | EAFNOSUPPORT errno |
| `libs/kevlar_vfs/src/socket_types.rs` | AF_NETLINK, AF_PACKET |
| `testing/Dockerfile` | ext4 disk image, resolv.conf, network config |
