# M11 Phase 2: Input Devices

**Goal:** Keyboard and mouse events reach userspace via `/dev/input/event*`.

## Approach: virtio-input or PS/2

**PS/2 keyboard/mouse** — QEMU emulates i8042 PS/2 controller by default.
The keyboard sends scan codes on IRQ 1, mouse on IRQ 12. This is the
simplest input path and works without virtio.

**virtio-input** — cleaner interface with virtio transport. Each device
is a separate virtio device with input events as virtio buffers. Better
for tablets/touchscreens. QEMU flag: `-device virtio-keyboard-pci`.

For Phase 2, PS/2 is the path of least resistance since QEMU already
emulates it and our kernel has IRQ handling infrastructure.

## Kernel Changes

### evdev subsystem

Linux's input subsystem uses `/dev/input/eventN` devices. Each device
produces `struct input_event` records:

```c
struct input_event {
    struct timeval time;   // 16 bytes
    uint16_t type;         // EV_KEY, EV_REL, EV_ABS, EV_SYN
    uint16_t code;         // KEY_A, REL_X, BTN_LEFT, etc.
    int32_t  value;        // 1=press, 0=release, rel offset
};
```

We need:
- `/dev/input/event0` (keyboard), `/dev/input/event1` (mouse)
- `read()` returns input_event structs (blocking until input)
- `ioctl(EVIOCGNAME)` — device name
- `ioctl(EVIOCGBIT)` — supported event types
- `poll()` — report POLLIN when events are queued

### PS/2 keyboard driver

Extend our existing serial keyboard handling to produce evdev events:
- i8042 port 0x60 read → scan code → map to KEY_* constant
- Queue as input_event, wake poll/epoll waiters

### PS/2 mouse driver

- i8042 aux port (IRQ 12)
- 3-byte packets: buttons, dx, dy
- Map to EV_REL + REL_X, REL_Y, BTN_LEFT/RIGHT/MIDDLE

## Verification

```
apk add evtest
evtest /dev/input/event0  # press keys, see events
```

Success: keyboard presses and mouse movements appear as evdev events.
