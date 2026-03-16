# M10: Boot Polish — Terminal Corruption, Login Prompt, and faccessat2

After implementing Phases 4–5 (networking, ext4, sysfs), the boot
sequence worked but the login prompt was invisible in real terminals.
Three separate bugs conspired to hide it.

## Bug 1: Auto-wrap disabled by SeaBIOS

SeaBIOS sends `ESC[?7l` (disable auto-wrap) during its initialization.
This VT100 escape sequence tells the terminal not to wrap long lines —
text past column 80 just overwrites the last character on the line.

The kernel never re-enabled wrapping. During OpenRC boot, the dynamic
linker logged 16 messages at 137 characters each. With wrapping
disabled, these lines overflowed silently, but the `\n` at the end
still advanced the cursor one row. Real terminals (Konsole, xterm)
lost track of which row the cursor was on, and the login prompt
rendered off-screen or in the wrong position.

The Python `pyte` terminal emulator didn't reproduce this because it
handles no-wrap mode slightly differently than Konsole/xterm.

**Fix:** One line in `kernel/main.rs` at early boot:

```rust
kevlar_platform::print!("\x1b[?7h");
```

## Bug 2: run-qemu.py line-buffered stdout

The `--save-dump` flag in `run-qemu.py` intercepts QEMU's stdout to
detect crash dumps. It used Python's `for line in p.stdout:` iterator,
which buffers by newline. BusyBox getty's login prompt (`kevlar login: `)
ends with a space, not a newline — it's waiting for the user to type
their username. Python's line iterator never flushed it, so the prompt
sat in a buffer forever.

**Fix:** Replaced line iteration with unbuffered `read1()`:

```python
while True:
    chunk = p.stdout.read1(4096)
    if not chunk:
        break
    sys.stdout.buffer.write(chunk)
    sys.stdout.buffer.flush()
```

## Bug 3: NUL bytes in serial output

Mysterious `\x0f\x00\x00\x00` byte sequences appeared in the serial
output between kernel log messages. The `\x0f` byte (SI — Shift In) is
a VT100 control character that switches the terminal to the G0 alternate
character set, making subsequent text render as line-drawing characters
or invisible glyphs. The three NUL bytes further confused terminal state.

These bytes weren't from any `write()` syscall (we verified by adding
kernel-side detection) and weren't from the logger. Their origin remains
unclear — possibly a race in concurrent serial port access or
uninitialized buffer contents.

**Fix:** Filter NUL and SI/SO bytes in the serial driver:

```rust
pub fn print_char(&self, ch: u8) {
    if ch == 0 || ch == 0x0e || ch == 0x0f {
        return;
    }
    // ...
}
```

## Other fixes in this session

**Default hostname:** The UTS namespace initialized with an empty hostname.
Getty used `?` as fallback, making the prompt `? login:` which was easy
to miss. Now defaults to `"kevlar"`.

**Dynamic link noise:** The `warn!("dynamic link: ...")` message fired for
every dynamically-linked program (16 times during OpenRC boot, each 137
chars). Changed to `trace!()` — invisible in normal builds, available
with debug log filter.

**Terminal type:** Changed getty from `vt100` to `linux` in inittab.

**`faccessat2` (syscall 439):** Bash uses this newer variant of `faccessat`.
Was printing "unimplemented system call" on every command. Wired to the
existing `sys_access()` handler.

**`make run` default:** Now boots OpenRC with KVM (was bare `/bin/sh`).
Old behavior available as `make run-sh`.

## Debugging approach

Built an automated boot test harness (`tools/test-boot.sh`) that:
1. Patches the ELF for QEMU multiboot loading
2. Boots with `-serial file:` (no interactive terminal needed)
3. Greps serial output for `login:`
4. Reports PASS/FAIL

Also built a PTY-based test (`tools/test-boot-interactive.py`) that
spawns QEMU with a real PTY and feeds output through `pyte` (Python
VT100 emulator) to see exactly what a terminal would render.

The final confirmation: launched xterm programmatically via `xdotool`,
captured a screenshot with ImageMagick `import`, and verified the
login prompt was visible.

## Files changed

| File | Change |
|------|--------|
| `kernel/main.rs` | `ESC[?7h` at boot + sysfs::populate() |
| `kernel/process/process.rs` | dynamic link log: warn→trace, cmdline in crash msg |
| `kernel/namespace/uts.rs` | Default hostname "kevlar" |
| `platform/x64/serial.rs` | Filter NUL/SI/SO bytes |
| `tools/run-qemu.py` | Unbuffered stdout in --save-dump, --batch flag |
| `testing/etc/inittab` | vt100→linux terminal type |
| `kernel/syscalls/mod.rs` | faccessat2 (439) wired to sys_access |
| `Makefile` | make run = OpenRC+KVM, make run-sh = bare shell |
| `tools/test-boot.sh` | Automated boot test harness |
| `tools/docker-progress.py` | Docker build progress filter |
