# M10.5 Phase 4: USB Host Controller

**Goal:** Load `xhci-hcd.ko` and `usb-storage.ko` from Linux 6.18. USB
flash drives and keyboards/mice work on real hardware.

---

## Target drivers

| Driver | Function | .ko files |
|--------|----------|-----------|
| xHCI | USB 3.x host controller | `xhci-hcd.ko` |
| EHCI | USB 2.0 host controller (legacy) | `ehci-hcd.ko`, `ehci-pci.ko` |
| USB Storage | USB mass storage (flash drives) | `usb-storage.ko` |
| USB HID | Keyboard, mouse (HID class) | `usbhid.ko`, `hid.ko`, `hid-generic.ko` |

xHCI is the priority — it handles USB 3.x (backward-compatible with 2.0/1.1).
Most systems since ~2013 have xHCI. EHCI is for older hardware.

---

## USB core

The USB subsystem has a layered architecture:

```
usb-storage.ko / usbhid.ko   (class drivers — what the device IS)
        │
        ▼
usbcore.ko  (USB core — enumeration, descriptors, URBs)
        │
        ▼
xhci-hcd.ko / ehci-hcd.ko   (host controller — hardware interface)
```

kcompat must implement `usbcore` as part of the shim (it's not a loadable
module in the traditional sense — it's linked into the kernel). In practice,
`usbcore` exports symbols that class drivers import.

---

## USB Request Blocks (URBs)

The core data structure for USB I/O is `struct urb`:

```c
struct urb {
    struct usb_device *dev;
    unsigned int pipe;          // endpoint | direction | type
    int status;                 // completion status
    unsigned int transfer_flags;
    void *transfer_buffer;
    dma_addr_t transfer_dma;
    u32 transfer_buffer_length;
    u32 actual_length;          // filled on completion
    usb_complete_t complete;    // completion callback
    void *context;
    // ... more fields
};
```

URBs are allocated, filled, submitted to the HCD, and completed asynchronously.

| Function | Implementation |
|----------|----------------|
| `usb_alloc_urb(iso_packets, mem_flags)` | Allocate URB |
| `usb_free_urb(urb)` | Free URB |
| `usb_fill_bulk_urb(...)` | Fill bulk transfer URB |
| `usb_fill_control_urb(...)` | Fill control transfer URB |
| `usb_fill_int_urb(...)` | Fill interrupt transfer URB |
| `usb_submit_urb(urb, mem_flags)` | Submit to HCD, returns immediately |
| `usb_kill_urb(urb)` | Cancel in-flight URB, wait for completion |
| `usb_unlink_urb(urb)` | Cancel without waiting |
| `usb_get_urb(urb)` | Increment refcount |

---

## USB device enumeration

When xhci-hcd detects a device (port status change), the USB core runs
device enumeration:

1. Reset port
2. GET_DESCRIPTOR (Device Descriptor) — 18 bytes
3. SET_ADDRESS
4. GET_DESCRIPTOR (Full Device + Config Descriptors)
5. Find matching class driver in driver table
6. Call `driver->probe(interface, id)`

The class driver (usb-storage, usbhid) then takes ownership.

---

## USB Storage class driver

`usb-storage.ko` wraps USB mass storage protocol over the USB bus:
- Uses Bulk-Only Transport (BOT) or USB Attached SCSI (UAS)
- Translates SCSI commands (READ_10, WRITE_10) to USB bulk transfers
- Exposes a SCSI disk to the block layer

Chain: `usb-storage.ko` → `sd.ko` (SCSI disk) → block layer

For simplicity, we can implement a minimal SCSI shim just for
READ_10/WRITE_10/INQUIRY/MODE_SENSE_6. UAS is simpler than BOT and
preferred on USB 3.x devices.

| Module | Dependency |
|--------|-----------|
| `usb-storage.ko` | `usbcore`, `scsi_mod` |
| `sd.ko` | `scsi_mod` |
| `scsi_mod.ko` | kernel SCSI core |

Three-module chain. `scsi_mod` has ~50K lines of code but most is error
handling and error recovery. The minimal subset for USB storage is tractable.

---

## USB HID (keyboard/mouse)

For M11 graphical, keyboard and mouse input is essential. The USB HID stack:

```
hid-generic.ko   (generic HID driver — handles most keyboards/mice)
        │
        ▼
usbhid.ko        (USB transport for HID)
        │
        ▼
hid.ko           (HID core: report descriptors, event parsing)
        │
        ▼
input.ko         (Linux input subsystem: /dev/input/event*)
```

The input subsystem creates `/dev/input/event*` nodes. Applications use
`libinput` (which reads these) for keyboard/mouse input. This is also
required for evdev in M11.

---

## xHCI HCD internals

xHCI (eXtensible Host Controller Interface) is an Intel-designed USB 3.x
controller specification:

- Register-based interface (MMIO)
- Supports streams, multiple transfer rings
- Doorbell registers to submit work
- Event ring for completion notification (MSI-X per interrupter)

The `xhci-hcd.ko` driver:
1. `pci_register_driver` → probe xHCI PCI device (class 0x0c0330)
2. `pci_iomap` BAR 0 → xHCI capability/operational/runtime registers
3. Allocate Device Context Base Address Array (DCBAA)
4. Initialize event ring, command ring, transfer rings
5. Enable MSI-X, register IRQ handler
6. Enable xHCI run bit, enumerate ports

This requires the full PCI + IRQ + DMA kcompat from Phase 2.

---

## QEMU USB emulation

QEMU's `xhci` device emulates a real xHCI controller:

```bash
qemu-system-x86_64 ... \
  -device qemu-xhci \
  -drive if=none,id=stick,file=/tmp/usb.img \
  -device usb-storage,bus=xhci.0,drive=stick
```

This lets us test USB storage without real hardware, validating the full
driver stack before moving to physical machines.

---

## Verification

### USB mass storage

```bash
insmod xhci-hcd.ko
insmod usb-storage.ko sd.ko
# USB flash drive should appear as /dev/sda
mount /dev/sda1 /mnt
ls /mnt
cp /etc/hostname /mnt/test.txt
umount /mnt
```

### USB HID (for M11)

```bash
insmod usbhid.ko hid.ko hid-generic.ko input.ko
# Plug in USB keyboard
# /dev/input/event0 should appear
cat /dev/input/event0 | hexdump  # see events on keypress
```

---

## Files to create/modify

- `kernel/kcompat/usb_core.rs` — USB core (URBs, device model, enumeration)
- `kernel/kcompat/usb_hid.rs` — HID event routing to Kevlar's input subsystem
- `kernel/kcompat/scsi_min.rs` — minimal SCSI shim for usb-storage
- `kernel/kcompat/input.rs` — Linux input subsystem (`/dev/input/event*`)
- `kernel/kcompat/symbols_6_18.rs` — add USB/HID/SCSI symbols
