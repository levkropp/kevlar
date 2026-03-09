# Phase 7: Benchmarks, CI Matrix, and Smarter Tooling

With the safety profile infrastructure in place (Phases 0-6), we need to
actually *measure* their impact.  This post covers the benchmark suite,
cross-profile CI, and some quality-of-life tooling improvements.

## Micro-benchmark suite

`benchmarks/bench.c` is a static musl binary included in the initramfs.
It measures eight fundamental kernel operations:

| Benchmark    | What it measures                         |
|--------------|------------------------------------------|
| `getpid`     | Bare syscall round-trip                  |
| `read_null`  | `read(/dev/null, 1)` latency            |
| `write_null` | `write(/dev/null, 1)` latency           |
| `pipe`       | `pipe` read/write throughput (4 KB chunks) |
| `fork_exit`  | `fork()` + `waitpid()` latency          |
| `open_close` | `open()` + `close()` a tmpfs file       |
| `mmap_fault` | Anonymous mmap + page fault throughput   |
| `stat`       | `stat()` latency                         |

Output is machine-parseable: `BENCH <name> <iters> <total_ns> <per_iter_ns>`.
A `--quick` flag reduces iteration counts for QEMU TCG, where emulation
adds ~10,000x overhead.

## Python runner and comparison

`benchmarks/run-benchmarks.py` wraps the whole flow:

```bash
# Run on Kevlar (builds, boots QEMU, parses output)
python3 benchmarks/run-benchmarks.py run --profile balanced

# Run on native Linux for baseline
python3 benchmarks/run-benchmarks.py linux --binary ./bench

# Compare JSON result files side-by-side
python3 benchmarks/run-benchmarks.py compare kevlar.json linux.json

# Run all four safety profiles
python3 benchmarks/run-benchmarks.py all-profiles
```

Or via Make:
```bash
make bench PROFILE=balanced
make bench-all
make bench-compare BENCH_FILES="a.json b.json"
```

## CI matrix: all four profiles

The CI workflow now tests all four safety profiles in parallel:

```yaml
strategy:
  fail-fast: false
  matrix:
    profile: [fortress, balanced, performance, ludicrous]
```

Each profile gets its own `cargo check` step using the correct target spec
(`x64-unwind.json` for fortress/balanced, `x64.json` for performance/ludicrous).
A separate `clippy` job runs on the balanced profile, and `rustfmt` runs
independently.

## QEMU port conflict handling

Previous QEMU sessions sometimes lingered, holding ports 20022 and 20080.
`run-qemu.py` now detects port conflicts at startup using `socket.bind()`,
identifies the holder via `ss -tlnp`, and kills stale QEMU processes
automatically.  This eliminates the "address already in use" failures that
plagued iterative development.

## Build system fixes

- **`INIT_SCRIPT` override**: The Makefile now conditionally sets
  `INIT_SCRIPT=/bin/sh` only when not already set, so `make bench` can
  override it to `/bin/bench`.
- **`build.rs` env tracking**: `kernel/build.rs` declares
  `cargo::rerun-if-env-changed=INIT_SCRIPT` so Cargo recompiles when the
  init script changes — no more stale binaries after switching between
  shell and bench modes.
- **Docker context**: The build context is now the repo root (not `testing/`),
  allowing the Dockerfile to `COPY benchmarks/bench.c` directly.

## Early results (QEMU TCG, quick mode)

These numbers are from software emulation and only useful for relative
comparison between profiles, not absolute performance:

| Benchmark    | Kevlar (ns/op) | Linux (ns/op) |
|--------------|---------------:|---------------:|
| getpid       |      2,233,600 |           264  |
| read_null    |      4,289,000 |           306  |
| write_null   |      4,164,600 |           288  |
| pipe         |     36,718,750 |         1,342  |

The ~10,000x factor is pure TCG overhead.  Real performance comparison
requires KVM (`make run KVM=1`) or native boot, which is where this
infrastructure will shine as Kevlar matures.

## What's next

- Fix the GPF-in-userspace bug that crashes fork and later benchmarks
- KVM-accelerated benchmark runs for meaningful Kevlar vs Linux numbers
- Asterinas comparison using the same bench binary
- Profile-to-profile comparison to quantify the cost of safety features
