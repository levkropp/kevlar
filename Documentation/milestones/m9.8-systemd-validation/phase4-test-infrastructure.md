# M9.8 Phase 4: Test Infrastructure

## Overview

Add Makefile targets that chain the 25-test synthetic init-sequence suite
(single + SMP) with a real systemd boot check, creating a single
`make test-systemd` command for comprehensive drop-in validation.

## 4.1 — test-systemd-v3-smp

**File:** `Makefile` (add after `test-systemd-v3`)

Same as `test-systemd-v3` but adds `-smp 4` and extends timeout to 180s.
Validates that all 25 systemd init-sequence operations work correctly under
multiprocessor scheduling.

- Log to `/tmp/kevlar-test-systemd-v3-smp-$(PROFILE).log`
- Exit 1 on `TEST_FAIL`
- Print "ALL SYSTEMD-V3 SMP TESTS PASSED" on `TEST_END`

## 4.2 — Upgrade test-m9

**File:** `Makefile` (replace existing `test-m9`)

Current `test-m9` has a 20s timeout — insufficient for real systemd boot under
KVM. Upgrade:

- Timeout: 20s → 90s
- Print PASS/FAIL per check (4 required checks)
- Add FAILED-unit count logging (informational, not a gate)
- Exit 1 if any required check fails
- Print "N/4 required checks passed" summary

## 4.3 — test-systemd Meta-Target

**File:** `Makefile` (add after `test-m9`)

Sequential chain of all three test stages:

```makefile
.PHONY: test-systemd
test-systemd:
    $(PROGRESS) "TEST" "M9.8: comprehensive systemd drop-in validation"
    @echo "Step 1/3: synthetic init-sequence (1 CPU)"
    $(MAKE) test-systemd-v3 PROFILE=$(PROFILE)
    @echo "Step 2/3: synthetic init-sequence SMP (4 CPUs)"
    $(MAKE) test-systemd-v3-smp PROFILE=$(PROFILE)
    @echo "Step 3/3: real systemd PID 1 boot"
    $(MAKE) test-m9 PROFILE=$(PROFILE)
    @echo "=== M9.8 test-systemd: ALL PASSED ==="
```

Stops on first failure via default make behavior.

## Verification

```bash
make RELEASE=1 test-systemd-v3         # 25/25, 1 CPU
make RELEASE=1 test-systemd-v3-smp     # 25/25, 4 CPUs
make RELEASE=1 test-m9                 # 4/4 checks
make RELEASE=1 test-systemd            # all three pass
```
