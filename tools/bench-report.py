#!/usr/bin/env python3
"""
Kevlar benchmark visualization and comparison tool.

Generates terminal charts and markdown reports comparing Kevlar
performance across profiles and against Linux.

Usage:
    python3 tools/bench-report.py                    # full report
    python3 tools/bench-report.py --format markdown  # markdown table
    python3 tools/bench-report.py --chart             # bar charts
"""
import sys
import os
import argparse

def parse_bench_file(path):
    """Parse BENCH output file into dict of {name: ns_per_iter}."""
    results = {}
    with open(path) as f:
        for line in f:
            parts = line.strip().split()
            if len(parts) >= 5 and parts[0] == 'BENCH':
                results[parts[1]] = int(parts[4])
    return results

def load_all_results(bench_dir='/tmp'):
    """Load all available benchmark results."""
    data = {}
    profiles = ['balanced', 'performance', 'fortress', 'ludicrous']
    for profile in profiles:
        path = f'{bench_dir}/kevlar-bench-{profile}.txt'
        if os.path.exists(path):
            data[profile] = parse_bench_file(path)

    linux_path = f'{bench_dir}/linux-bench-kvm.txt'
    if os.path.exists(linux_path):
        data['linux'] = parse_bench_file(linux_path)

    return data

def format_ns(ns):
    """Format nanoseconds nicely."""
    if ns >= 1_000_000:
        return f'{ns/1_000_000:.1f}ms'
    elif ns >= 1_000:
        return f'{ns/1_000:.1f}µs'
    else:
        return f'{ns}ns'

def bar_chart(label, value, max_val, width=40, char='█'):
    """Render a horizontal bar chart line."""
    if max_val == 0:
        filled = 0
    else:
        filled = int(value / max_val * width)
    bar = char * filled + '░' * (width - filled)
    return f'{label:<20} {bar} {format_ns(value):>8}'

def ratio_color(ratio):
    """ANSI color for ratio."""
    if ratio <= 0.8:
        return '\033[32m'  # green (faster)
    elif ratio <= 1.1:
        return '\033[0m'   # default (ok)
    elif ratio <= 2.0:
        return '\033[33m'  # yellow (slower)
    else:
        return '\033[31m'  # red (regression)

RESET = '\033[0m'

def print_comparison(data, format='terminal'):
    """Print benchmark comparison table."""
    if 'linux' not in data:
        print("No Linux baseline found at /tmp/linux-bench-kvm.txt")
        return

    linux = data['linux']
    # Use balanced as default Kevlar profile
    kevlar_profile = 'balanced'
    if kevlar_profile not in data:
        kevlar_profile = list(k for k in data.keys() if k != 'linux')[0]
    kevlar = data[kevlar_profile]

    # Categorize results
    faster = []
    ok = []
    slower = []
    regression = []

    all_benches = sorted(set(linux.keys()) & set(kevlar.keys()))

    for name in all_benches:
        l, k = linux[name], kevlar[name]
        ratio = k / l
        entry = (name, l, k, ratio)
        if ratio <= 0.9:
            faster.append(entry)
        elif ratio <= 1.1:
            ok.append(entry)
        elif ratio <= 2.0:
            slower.append(entry)
        else:
            regression.append(entry)

    if format == 'markdown':
        print(f'## Kevlar vs Linux KVM ({kevlar_profile} profile)\n')
        print(f'| Benchmark | Linux | Kevlar | Ratio | Status |')
        print(f'|-----------|-------|--------|-------|--------|')
        for name in all_benches:
            l, k = linux[name], kevlar[name]
            ratio = k / l
            if ratio <= 0.9: status = 'Faster'
            elif ratio <= 1.1: status = 'OK'
            elif ratio <= 2.0: status = 'Slower'
            else: status = 'Regression'
            print(f'| {name} | {format_ns(l)} | {format_ns(k)} | {ratio:.2f}x | {status} |')
        print(f'\n**Summary:** {len(faster)} faster, {len(ok)} OK, '
              f'{len(slower)} marginal, {len(regression)} regression')
        return

    # Terminal output with colors and charts
    print(f'\n\033[1m{"═" * 72}\033[0m')
    print(f'\033[1m  Kevlar vs Linux KVM Benchmark Comparison ({kevlar_profile} profile)\033[0m')
    print(f'\033[1m{"═" * 72}\033[0m\n')

    max_ns = max(max(linux.values()), max(kevlar.values()))

    print(f'{"Benchmark":<20} {"Linux":>8} {"Kevlar":>8} {"Ratio":>7}  Status')
    print(f'{"─" * 20} {"─" * 8} {"─" * 8} {"─" * 7}  {"─" * 12}')

    for name in all_benches:
        l, k = linux[name], kevlar[name]
        ratio = k / l
        color = ratio_color(ratio)
        if ratio <= 0.9: status = '✓ faster'
        elif ratio <= 1.1: status = '= ok'
        elif ratio <= 2.0: status = '▽ slower'
        else: status = '✗ REGRESS'
        print(f'{color}{name:<20} {format_ns(l):>8} {format_ns(k):>8} {ratio:>6.2f}x  {status}{RESET}')

    # New benchmarks (no Linux baseline)
    kevlar_only = sorted(set(kevlar.keys()) - set(linux.keys()))
    if kevlar_only:
        print(f'\n\033[36mNew benchmarks (no Linux baseline):\033[0m')
        for name in kevlar_only:
            print(f'  {name:<20} {format_ns(kevlar[name]):>8}')

    print(f'\n\033[1mSummary:\033[0m '
          f'\033[32m{len(faster)} faster\033[0m, '
          f'{len(ok)} OK, '
          f'\033[33m{len(slower)} marginal\033[0m, '
          f'\033[31m{len(regression)} regression\033[0m')

def print_profile_comparison(data):
    """Compare all Kevlar profiles against each other."""
    profiles = [p for p in ['balanced', 'performance', 'fortress', 'ludicrous'] if p in data]
    if len(profiles) < 2:
        print("Need at least 2 profiles for comparison")
        return

    all_benches = set()
    for p in profiles:
        all_benches.update(data[p].keys())
    all_benches = sorted(all_benches)

    print(f'\n\033[1m{"═" * 72}\033[0m')
    print(f'\033[1m  Profile Comparison (ns per iteration)\033[0m')
    print(f'\033[1m{"═" * 72}\033[0m\n')

    header = f'{"Benchmark":<20}'
    for p in profiles:
        header += f' {p:>12}'
    print(header)
    print('─' * 20 + (' ' + '─' * 12) * len(profiles))

    for name in all_benches:
        line = f'{name:<20}'
        vals = [data[p].get(name, 0) for p in profiles]
        min_val = min(v for v in vals if v > 0) if any(v > 0 for v in vals) else 1
        for i, p in enumerate(profiles):
            v = data[p].get(name, 0)
            if v == 0:
                line += f' {"N/A":>12}'
            elif v == min_val:
                line += f' \033[32m{format_ns(v):>12}\033[0m'
            elif v > min_val * 1.5:
                line += f' \033[33m{format_ns(v):>12}\033[0m'
            else:
                line += f' {format_ns(v):>12}'
        print(line)

def print_bar_charts(data):
    """Print horizontal bar charts for key benchmarks."""
    if 'linux' not in data:
        print("No Linux baseline")
        return

    linux = data['linux']
    kevlar = data.get('balanced', data.get('performance', {}))

    categories = {
        'Fast syscalls (ns)': ['getpid', 'gettid', 'getuid', 'clock_gettime', 'brk'],
        'File ops (ns)': ['open_close', 'stat', 'fstat', 'read_null', 'write_null'],
        'Memory (ns)': ['mmap_munmap', 'mmap_fault', 'mprotect', 'brk'],
        'IPC (ns)': ['pipe', 'pipe_pingpong', 'socketpair', 'eventfd'],
        'Signals (ns)': ['sigaction', 'sigprocmask', 'signal_delivery'],
    }

    for cat_name, benches in categories.items():
        print(f'\n\033[1m{cat_name}\033[0m')
        available = [b for b in benches if b in linux and b in kevlar]
        if not available:
            continue
        max_val = max(max(linux.get(b, 0), kevlar.get(b, 0)) for b in available)
        for b in available:
            l = linux.get(b, 0)
            k = kevlar.get(b, 0)
            print(f'  Linux  {bar_chart(b, l, max_val, width=30, char="▓")}')
            color = ratio_color(k / l) if l > 0 else ''
            print(f'  {color}Kevlar {bar_chart(b, k, max_val, width=30, char="█")}{RESET}')
            print()

def main():
    parser = argparse.ArgumentParser(description='Kevlar benchmark report')
    parser.add_argument('--format', choices=['terminal', 'markdown'], default='terminal')
    parser.add_argument('--chart', action='store_true', help='Show bar charts')
    parser.add_argument('--profiles', action='store_true', help='Compare profiles')
    parser.add_argument('--all', action='store_true', help='Show everything')
    args = parser.parse_args()

    data = load_all_results()
    if not data:
        print("No benchmark data found in /tmp/. Run benchmarks first.")
        sys.exit(1)

    if args.all or (not args.chart and not args.profiles):
        print_comparison(data, args.format)

    if args.chart or args.all:
        print_bar_charts(data)

    if args.profiles or args.all:
        print_profile_comparison(data)

if __name__ == '__main__':
    main()
