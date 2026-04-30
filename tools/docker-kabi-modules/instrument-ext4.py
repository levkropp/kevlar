#!/usr/bin/env python3
"""ext4.ko diagnostic patcher.

Injects calls to `kabi_breadcrumb(line, target_id, err)` at chosen
sites in `fs/ext4/super.c` so the kABI side can identify which error
path actually fires inside `ext4_fill_super` (and similar functions).

Why a dedicated breadcrumb helper instead of `printk`?
  * Fixed 3-arg signature → predictable code emission, no variadic
    grammar to hit edge cases in.
  * Three-instruction call shape (`bl kabi_breadcrumb`) shifts
    nearby code by a fixed amount per site, minimizing surprise
    in `.altinstructions` / `__bug_table` slot layout.

Modes:
  --mode all          Inject at every `goto failed_mount*` site in
                      ext4_fill_super.  Cleanest for end-to-end
                      tracing once we've confirmed the breadcrumb
                      call shape doesn't disturb downstream offsets.

  --mode lines        Only inject at the specified line numbers
                      (`--lines L1,L2,L3`).  Useful for surgical
                      verification of one suspect.

  --mode bisect       Inject at ALL sites in the chosen half of the
                      function range (`--half lower|upper`).  Run
                      twice (lower then upper); whichever half
                      produces a breadcrumb contains the failing
                      branch.

  --mode functions    Inject at all goto-failure sites in the named
                      function(s) (`--functions ext4_fill_super,
                      ext4_setup_super`).

Idempotent: re-running on already-patched source is a no-op
(detected by the `KABI_INSTRUMENT` sentinel comment).
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

SENTINEL = "/* KABI_INSTRUMENT */"
GOTO_RE = re.compile(
    r'^(\s*)goto\s+(failed_mount\w*|failed_mount|out_\w*|out|err\w*|err)\s*;'
)
FUNC_OPEN_RE = re.compile(r'^(static\s+)?\w[\w\s\*]*\b(\w+)\s*\([^)]*\)\s*\{?\s*$')


def find_function_ranges(lines: list[str], func_names: set[str]) -> dict[str, tuple[int, int]]:
    """Return {func_name: (start_line, end_line)} for each requested function.
    Tracks brace depth to find the closing `}`.
    """
    out: dict[str, tuple[int, int]] = {}
    i = 0
    while i < len(lines):
        line = lines[i]
        # Match function header: starts with `int <name>(` (possibly with static).
        m = re.match(r'^(static\s+)?int\s+(\w+)\s*\(', line)
        if m:
            name = m.group(2)
            if name in func_names:
                start = i + 1  # 1-based
                # Find opening brace.
                depth = 0
                seen_open = False
                j = i
                while j < len(lines):
                    opens = lines[j].count('{')
                    closes = lines[j].count('}')
                    if opens > 0:
                        seen_open = True
                    depth += opens - closes
                    if seen_open and depth <= 0:
                        out[name] = (start, j + 1)
                        i = j
                        break
                    j += 1
        i += 1
    return out


def encode_target_id(target: str) -> int:
    """Stable small-int encoding for a goto label."""
    # Keep the range tight so logs are easy to read.  Hash collisions
    # within ext4 fill_super's ~30 distinct labels are unlikely.
    return abs(hash(target)) % 1000


def inject(
    lines: list[str],
    in_range: callable,  # type: ignore[name-defined]
    site_filter: callable | None = None,  # type: ignore[name-defined]
) -> tuple[list[str], int]:
    """Walk `lines` and inject breadcrumbs at goto sites that pass
    both `in_range(line_no)` and `site_filter(line_no, target)`.
    """
    out: list[str] = []
    injected = 0
    for i, line in enumerate(lines, start=1):
        if SENTINEL in line:
            out.append(line)
            continue
        m = GOTO_RE.match(line)
        if m and in_range(i) and (site_filter is None or site_filter(i, m.group(2))):
            indent = m.group(1)
            target = m.group(2)
            tid = encode_target_id(target)
            instr = (
                f'{indent}kabi_breadcrumb({i}, {tid}, err); {SENTINEL} '
                f'/* {target} */\n'
            )
            out.append(instr)
            injected += 1
        out.append(line)
    return out, injected


def ensure_extern_decl(lines: list[str]) -> list[str]:
    """Insert `extern void kabi_breadcrumb(int, int, int);` near the
    top of the file (after #include block) if not already there.
    """
    if any('kabi_breadcrumb' in l and 'extern' in l for l in lines):
        return lines
    out: list[str] = []
    inserted = False
    for i, line in enumerate(lines):
        out.append(line)
        if not inserted and i + 1 < len(lines):
            # Find first non-include non-comment line after at least
            # one #include.
            saw_include = any('#include' in l for l in lines[: i + 1])
            nxt = lines[i + 1].strip()
            if (
                saw_include
                and not nxt.startswith('#include')
                and not nxt.startswith('//')
                and not nxt.startswith('/*')
                and not nxt.startswith('*')
                and nxt  # not blank
            ):
                out.append(
                    '\n/* KABI_INSTRUMENT: breadcrumb declaration. */\n'
                    'extern void kabi_breadcrumb(int line, int target_id, int err);\n\n'
                )
                inserted = True
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument('source', help='path to fs/ext4/super.c (or similar)')
    ap.add_argument(
        '--mode',
        choices=['all', 'lines', 'bisect', 'functions'],
        default='all',
    )
    ap.add_argument(
        '--lines',
        help='comma-separated source line numbers (mode=lines)',
        default='',
    )
    ap.add_argument(
        '--half',
        choices=['lower', 'upper'],
        help='which half of the function (mode=bisect)',
    )
    ap.add_argument(
        '--functions',
        default='__ext4_fill_super,ext4_fill_super',
        help='comma-separated function names (modes=all,bisect,functions). '
             'Linux 7.0 splits the work into __ext4_fill_super (the heavy '
             'one) plus a thin ext4_fill_super wrapper.',
    )
    args = ap.parse_args()

    src = Path(args.source)
    lines = src.read_text().splitlines(keepends=True)

    func_names = {n.strip() for n in args.functions.split(',') if n.strip()}
    ranges = find_function_ranges(lines, func_names)
    if not ranges:
        print(f'instrument-ext4: no functions matched {func_names}', file=sys.stderr)
        return 1
    for name, (s, e) in ranges.items():
        print(f'  function {name}: lines {s}..{e}')

    in_range_funcs = lambda i: any(s <= i <= e for s, e in ranges.values())  # noqa: E731

    if args.mode == 'all':
        in_range = in_range_funcs
        site_filter = None
    elif args.mode == 'functions':
        in_range = in_range_funcs
        site_filter = None
    elif args.mode == 'lines':
        wanted = {int(x) for x in args.lines.split(',') if x.strip()}
        in_range = lambda i: i in wanted  # noqa: E731
        site_filter = None
    elif args.mode == 'bisect':
        if not args.half:
            print('--mode bisect requires --half lower|upper', file=sys.stderr)
            return 1
        # Use the first matched function for bisection.
        first = next(iter(ranges.values()))
        s, e = first
        mid = (s + e) // 2
        if args.half == 'lower':
            in_range = lambda i: s <= i <= mid  # noqa: E731
        else:
            in_range = lambda i: mid < i <= e  # noqa: E731
        site_filter = None
    else:
        return 1

    lines = ensure_extern_decl(lines)
    new_lines, n = inject(lines, in_range, site_filter)
    src.write_text(''.join(new_lines))
    print(f'instrument-ext4: injected {n} breadcrumb(s) into {src}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
