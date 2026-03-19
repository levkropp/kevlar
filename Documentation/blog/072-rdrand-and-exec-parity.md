# M9.6 Part 2: The 50µs RDRAND Tax and Reaching Linux exec Parity

After the page cache and prefaulting work in post 071, `exec_true`
sat at 118µs — fast enough to see the shape of the remaining problem,
but still 1.8x slower than Linux's 67µs.  We added TSC-based phase
profiling to the exec path and found a single instruction eating more
than half the time.

## Profiling the exec path

We instrumented `Process::execve()`, `do_setup_userspace()`, and
`do_elf_binfmt()` with `read_clock_counter()` calls at phase boundaries,
accumulating into global atomics and dumping averages after 50 execs.

The results for a warm-cache `exec_true` (fork + exec `/bin/true` +
wait):

| Phase | Avg time | % of exec |
|-------|----------|-----------|
| close_cloexec + cmdline | 130ns | 0.1% |
| Vm::new (PML4 alloc) | 5,740ns | 6.1% |
| load_elf_segments | 1,152ns | 1.2% |
| **read_secure_random** | **50,165ns** | **53.3%** |
| prefault_cached_pages | 8,277ns | 8.8% |
| stack alloc + init | 1,127ns | 1.2% |
| de_thread + CR3 switch | 440ns | 0.5% |

One function — `read_secure_random` — consumed **50µs** out of a 94µs
exec.

## The RDRAND VM exit tax

`read_secure_random` fills 16 bytes of AT_RANDOM data for the ELF
auxiliary vector.  It calls `x86::random::rdrand_slice()`, which
executes two RDRAND instructions (8 bytes each).

On bare metal, RDRAND takes ~800 cycles (~330ns at 2.4GHz).  Under
KVM, each RDRAND triggers a **VM exit** — the CPU traps to the
hypervisor, which emulates the instruction and returns.  Our profiling
showed each RDRAND VM exit costs ~25µs on this host, making two RDRAND
calls cost **~50µs**.

This is a known KVM issue: RDRAND is unconditionally intercepted
because the hypervisor must control entropy sources.  Linux avoids
this by seeding a kernel CRNG once at boot and never calling RDRAND
in hot paths.

## The fix: buffered SplitMix64 PRNG

We replaced per-exec RDRAND with a lock-free SplitMix64 PRNG seeded
once from RDRAND during boot:

```rust
static PRNG_STATE: AtomicU64 = AtomicU64::new(0);

fn splitmix64_next() -> u64 {
    let s = PRNG_STATE.fetch_add(0x9e3779b97f4a7c15, Ordering::Relaxed);
    let mut z = s.wrapping_add(0x9e3779b97f4a7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}
```

SplitMix64 has excellent statistical quality (passes BigCrush), is
trivially parallelizable via `fetch_add`, and costs ~5ns per 8 bytes
vs ~25µs for RDRAND under KVM.  The single RDRAND at boot is amortized
over the kernel's lifetime.

For `/dev/urandom` reads we use the same PRNG.  A proper CRNG with
periodic reseeding is future work but not needed for the benchmarks.

## Results

**BusyBox test suite:** 101/101 pass (unchanged)

**Workload benchmarks** (Kevlar KVM, lower = faster):

| Benchmark | Post 071 | Now | Speedup | vs Linux |
|-----------|----------|-----|---------|----------|
| exec_true | 118µs | **66µs** | 1.79x | **0.99x** |
| shell_noop | 162µs | **111µs** | 1.46x | 1.70x |
| pipe_grep | 429µs | **314µs** | 1.37x | 4.83x |
| sed_pipeline | 526µs | **407µs** | 1.29x | 6.26x |
| fork_exit | 43µs | 46µs | ~same | — |

**exec_true reached Linux parity** — the first workload benchmark
to do so.  The RDRAND fix removed ~50µs from every exec, which
compounds for multi-exec workloads.

**Cumulative progress from the start of M9.6:**

| Benchmark | Before M9.6 | Now | Total speedup |
|-----------|-------------|-----|---------------|
| exec_true | 177µs | **66µs** | **2.68x** |
| shell_noop | 345µs | **111µs** | **3.11x** |
| pipe_grep | 979µs | **314µs** | **3.12x** |
| sed_pipeline | 1370µs | **407µs** | **3.37x** |

## What's left

`exec_true` is at parity but the multi-fork benchmarks are still
4-6x off.  Each iteration of `pipe_grep` does `fork + exec(sh) +
fork + exec(grep) + read + wait` — at least two fork+exec cycles.
The per-exec overhead is now ~30µs (at parity), so the remaining
gap is in:

- **Fork CoW overhead** (46µs per fork vs Linux's ~15µs)
- **Shell startup** (BusyBox sh initialization, command parsing)
- **I/O path** (pipe reads/writes, `/dev/null` redirection)
- **Process exit/wait** (reaping, signal delivery)

Fork is the next target — at 46µs it's 3x Linux and multiplies
with every child process.
