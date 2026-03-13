# Phase 1: Comparative Test Harness

**Duration:** ~3-4 days
**Prerequisite:** M6
**Goal:** Build infrastructure to run identical tests on both Linux and Kevlar, compare results.

## Scope

The test harness is the foundation for all subsequent phases. It must:
1. Boot both Linux and Kevlar (in separate QEMU instances)
2. Run identical test binaries on both
3. Capture output (stdout, stderr, exit code, file system state)
4. Compare results and report divergences
5. Integrate with CI/CD

## Architecture

### Test Runner (`tools/compare-contracts.py`)

```bash
python3 tools/compare-contracts.py \
  --test testing/contracts/vm/demand_paging.c \
  --linux-kernel /path/to/linux-bzimage \
  --kevlar-kernel /path/to/kevlar.x64.img \
  --output results.json
```

**Behavior:**
1. Compile test binary (statically with musl)
2. Start two QEMU instances in parallel:
   - Linux: qemu-system-x86_64 -kernel linux-bzimage -initrd initramfs-linux.cpio
   - Kevlar: qemu-system-x86_64 -kernel kevlar.x64.img -initrd initramfs-kevlar.cpio
3. Copy test binary into each initramfs
4. Run test via init script (equivalent to `INIT_SCRIPT=/bin/test-contract`)
5. Capture stdout, stderr, exit code
6. Optionally diff filesystem state (/tmp, /root, etc.)
7. Compare results: if (linux_output == kevlar_output) → PASS, else DIVERGE

### Test Structure

Each test is a C binary that:
- Takes no arguments
- Writes output to stdout (human-readable)
- Returns exit code 0 for pass, non-zero for fail
- Uses deterministic output (no timestamps, PIDs in output, etc.)

Example test (contracts/vm/demand_paging.c):
```c
#include <stdio.h>
#include <unistd.h>
#include <sys/mman.h>

int main() {
    // Allocate 1MB
    char *page = mmap(NULL, 1024*1024, PROT_READ|PROT_WRITE,
                      MAP_PRIVATE|MAP_ANONYMOUS, -1, 0);

    // Write to trigger demand paging
    page[0] = 'A';

    // Fork and check if child sees the write
    pid_t pid = fork();
    if (pid == 0) {
        // Child
        printf("child_sees: %c\n", page[0]);
        exit(page[0] == 'A' ? 0 : 1);
    } else {
        // Parent
        printf("parent_continues\n");
        wait(NULL);
    }
    return 0;
}
```

Expected output (deterministic):
```
parent_continues
child_sees: A
```

If Kevlar's demand paging has different semantics, child might see '\0' instead of 'A' → divergence detected.

## Implementation

### 1. Test Harness Code

**File:** `tools/compare-contracts.py`

```python
#!/usr/bin/env python3
import subprocess
import json
import tempfile
import shutil
from pathlib import Path

def run_test_on_kernel(test_binary, kernel_image, initramfs_template):
    """Run test on a kernel instance, return (stdout, stderr, exit_code)"""
    with tempfile.TemporaryDirectory() as tmpdir:
        # Copy initramfs, inject test binary
        initramfs = Path(tmpdir) / "initramfs.cpio"
        shutil.copy(initramfs_template, initramfs)

        # Add test binary to initramfs
        # (use cpio tools or python cpio library)
        subprocess.run([
            "cpio", "-oa", "-F", str(initramfs),
            "-C", "newc"
        ], input=f"bin/{Path(test_binary).name}\n",
           cwd=Path(test_binary).parent)

        # Boot kernel
        proc = subprocess.run([
            "qemu-system-x86_64",
            "-kernel", kernel_image,
            "-initrd", str(initramfs),
            "-m", "512",
            "-nographic",
            "-serial", "stdio",
            "-append", f"init=/bin/{Path(test_binary).name}",
            "-timeout", "30"
        ], capture_output=True, text=True)

        return proc.stdout, proc.stderr, proc.returncode

def compare_tests(test_binary, linux_kernel, kevlar_kernel):
    """Run test on both kernels, compare results"""
    linux_out, linux_err, linux_code = run_test_on_kernel(
        test_binary, linux_kernel, "initramfs-linux.cpio")

    kevlar_out, kevlar_err, kevlar_code = run_test_on_kernel(
        test_binary, kevlar_kernel, "initramfs-kevlar.cpio")

    # Compare
    match = (linux_out == kevlar_out and
             linux_code == kevlar_code)

    return {
        "test": Path(test_binary).name,
        "status": "PASS" if match else "DIVERGE",
        "linux": {
            "stdout": linux_out,
            "exit_code": linux_code,
        },
        "kevlar": {
            "stdout": kevlar_out,
            "exit_code": kevlar_code,
        }
    }

if __name__ == "__main__":
    # Parse args, run comparisons, output JSON
    pass
```

### 2. Initramfs Template

Create minimal initramfs with:
- BusyBox (for basic shell, utilities)
- Test binaries
- /proc, /sys mounted
- tmpfs mounted

**File:** `testing/initramfs-contract-test.cpio`

```bash
# Create with:
mkdir -p tmpdir/{bin,etc,proc,sys,tmp}
cp /path/to/busybox tmpdir/bin/
ln -s busybox tmpdir/bin/sh
touch tmpdir/etc/passwd tmpdir/etc/group
cd tmpdir && find . | cpio -o -c > ../initramfs-contract-test.cpio && cd ..
```

### 3. Test Organization

```
testing/contracts/
├── vm/
│   ├── demand_paging.c
│   ├── page_cache.c
│   ├── fork_cow.c
│   └── mmap_semantics.c
├── scheduling/
│   ├── nice_values.c
│   ├── priority_inversion.c
│   └── deadline_scheduling.c
├── signals/
│   ├── delivery_order.c
│   ├── mask_semantics.c
│   └── coredump_layout.c
└── subsystems/
    ├── proc_layout.c
    ├── dev_permissions.c
    └── sys_hierarchy.c
```

Each is a standalone C program that can be compiled with:
```bash
musl-gcc -static -o testing/contracts/vm/demand_paging \
  testing/contracts/vm/demand_paging.c
```

### 4. Results Dashboard

Store results in `build/contract-results.json`:
```json
{
  "timestamp": "2026-03-13T10:30:00Z",
  "kevlar_commit": "abc123def456",
  "linux_version": "6.7",
  "results": [
    {
      "test": "demand_paging",
      "status": "PASS"
    },
    {
      "test": "page_cache",
      "status": "DIVERGE",
      "linux_output": "...",
      "kevlar_output": "..."
    }
  ],
  "summary": {
    "total": 42,
    "passed": 40,
    "diverged": 2
  }
}
```

## Build System Integration

Add to Makefile:

```makefile
.PHONY: build-contract-tests
build-contract-tests:
	find testing/contracts -name "*.c" | while read f; do \
		musl-gcc -static -O2 -o "$${f%.c}" "$$f"; \
	done

.PHONY: test-contracts
test-contracts: build-contract-tests
	python3 tools/compare-contracts.py \
		--linux-kernel /path/to/linux-bzimage \
		--kevlar-kernel build/kernel/x64/kernel.elf \
		--output build/contract-results.json
	# Print summary
	python3 -c "import json; d=json.load(open('build/contract-results.json')); \
		print(f\"Contracts: {d['summary']['passed']}/{d['summary']['total']} PASS\")"
```

## Success Criteria

- [ ] Harness compiles and runs
- [ ] Can boot both Linux and Kevlar instances in parallel
- [ ] Can inject test binaries into initramfs
- [ ] Can capture output from both kernels
- [ ] Can diff output and report divergences
- [ ] Results dashboard shows PASS/DIVERGE status
- [ ] Test runs complete in <5 minutes total (parallel boot)
- [ ] Harness integrates with CI/CD

## Known Challenges

1. **Initramfs management:** Injecting binaries into cpio archives is tricky. Consider using a helper library or shell scripts.

2. **Deterministic output:** Tests must output the same thing on both kernels. Avoid:
   - PID/TID/addresses in output
   - Timestamps
   - Randomized allocation addresses
   - /proc entries that vary (use `sysctl` to disable ASLR if needed)

3. **Timeout handling:** QEMU might hang on divergent behavior. Use `timeout 30` to avoid hanging the test suite.

4. **Linux version mismatch:** Different Linux versions might have slightly different behavior. Pin Linux version and document it.

## Integration Points

- **Makefile:** Add test-contracts target
- **CI/CD:** Run test-contracts in separate job, fail if divergences exceed threshold
- **Phases 2-6:** Build test binaries in contracts/ subdirectories, harness runs all of them

## Future Optimization

Phase 1 is the slow, careful approach. Later optimizations:
- Run multiple tests in parallel (different QEMU instances)
- Cache kernel boots (snapshot-restore)
- Use KVM for faster Linux boots
- Use VirtIO for faster file I/O
