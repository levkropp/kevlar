# Kevlar integration tests

YAML-driven end-to-end tests for Kevlar's userspace stack
(LXDE / xterm / etc.).  Each test boots Kevlar in QEMU, runs
a sequence of steps (wait for serial output, inject mouse /
keyboard events via QMP, capture screenshots, assert pass /
fail conditions), and persists artifacts to
`build/itest/<test-name>/` for inspection.

Run a single test:

```
make ARCH=arm64 itest TEST=tests/integration/lxde-smoke.yaml
```

Run every test in this directory:

```
make ARCH=arm64 itest-all
```

The runner is `tools/itest.py`.

## YAML schema

```yaml
name: my-test                    # output dir name
description: |
  Free-form text.

arch: arm64                      # 'arm64' or 'x64'
disk: build/alpine-lxde.arm64.img
init: /bin/test-lxde             # initramfs init script
cmdline: "kevlar_interactive"    # kernel cmdline appended
boot_timeout: 60s
qemu_extra:                      # extra args after `--` to run-qemu.py
  - "-smp"
  - "2"
  - "-m"
  - "1024"
  - "-vga"
  - "std"
  - "-mem-prealloc"

steps:
  - <step_name>:
      <args>
```

## Step reference

### `wait_for_serial`

Block until a regex pattern appears on QEMU's serial output.

```yaml
- wait_for_serial:
    pattern: "DESKTOP_READY pcmanfm_pid=([0-9]+)"
    capture: pcmanfm_pid          # optional: bind group(1) to a variable
    timeout: 30s
```

### `inject_keys`

Inject keystrokes via QMP `input-send-event`.  Mirrors
`tools/run-qemu.py --inject-keys`.  Maps printable ASCII +
newline + tab + space; capitals via shift-modified press.

```yaml
- inject_keys:
    text: "hello world\n"
    delay_after: 500ms
```

### `inject_mouse`

Inject absolute mouse events (works with `virtio-tablet-device`).
Coordinates are pixel positions; the runner scales to the
0..32767 absolute axis range expected by virtio-input.

```yaml
- inject_mouse:
    action: move | click | double_click
    x: 80
    y: 80
    button: left | middle | right
    fb_w: 1024                   # optional override (default 1024)
    fb_h: 768                    # optional override (default 768)
    delay_after: 250ms
```

### `capture_state`

Take a screenshot via QEMU's QMP `screendump`.  Saves a PPM
and (where converters are available) a PNG to the test's
output directory.

```yaml
- capture_state:
    tag: pre-click               # filename prefix
    delay_before: 2s             # optional sleep before capture
```

### `emit_serial`

(v1: harness-local only — appends to the serial-buffer view
the harness uses for `wait_for_serial`.  Does NOT actually
send to the guest.  Will be wired to a real channel in a
future revision.)

```yaml
- emit_serial:
    text: "CAPTURE_DIAG"
```

### `extract_disk_artifacts`

Use `debugfs` to copy files out of the ext2 disk image into
`build/itest/<name>/disk/`.  Paths are flattened — slashes
become double-underscore.

```yaml
- extract_disk_artifacts:
    paths:
      - /var/log/Xorg.0.log
      - /var/log/lxde-session.log
      - /var/log/diag
```

### `assert`

Pass/fail check.  Failed asserts don't abort the test
(subsequent steps still run); they accumulate into the
final summary.

```yaml
- assert:
    type: framebuffer_painted
    tag: post-boot
    min_percent: 50
    describe: "Wallpaper should have painted by now."

- assert:
    type: framebuffer_changed
    between: [pre-click, post-click-2s]
    min_pixels_changed: 5000

- assert:
    type: framebuffer_unchanged
    between: [pre-click, post-click-2s]
    max_pixels_changed: 100      # tolerate cursor blink etc.

- assert:
    type: serial_contains
    pattern: "TEST_PASS xorg_running"

- assert:
    type: file_contains
    path: /var/log/Xorg.0.log    # extracted by extract_disk_artifacts first
    text: "Found absolute axes"
```

## Output

Each test produces `build/itest/<name>/`:

- `<tag>.ppm` / `<tag>.png` — screenshots
- `serial.log` — full QEMU stdout
- `disk/<flattened-path>` — extracted disk files
- `summary.json` — structured pass/fail per assert + step + variable

## Tips

- Folder/icon coordinates depend on the LXDE icon layout.  The
  desktop renders icons starting near (40, 40) on a 1024×768
  framebuffer — adjust per test.  Use a `move` action +
  `capture_state` first to verify the cursor lands on the
  intended target.
- When investigating a hang, sandwich the suspected action
  with `capture_state` calls (`pre-act`, `post-act-2s`,
  `post-act-10s`) and assert `framebuffer_changed` between
  them.  If the framebuffer doesn't change, the desktop is
  frozen.
- `extract_disk_artifacts` must run AFTER QEMU exits — the
  step is fine to place anywhere in `steps:` but extraction
  itself is deferred to the runner's teardown.

  **(v1 caveat: extraction runs synchronously when the step
  is encountered, which means it requires QEMU to have
  released the disk lock.  In practice, run it as the last
  step before `assert`s that depend on it.)**
