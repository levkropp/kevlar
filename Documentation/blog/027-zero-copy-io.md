# M5 Phase 3: Zero-Copy I/O

sendfile, splice, tee, and copy_file_range are the Linux syscalls that
move data between file descriptors without copying through userspace.
Web servers use sendfile to push static files into sockets, and cp/rsync
use copy_file_range for efficient file-to-file transfers.

## Implementation

All four syscalls follow the same pattern: a kernel-side bounce buffer
(`[u8; 4096]`) shuttles data between two file descriptors in a loop.
Despite the name "zero-copy I/O," there's no actual zero-copy happening
here — that would require scatter-gather DMA or page remapping. The
real benefit is avoiding the userspace roundtrip: one syscall instead of
read() + write() pairs.

### sendfile(2)

Transfers data from an input file descriptor to an output fd. Supports
an optional offset pointer — if provided, reads from that offset without
changing the file position (useful for serving the same file to multiple
clients concurrently).

### splice(2)

Like sendfile but for pipes: transfers data between a pipe and a file
descriptor. Both input and output support optional offset pointers. The
inner loop handles short writes correctly — if the output fd accepts
fewer bytes than read, the loop continues from where it left off.

### copy_file_range(2)

File-to-file transfer. Both input and output are regular files, both
support offset pointers, and both file positions are updated correctly
(either written back to the pointer or advanced on the OpenedFile).

### tee(2)

Duplicates pipe contents without consuming them. This requires
non-consuming reads from a pipe, which we don't support yet. Returns
EINVAL — programs that use tee() are rare enough that this is fine for
now.

## Offset Handling

The trickiest part is getting offset semantics right. Each syscall has
up to two offset pointers. For each:

1. If the pointer is non-null, read the offset from userspace
2. Use it as the read/write position
3. After the transfer, write the updated offset back to userspace
4. If the pointer is null, use (and update) the file's current position

This matches Linux's behavior exactly and is critical for programs that
use offset-based I/O for concurrent access to the same file.

## What's Next

Phase 4 fills the /proc and /sys gaps that real-world programs expect.
