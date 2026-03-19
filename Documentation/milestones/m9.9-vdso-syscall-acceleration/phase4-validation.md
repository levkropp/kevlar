# M9.9 Phase 4: Validation + Benchmark

**Target:** Verify all vDSO optimizations, confirm no regressions, document results.

## Validation checklist

### Correctness

- [ ] `getpid()` returns correct tgid after fork (child gets new PID)
- [ ] `getpid()` returns namespace-local PID in PID namespaces
- [ ] `gettid()` returns correct TID (== PID for main thread)
- [ ] `gettid()` falls back to syscall in multi-threaded processes
- [ ] `getuid()` returns 0 (root) by default
- [ ] `getuid()` updates after `setuid()` call
- [ ] `getpriority(PRIO_PROCESS, 0)` returns `20 - nice`
- [ ] `getpriority()` falls back to syscall for non-self queries
- [ ] `uname()` returns correct strings (sysname, release, machine)
- [ ] vDSO ELF is valid — `readelf -a /proc/self/maps` equivalent
- [ ] musl's `__vdsosym()` discovers all new symbols

### Performance

- [ ] getpid < 15ns (currently 77ns)
- [ ] gettid < 15ns (currently 80ns)
- [ ] getuid < 15ns (currently 76ns)
- [ ] getpriority < 15ns (currently 80ns)
- [ ] uname < 50ns (currently 145ns)
- [ ] No regression on any other benchmark (0 marginal, 0 regression)

### Integration

- [ ] `make test-contracts` — 105 PASS, 0 FAIL
- [ ] `make test-m6` — all threading tests pass (SMP, 4 CPUs)
- [ ] `make check-all-profiles` — all 4 profiles compile clean
- [ ] Alpine boot still works (getpid used extensively by init)

## Benchmark procedure

```bash
# 1. Build release
make RELEASE=1 build

# 2. Run Linux baseline (if stale)
make bench-linux

# 3. Run Kevlar benchmark
make bench-kvm

# 4. Compare — expect 5 new "faster" entries
make bench-report
```

## Expected final benchmark table

| Syscall | Linux KVM | Kevlar (before) | Kevlar (after) | Speedup |
|---------|-----------|-----------------|----------------|---------|
| getpid | 86ns | 77ns | ~10ns | **8.6x vs Linux** |
| gettid | 90ns | 80ns | ~10ns | **9.0x vs Linux** |
| getuid | 84ns | 76ns | ~10ns | **8.4x vs Linux** |
| getpriority | 86ns | 80ns | ~10ns | **8.6x vs Linux** |
| uname | 162ns | 145ns | ~40ns | **4.1x vs Linux** |

## Documentation

After validation, update:
- `Documentation/milestones/m9.9-vdso-syscall-acceleration/README.md` — mark phases complete
- Memory file for project status — update M9.9 status
