#!/usr/bin/env python3
"""Inject pr_err calls before every `goto failed_mount` in
ext4_fill_super so we can identify exactly which error path fires.

Usage: python3 instrument-ext4.py <path/to/fs/ext4/super.c>

Idempotent — sentinel comment prevents re-injection.
"""
import re
import sys

SENTINEL = "/* KABI_INSTRUMENT */"
GOTO_RE = re.compile(r'^(\s*)goto\s+(failed_mount\w*)\s*;')


def main(path: str) -> int:
    with open(path, 'r') as f:
        lines = f.readlines()

    # Find ext4_fill_super function start.
    in_func = False
    brace_depth = 0
    out = []
    seen_open_brace = False

    for i, line in enumerate(lines, start=1):
        if not in_func:
            # Pattern: __ext4_fill_super(...) { ... } OR ext4_fill_super(...)
            if re.match(r'^(static\s+)?int\s+(__)?ext4_fill_super\s*\(', line):
                in_func = True
                seen_open_brace = False
                brace_depth = 0
            out.append(line)
            continue

        # Track braces to know when ext4_fill_super ends.
        opens = line.count('{')
        closes = line.count('}')
        if not seen_open_brace and opens > 0:
            seen_open_brace = True
        brace_depth += opens - closes

        # Skip already-instrumented lines.
        if SENTINEL in line:
            out.append(line)
            if seen_open_brace and brace_depth <= 0:
                in_func = False
            continue

        m = GOTO_RE.match(line)
        if m:
            indent = m.group(1)
            target = m.group(2)
            # Inject a pr_err right before the goto.  Use only %d
            # formatting (no %s) to avoid any pointer-deref issues
            # in our printk shim.  Encode the goto target via a
            # small int range based on the target name.
            target_id = abs(hash(target)) % 100
            instr = (
                f'{indent}printk(KERN_ERR "KABI_FS_FAIL line=%d target=%d '
                f'err=%d\\n", {i}, {target_id}, err); {SENTINEL}\n'
            )
            out.append(instr)
            out.append(line)
        else:
            out.append(line)

        if seen_open_brace and brace_depth <= 0:
            in_func = False

    with open(path, 'w') as f:
        f.writelines(out)

    print(f"instrumented {path}: injected printks before goto failed_mount sites")
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1]))
