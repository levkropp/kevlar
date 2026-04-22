#!/usr/bin/env python3
"""Summarize KERNEL_PTR_LEAK / PAGE_ZERO_MISS / LEAK_PAGE_SCAN output.

Run against a test-xfce or test-lxde log to get a one-screen summary:
- How many leaks fired, by process / ip.
- How many PAGE_ZERO_MISS, by site.
- Kernel-pointer density distribution from LEAK_PAGE_SCAN.

Usage: python3 tools/analyze-leak-log.py /tmp/kevlar-test-xfce-balanced.log
"""
import sys
import re
from collections import Counter, defaultdict


def strip_ansi(s: str) -> str:
    return re.sub(r"\x1b\[[0-9;]*m", "", s)


def main() -> int:
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} <log-file> [log-file ...]", file=sys.stderr)
        return 2

    # Aggregate across all files.
    leak_by_ip = Counter()
    leak_by_cmd = Counter()
    leak_by_paddr = Counter()
    zero_miss_by_site = Counter()
    zero_miss_with_kptr = 0
    zero_miss_total = 0
    leak_scan_density = []   # list of (gpr_name, kernel_ptrs, region)
    leak_scan_by_region = Counter()

    leak_re = re.compile(
        r"KERNEL_PTR_LEAK: pid=(\d+) fault_addr=(0x[0-9a-f]+)"
        r"(?: ip=(0x[0-9a-f]+))?")
    cmd_re = re.compile(r"SIGSEGV: pid=\d+ cmd=(\S+)")
    zero_re = re.compile(
        r"PAGE_ZERO_MISS site=(\S+) paddr=(0x[0-9a-f]+) "
        r"first_nz_off=(0x[0-9a-f]+) nonzero_words=(\d+) "
        r"kernel_ptr_words=(\d+)")
    scan_re = re.compile(
        r"LEAK_PAGE_SCAN (\w+): vaddr=(0x[0-9a-f]+) paddr=(0x[0-9a-f]+) "
        r"kernel_ptrs=(\d+) in (\S+) region")

    for path in sys.argv[1:]:
        try:
            with open(path) as f:
                lines = [strip_ansi(line) for line in f]
        except FileNotFoundError:
            print(f"  WARN: {path} not found, skipping", file=sys.stderr)
            continue

        last_ip = None
        for line in lines:
            m = leak_re.search(line)
            if m:
                pid, addr, ip = m.groups()
                leak_by_paddr[addr] += 1
                if ip:
                    last_ip = ip
                    leak_by_ip[ip] += 1
            m = cmd_re.search(line)
            if m:
                leak_by_cmd[m.group(1)] += 1
            m = zero_re.search(line)
            if m:
                site, paddr, nz_off, nz_count, kptr_count = m.groups()
                zero_miss_by_site[site] += 1
                zero_miss_total += 1
                if int(kptr_count) > 0:
                    zero_miss_with_kptr += 1
            m = scan_re.search(line)
            if m:
                gpr, vaddr, paddr, kptrs, region = m.groups()
                leak_scan_density.append((gpr, int(kptrs), region))
                leak_scan_by_region[region] += 1

    print("=" * 66)
    print(" Kevlar leak-instrumentation summary")
    print("=" * 66)
    print()
    print(f" KERNEL_PTR_LEAK events   : {sum(leak_by_cmd.values())}")
    print(f" Unique leaked paddrs     : {len(leak_by_paddr)}")
    if leak_by_cmd:
        print(" By process cmdline:")
        for cmd, n in leak_by_cmd.most_common(5):
            print(f"   {cmd}: {n}")
    if leak_by_ip:
        print(" By fault ip (top 5):")
        for ip, n in leak_by_ip.most_common(5):
            print(f"   {ip}: {n}")
    print()
    print(f" PAGE_ZERO_MISS events    : {zero_miss_total}"
          f"  (with kernel-VA words: {zero_miss_with_kptr})")
    if zero_miss_by_site:
        print(" By site:")
        for site, n in zero_miss_by_site.most_common():
            print(f"   {site}: {n}")
    print()
    print(f" LEAK_PAGE_SCAN samples   : {len(leak_scan_density)}")
    if leak_scan_by_region:
        print(" Region classification:")
        for region, n in leak_scan_by_region.most_common():
            print(f"   {region}: {n}")
    if leak_scan_density:
        # Density histogram — kernel-pointer count per user page.
        bins = [1, 2, 5, 10, 50, 100, 1000]
        hist = [0] * (len(bins) + 1)
        for _, kp, _ in leak_scan_density:
            bucket = len(bins)
            for i, b in enumerate(bins):
                if kp <= b:
                    bucket = i
                    break
            hist[bucket] += 1
        print(" Kernel-pointer density per scanned user page:")
        labels = ["=1", "=2", "3-5", "6-10", "11-50", "51-100",
                  "101-1000", ">1000"]
        for label, cnt in zip(labels, hist):
            if cnt:
                print(f"   {label:>8}: {cnt}")
        print()
        print(" Interpretation:")
        print("   density=1           → single coincidental leaked value")
        print("   density=2-10        → small struct in user page")
        print("   density>10          → page-recycle from kernel slab")
    return 0


if __name__ == "__main__":
    sys.exit(main())
