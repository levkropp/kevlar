# Milestone 12: Kubuntu Desktop Integration

**Goal:** Run a modern desktop environment (Kubuntu 24.04) with graphical
interface, window manager, and user applications.

**Current state:** M12 provides systemd-based headless OS. M11 adds GPU support,
graphics stack, and desktop environment.

**Impact:** Kubuntu running on Kevlar demonstrates Kevlar is a true drop-in
replacement for the Linux kernel, usable for day-to-day computing.

## Scope

This is a massive effort. Breaking it down:

### M12a: Display Server (Wayland or X11)
- **GPU driver:** KMS (Kernel Mode Setting) for display output
- **Display server:** wayland-server or Xvfb for testing
- **Compositor:** weston or simple test compositor
- **Rendering:** Mesa (GPU-accelerated) or software rendering

### M12b: Desktop Environment (KDE Plasma)
- **kde-workspace:** Plasma shell, plasmoid system
- **KDE frameworks:** KDE's application frameworks library
- **Widget engine:** Qt 6

### M12c: Applications
- **File manager:** Dolphin
- **Web browser:** Firefox or Chromium
- **Terminal:** Konsole or Yakuake
- **Utilities:** Text editor, image viewer, etc.

## Challenges (Very Large)

1. **GPU driver:** Writing a GPU driver is a multi-year effort. Options:
   - Use QEMU's emulated GPU (virtio-gpu or vmvga)
   - Use software rendering (llvmpipe) — very slow
   - Partner with open-source GPU projects (Intel, AMD, NVIDIA)

2. **Qt/KDE:** KDE/Qt are massive codebases. Require full C++ standard library,
   X11/Wayland client libraries, accessibility frameworks, etc.

3. **System integration:** Plasma expects:
   - systemd-logind (session management)
   - PulseAudio or PipeWire (sound)
   - DBus (IPC)
   - PolicyKit (privilege escalation)
   - OpenSSL/certificates (secure communications)

4. **Input devices:** Keyboard, mouse, touchpad require udev, input subsystem,
   KMS/DRM event handling.

5. **Licensing & Testing:** Vast surface area for bugs and regressions.
   Need continuous integration, automated GUI testing, etc.

## Realistic Approach

Rather than implementing full GPU drivers (infeasible), use:
1. **QEMU's virtio-gpu:** Simple paravirtualized GPU
2. **Software rendering:** Mesa/llvmpipe for fast fallback
3. **Minimal WM:** Launch X11 with simple window manager (i3, Openbox) first
4. **Then KDE:** If simpler WMs work, Plasma is feasible

## Test Plan

**Phase 1:** X11 test environment
```bash
Xvfb :1 -screen 0 1024x768x24 &
openbox &
# Simple GUI program shows
```

**Phase 2:** Wayland test environment
```bash
weston --backend=rdp-backend &
weston-terminal &
```

**Phase 3:** Minimal window manager
```bash
startx
# i3 / Openbox window manager
i3 &
# Launch xterm or simple app
```

**Phase 4:** KDE Plasma
```bash
startplasma-x11
# Plasma shell with widgets
```

## Success Criteria

- [ ] X11 server (Xvfb or Xwayland) starts
- [ ] Simple window manager (i3 or Openbox) manages windows
- [ ] Mouse/keyboard input works
- [ ] GUI applications (xterm, xcalc) launch and respond
- [ ] Wayland compositor (weston) starts
- [ ] KDE Plasma Desktop boots to desktop
- [ ] Konsole terminal emulator launches
- [ ] File manager (Dolphin) works
- [ ] Web browser renders pages

## Reality Check

Implementing full GPU support + KDE Plasma is a **multi-year effort** for a
small team. It's not unreasonable to stop at M9 (systemd) and declare Kevlar
a successful OS kernel for headless servers, containers, and embedded systems.

If GPU support is desired:
- Partner with Mesa/Nouveau/AMD developers
- Use QEMU-provided GPU simulation (reality: very slow)
- Focus on modern, lighter desktop environments (Sway, GNOME, Xfce) rather than KDE
- Ship as a development kernel first; full desktop polish second

## Alternative Path: Embedded + Containers

Instead of full desktop, position Kevlar as:
- **Container runtime:** crun/runc on Kevlar, compatible with Docker/Kubernetes
- **Embedded OS:** Used in appliances, IoT devices, servers
- **Development kernel:** Where kernel researchers test new features
- **HPC kernel:** Optimized for scientific computing

This is lower-effort and still valuable.

## Success Metrics

If M12 is attempted:
- [ ] Kubuntu 24.04 boots to graphical login
- [ ] User login works
- [ ] Desktop environment fully functional
- [ ] Launcher menu works
- [ ] Applications launch and respond
- [ ] No obvious kernel crashes in dmesg
- [ ] Performance is acceptable (GPU not required for UI responsiveness)

## Post-M12 Vision

A fully functional Kevlar-based Kubuntu system would:
- Demonstrate kernel feature parity with Linux
- Prove Kevlar's security (Fortress profile + hardening)
- Show Kevlar's performance gains (syscall speed, memory efficiency)
- Enable side-by-side comparison: Kubuntu on Linux vs. Kubuntu on Kevlar
- Validate Kevlar for production use on heterogeneous workloads

This is the ultimate goal: drop-in replacement that's not just compatible,
but better.
