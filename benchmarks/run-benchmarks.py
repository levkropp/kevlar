#!/usr/bin/env python3
"""
Kevlar benchmark runner.

Boots the kernel in QEMU, runs /bin/bench via the init script, captures
serial output, and parses BENCH lines into a JSON results file.

Usage:
    python3 benchmarks/run-benchmarks.py [--profile PROFILE] [--output results.json]
    python3 benchmarks/run-benchmarks.py --compare results-kevlar.json results-linux.json

For Linux comparison, run the same bench binary in a Linux VM or container:
    docker run --rm -v $PWD/benchmarks:/b alpine sh -c 'apk add build-base && \
        gcc -static -O2 -o /bench /b/bench.c && /bench'
"""
import argparse
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import time


def run_kevlar_bench(profile="balanced", timeout_secs=120, arch="x64"):
    """Boot Kevlar in QEMU, run /bin/bench, return parsed results."""
    # Build with the bench init script.
    env = os.environ.copy()
    env["INIT_SCRIPT"] = "/bin/bench"
    env["PROFILE"] = profile

    print(f"Building Kevlar ({profile} profile)...", file=sys.stderr)
    rc = subprocess.run(
        ["make", "build", f"PROFILE={profile}"],
        env=env,
        capture_output=True,
        text=True,
    )
    if rc.returncode != 0:
        print(f"Build failed:\n{rc.stderr}", file=sys.stderr)
        sys.exit(1)

    # Boot QEMU and capture serial output.
    print(f"Booting QEMU ({profile})...", file=sys.stderr)
    kernel_elf = f"kevlar.{arch}.elf"

    qemu_cmd = [
        "python3", "tools/run-qemu.py",
        "--arch", arch,
        "--append-cmdline", "init=/bin/bench",
        kernel_elf,
    ]

    try:
        result = subprocess.run(
            qemu_cmd,
            capture_output=True,
            text=True,
            timeout=timeout_secs,
            env=env,
        )
        output = result.stdout + result.stderr
    except subprocess.TimeoutExpired as e:
        output = (e.stdout or b"").decode("utf-8", errors="replace")
        output += (e.stderr or b"").decode("utf-8", errors="replace")

    return parse_bench_output(output, f"kevlar-{profile}")


def run_linux_bench(bench_binary="./bench"):
    """Run the bench binary natively on Linux, return parsed results."""
    print("Running bench on Linux...", file=sys.stderr)
    result = subprocess.run(
        [bench_binary],
        capture_output=True,
        text=True,
        timeout=120,
    )
    return parse_bench_output(result.stdout, "linux")


def parse_bench_output(output, label):
    """Parse BENCH lines from serial/stdout output."""
    results = {"label": label, "benchmarks": {}}

    for line in output.splitlines():
        line = line.strip()
        # Strip ANSI escape codes.
        line = re.sub(r"\x1b\[[0-9;]*m", "", line)
        line = re.sub(r"\r", "", line)

        m = re.match(r"^BENCH (\S+) (\d+) (\d+) (\d+)$", line)
        if m:
            name, iters, total_ns, per_iter_ns = m.groups()
            results["benchmarks"][name] = {
                "iterations": int(iters),
                "total_ns": int(total_ns),
                "per_iter_ns": int(per_iter_ns),
            }

        m = re.match(r"^BENCH_EXTRA (\S+) (.+)$", line)
        if m:
            name, value = m.groups()
            try:
                results["benchmarks"][name] = {"value": float(value)}
            except ValueError:
                results["benchmarks"][name] = {"value": value}

    return results


def compare_results(results_list):
    """Print a comparison table of multiple result sets."""
    # Collect all benchmark names.
    all_names = set()
    for r in results_list:
        all_names.update(r["benchmarks"].keys())
    all_names = sorted(all_names)

    # Header.
    labels = [r["label"] for r in results_list]
    header = f"{'Benchmark':<20s}"
    for label in labels:
        header += f"  {label:>18s}"
    if len(labels) >= 2:
        header += f"  {'ratio':>10s}"
    print(header)
    print("-" * len(header))

    for name in all_names:
        row = f"{name:<20s}"
        values = []
        for r in results_list:
            b = r["benchmarks"].get(name, {})
            if "per_iter_ns" in b:
                val = f"{b['per_iter_ns']} ns/op"
                values.append(b["per_iter_ns"])
            elif "value" in b:
                val = str(b["value"])
                values.append(None)
            else:
                val = "—"
                values.append(None)
            row += f"  {val:>18s}"

        # Ratio of first vs second.
        if len(values) >= 2 and values[0] is not None and values[1] is not None and values[1] != 0:
            ratio = values[0] / values[1]
            row += f"  {ratio:>9.2f}x"

        print(row)


def main():
    parser = argparse.ArgumentParser(description="Kevlar benchmark runner")
    sub = parser.add_subparsers(dest="command")

    run_p = sub.add_parser("run", help="Run benchmarks on Kevlar")
    run_p.add_argument("--profile", default="balanced")
    run_p.add_argument("--output", default=None, help="JSON output file")
    run_p.add_argument("--timeout", type=int, default=120)

    linux_p = sub.add_parser("linux", help="Run benchmarks on host Linux")
    linux_p.add_argument("--binary", default="./bench")
    linux_p.add_argument("--output", default=None)

    cmp_p = sub.add_parser("compare", help="Compare result files")
    cmp_p.add_argument("files", nargs="+", help="JSON result files")

    all_p = sub.add_parser("all-profiles", help="Run benchmarks for all Kevlar profiles")
    all_p.add_argument("--output-dir", default="benchmarks/results")

    args = parser.parse_args()

    if args.command == "run":
        results = run_kevlar_bench(profile=args.profile, timeout_secs=args.timeout)
        if args.output:
            with open(args.output, "w") as f:
                json.dump(results, f, indent=2)
            print(f"Results written to {args.output}", file=sys.stderr)
        else:
            print(json.dumps(results, indent=2))

    elif args.command == "linux":
        results = run_linux_bench(bench_binary=args.binary)
        if args.output:
            with open(args.output, "w") as f:
                json.dump(results, f, indent=2)
        else:
            print(json.dumps(results, indent=2))

    elif args.command == "compare":
        results_list = []
        for path in args.files:
            with open(path) as f:
                results_list.append(json.load(f))
        compare_results(results_list)

    elif args.command == "all-profiles":
        os.makedirs(args.output_dir, exist_ok=True)
        all_results = []
        for profile in ["fortress", "balanced", "performance", "ludicrous"]:
            print(f"\n=== Profile: {profile} ===", file=sys.stderr)
            results = run_kevlar_bench(profile=profile)
            outfile = os.path.join(args.output_dir, f"{profile}.json")
            with open(outfile, "w") as f:
                json.dump(results, f, indent=2)
            all_results.append(results)
        compare_results(all_results)

    else:
        parser.print_help()


if __name__ == "__main__":
    main()
