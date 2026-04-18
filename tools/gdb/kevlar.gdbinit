# Kevlar kernel GDB init.
# Usage:
#   1. Run kernel with --gdb (run-qemu.py) — opens GDB stub on :7789, paused.
#   2. From kevlar repo root: rust-gdb -x tools/gdb/kevlar.gdbinit
# Then `c` to continue, or set additional breakpoints first.

# The kernel ELF is the unwind variant — it has full debug info.
file target/x64-unwind/debug/kevlar_kernel

# Connect to QEMU's GDB stub.
target remote :7789

set pagination off
set print pretty on
set print elements 64

# Heuristics for kernel debugging.
set disassembly-flavor intel

# Useful aliases.
define kpanic
    # Break at the panic handler entry. Stops on every panic with full state.
    break panic
    commands
        printf "\n=== KEVLAR PANIC TRAP ===\n"
        info registers
        x/8gx $rsp
        backtrace 20
        printf "===\n"
        # Don't continue — let the user inspect.
    end
end

define kbt
    # Print a kernel backtrace, walking RBP frames.
    set $f = $rbp
    set $i = 0
    while ($f != 0 && $i < 32)
        set $ret = *(unsigned long *)($f + 8)
        printf "  [%d] rbp=%#018lx  ret=%#018lx", $i, $f, $ret
        info symbol $ret
        set $f = *(unsigned long *)($f)
        set $i = $i + 1
    end
end

define kdump
    # Dump panic-style state at the current point.
    printf "\n=== state ===\n"
    info registers
    printf "stack at rsp:\n"
    x/16gx $rsp
    printf "kbt:\n"
    kbt
end

# By default, install panic trap so any panic stops cleanly with state.
kpanic

printf "\n[kevlar.gdbinit] panic breakpoint armed. Type 'c' to run.\n"
printf "Useful commands: kdump, kbt\n"
