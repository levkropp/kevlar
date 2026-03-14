# M6.6 Phase 1: Expand Benchmarks and Establish Baseline

**Duration:** ~1 day
**Goal:** Add missing benchmarks and run the full suite on both Linux and Kevlar under KVM.

## New benchmarks to add to bench.c

The existing 24 benchmarks cover the M5 hot paths.  Add 4 more for
syscalls changed or added since M5:

### 1. `sched_yield` — scheduler context switch
```c
static void bench_sched_yield(void) {
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        sched_yield();
    }
    report("sched_yield", iters, now_ns() - start);
}
```

### 2. `getpriority` / `setpriority` — new in M6.5
```c
static void bench_getpriority(void) {
    int iters = ITERS(500000, 5000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        getpriority(PRIO_PROCESS, 0);
    }
    report("getpriority", iters, now_ns() - start);
}
```

### 3. `read_zero` — /dev/zero read (new device)
```c
static void bench_read_zero(void) {
    int fd = open("/dev/zero", O_RDONLY);
    if (fd < 0) { printf("BENCH_SKIP read_zero\n"); return; }
    char buf[4096];
    int iters = ITERS(200000, 2000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        read(fd, buf, sizeof(buf));
    }
    report("read_zero", iters, now_ns() - start);
    close(fd);
}
```

### 4. `signal_delivery` — raise + handler round-trip
```c
static volatile int sig_count;
static void sig_handler(int sig) { (void)sig; sig_count++; }

static void bench_signal_delivery(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = sig_handler;
    sigaction(SIGUSR1, &sa, NULL);
    sig_count = 0;
    int iters = ITERS(200000, 2000);
    long long start = now_ns();
    for (int i = 0; i < iters; i++) {
        raise(SIGUSR1);
    }
    report("signal_delivery", iters, now_ns() - start);
}
```

## Baseline procedure

1. Build Kevlar: `make build PROFILE=balanced`
2. Run Kevlar benchmarks: `make bench-kvm`
   - Uses `--kvm -mem-prealloc` for stable measurements
   - Outputs `BENCH <name> <iters> <total_ns> <per_iter_ns>`
3. Run Linux benchmarks: `python3 tools/run-all-benchmarks.py --linux --kvm`
4. Save results to `build/bench-m6.6-baseline.csv`

## Expected output format

```
kernel,benchmark,iters,total_ns,per_iter_ns
kevlar,getpid,1000000,200000000,200
linux,getpid,1000000,338000000,338
kevlar,read_null,500000,257000000,514
linux,read_null,500000,285000000,570
...
```

## Integration

- Add new benchmarks to the `benchmarks[]` registry in bench.c
- Add `#include <sys/resource.h>` for getpriority
- Register as extended (not core) benchmarks
- Rebuild initramfs to include updated bench binary
