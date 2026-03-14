# Phase 4: Service Management

**Duration:** 3-5 days
**Prerequisite:** Phase 3 (systemd reaches basic.target)
**Goal:** systemd reaches multi-user.target, runs at least one service (getty), responds to systemctl.

## Getty Service

Custom unit file for Kevlar (skip agetty/login, run /bin/sh directly):

```ini
[Unit]
Description=Kevlar Console Shell

[Service]
Type=idle
ExecStart=/bin/sh
StandardInput=tty
StandardOutput=tty
StandardError=tty
Restart=always

[Install]
WantedBy=multi-user.target
```

## systemctl Communication

systemctl talks to systemd via `/run/systemd/private` (AF_UNIX SOCK_STREAM, native binary protocol — not D-Bus). If the socket works, `systemctl list-units` should work since systemctl is built from the same source as systemd.

## ps aux

BusyBox `ps` reads `/proc/[pid]/stat`, `/proc/[pid]/status`, `/proc/[pid]/cmdline` and iterates `/proc/` numeric entries. All already implemented.

## Shutdown

`reboot(2)` with `LINUX_REBOOT_CMD_POWER_OFF` triggers QEMU shutdown. systemd calls this after stopping all services. Verify clean shutdown sequence.

## Integration Test

End-to-end boot test:
1. Boot QEMU with systemd as PID 1
2. Wait for "Reached target Multi-User System" in serial output
3. Verify shell prompt appears (from getty service)
4. Run `echo hello` via automated serial input
5. Run `ps aux` and verify systemd + sh visible
6. Trigger `reboot` and verify clean shutdown

**Makefile target:** `test-m9`

## Success Criteria

- [ ] systemd reaches multi-user.target
- [ ] `systemctl list-units --type=service` shows running services
- [ ] Getty service starts, shell prompt appears
- [ ] `ps aux` shows systemd + services
- [ ] Clean shutdown via `reboot`
- [ ] `make test-m9` passes end-to-end

## M9 Duration Summary

| Phase | Duration | Cumulative |
|-------|----------|------------|
| 1: Syscall gaps | 3-4 days | 3-4 days |
| 2: Init sequence | 4-5 days | 7-9 days |
| 3: Real systemd | 5-7 days | 12-16 days |
| 4: Services | 3-5 days | 15-21 days |
| **Total** | | **~3 weeks** |

## Risk Assessment

- **Phase 1-2:** Low risk. Well-scoped kernel changes with clear tests.
- **Phase 3:** High risk. systemd has implicit dependencies. Fix cycle could take 2x estimate.
- **Phase 4:** Medium risk. D-Bus/private socket protocol is the biggest unknown.
- **Mitigation:** If Phase 3 stalls, the mini_systemd_v3 from Phase 2 already proves kernel readiness. Phase 1-2 work is valuable regardless.
