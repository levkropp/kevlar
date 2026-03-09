# The 8-Byte Copy That Should Have Been 4

BusyBox ash boots, runs commands, seems fine. Then bash crashes with a
stack canary corruption. GDB shows `rep movsb` in the usercopy trailing-bytes
path wrote 8 bytes when we asked for 4. The Rust code is correct. The
compiler generates correct code. Something else changes `rdx` before it
reaches the assembly. Except it doesn't — the bug is in the assembly itself.

## The symptom

A `write::<c_int>` (4 bytes) to userspace overwrites the stack canary at
`fsbase+0x28`. The watchpoint shows the copy wrote to an 8-byte range
starting exactly 4 bytes before the canary. Kernel pointer bytes leaked
into the canary location.

## The root cause

The x86_64 usercopy assembly had two copy paths:

```asm
copy_to_user:
    cmp rdx, 8
    jb .Lbyte_copy       // < 8 bytes: simple path

    // ... alignment + bulk qword copy ...
usercopy1:
    rep movsb            // leading bytes
.Laligned:
    rep movsq            // bulk qwords
    rep movsb            // trailing bytes
    ret

.Lbyte_copy:
    mov rcx, rdx
    jmp usercopy1        // BUG: falls through to .Laligned!
```

`.Lbyte_copy` jumped to `usercopy1` (`rep movsb`) for the simple copy.
But `usercopy1` has no `ret` — it falls through to `.Laligned`, which
executes the qword bulk copy AND trailing bytes copy *again*. For a
4-byte copy: 4 bytes from byte_copy + 0 qwords + 4 trailing = **8 bytes
total**. Every copy under 8 bytes was doubled.

The fix: `.Lbyte_copy` gets its own `rep movsb; ret` with a new
`usercopy1d` label. No fall-through.

## Why existing tooling couldn't catch it

Our Rust-level instrumentation logged `buf.len()` which correctly showed 4.
The canary check caught the corruption post-syscall but couldn't identify
which copy caused it — there are dozens of `write::<T>` calls per syscall.
We needed to see what the CPU actually executed, not what Rust thought it
passed.

## The debug tooling we built

### Assembly-level trace ring buffer

A 32-entry ring buffer written by the `copy_to_user` assembly probe at
function entry, before any computation:

```asm
.Ltrace_entry:
    push rax
    push rcx
    push r8
    push rdx
    lea r8, [rip + ucopy_trace_buf]
    // ... compute slot ...
    mov [r8 + 0],  rdi       // dst
    mov [r8 + 8],  rsi       // src
    mov [r8 + 16], rdx       // len — the actual value
    mov rcx, [rsp + 32]
    mov [r8 + 24], rcx       // return address
```

This captures the **actual CPU register values** — not what Rust thinks it
passed. After a canary corruption, the ring buffer dump shows every recent
copy with its real length and return address.

Fast path when disabled: a single `cmp qword ptr [rax], 0` + not-taken
`jne`. Essentially zero overhead.

### Structured JSONL event system

15 event types emitted as `DBG {"type":"...","pid":...}` lines to serial
output. Categories enabled independently via `debug=syscall,signal,fault,
canary,usercopy`:

- **SyscallEntry/Exit** — strace-like with args, return values, errno names
- **CanaryCheck** — pre/post syscall canary comparison
- **PageFault** — with VMA context, resolution status
- **UsercopyFault** — which assembly phase (leading/bulk/trailing/small)
- **UsercopyTraceDump** — the ring buffer contents, auto-emitted on corruption
- **Signal/ProcessFork/ProcessExec/ProcessExit** — lifecycle events
- **Panic** — with structured backtrace (stack-allocated, panic-safe)

### Usercopy context tags

Every `write::<T>` to userspace is wrapped with a context tag:

```rust
debug::usercopy::set_context("ioctl:TCGETS");
let r = arg.write::<Termios>(&termios);
debug::usercopy::clear_context();
r?;
```

When a fault or corruption occurs, the tag identifies the kernel operation.
Instrumented: all TTY/PTY ioctls, uname, getcwd, getdents64, wait4,
select, rt_sigaction, signal stack setup.

### MCP debug server

21 tools exposed via the Model Context Protocol for LLM-driven debugging:

- `debug_summary` — aggregate session stats
- `get_usercopy_trace_dumps` — the assembly ring buffer dumps
- `get_canary_corruptions` — all detected stack corruptions
- `get_syscall_trace` — strace-like filtered trace
- `resolve_address` — offline symbol resolution

### Crash analyzer

Offline CLI tool for crash dumps and serial logs. Detects patterns
(canary corruption, usercopy faults, null derefs, missing syscalls)
and outputs structured JSON for LLM consumption.

## Results

With the usercopy fix, BusyBox ash boots cleanly. Bash runs inside ash
with only a minor warning. `ls -l` works. Zero canary corruptions in a
40-second boot with `debug=canary,fault` enabled.

The debug tooling that was built to find this bug is now permanent
infrastructure — it'll catch the next register-level bug automatically.
