// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! virtio-input driver — keyboard / mouse / tablet for QEMU `-machine
//! virt` arm64.
//!
//! QEMU virt has no PS/2 or USB by default; the natural input is
//! `virtio-{keyboard,mouse,tablet}-device` over virtio-mmio.  This
//! driver receives raw 8-byte virtio-input events from the device and
//! queues them as Linux 24-byte `struct input_event`s for
//! `/dev/input/eventN` in `kernel/fs/devfs/input.rs`.
//!
//! Spec: <https://docs.oasis-open.org/virtio/virtio/v1.1/csprd01/virtio-v1.1-csprd01.html#x1-2390008>
//!
//! Each device exposes two virtqueues:
//!   - `eventq` (idx 0): device → guest events.  We pre-fill it with
//!     8-byte buffers; the device writes one event per buffer.
//!   - `statusq` (idx 1): guest → device (LED state etc.).  Unused for now.
//!
//! Wire format (8 bytes per event):
//!   u16 type   — EV_KEY=0x01, EV_REL=0x02, EV_ABS=0x03, EV_SYN=0x00, ...
//!   u16 code   — KEY_*, BTN_*, REL_X/REL_Y, ABS_X/ABS_Y, SYN_REPORT, ...
//!   u32 value  — key down/up, axis delta, abs position, ...
#![no_std]

extern crate alloc;

#[macro_use]
extern crate kevlar_api;

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;

use virtio::device::{Virtio, VirtqDescBuffer};
use virtio::transports::{virtio_mmio::VirtioMmio, VirtioAttachError, VirtioTransport};

use kevlar_api::address::{PAddr, VAddr};
use kevlar_api::driver::{
    attach_irq, register_driver_prober, DeviceProber, VirtioMmioDevice,
};
use kevlar_api::mm::{alloc_pages, AllocPageFlags};
use kevlar_api::sync::SpinLock;

const VIRTIO_ID_INPUT: u32 = 18;
const VIRTIO_INPUT_EVENTQ: u16 = 0;
const VIRTIO_INPUT_STATUSQ: u16 = 1;

const EVENT_SIZE: usize = 8;
const PAGE_SIZE: usize = 4096;

/// 8-byte virtio-input event as it comes off the wire.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct VirtioInputEvent {
    pub ty: u16,
    pub code: u16,
    pub value: u32,
}

// Linux input_event types we care about.
pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
pub const EV_ABS: u16 = 0x03;

// virtio-input config-space layout (spec §5.8.5).
// `select` and `subsel` are at offsets 0 and 1 in device config; the
// device populates `size` (offset 2) and the data union (offset 8) in
// response.  We use this to query each device's true capability
// bitmaps so /dev/input/eventN's EVIOCGBIT response is honest.
const VIRTIO_INPUT_CFG_ID_NAME: u8 = 0x01;
const VIRTIO_INPUT_CFG_EV_BITS: u8 = 0x11;
const VIRTIO_INPUT_CFG_OFF_SELECT: u16 = 0;
const VIRTIO_INPUT_CFG_OFF_SUBSEL: u16 = 1;
const VIRTIO_INPUT_CFG_OFF_SIZE: u16 = 2;
const VIRTIO_INPUT_CFG_OFF_DATA: u16 = 8;
const VIRTIO_INPUT_BITMAP_MAX: usize = 128;

/// Read the bitmap for a given (select, subsel) pair from the device's
/// config space.  Returns the raw bytes (up to 128) the device reports.
fn read_config_bitmap(virtio: &Virtio, select: u8, subsel: u8) -> alloc::vec::Vec<u8> {
    virtio.write_device_config8(VIRTIO_INPUT_CFG_OFF_SELECT, select);
    virtio.write_device_config8(VIRTIO_INPUT_CFG_OFF_SUBSEL, subsel);
    let size = virtio.read_device_config8(VIRTIO_INPUT_CFG_OFF_SIZE) as usize;
    let n = core::cmp::min(size, VIRTIO_INPUT_BITMAP_MAX);
    let mut out = alloc::vec::Vec::with_capacity(n);
    for i in 0..n {
        out.push(virtio.read_device_config8(VIRTIO_INPUT_CFG_OFF_DATA + i as u16));
    }
    out
}

/// Read the device's name (NUL-terminated) from config.  Used for
/// EVIOCGNAME and for picking a sane in-kernel label.
fn read_config_name(virtio: &Virtio) -> alloc::string::String {
    virtio.write_device_config8(VIRTIO_INPUT_CFG_OFF_SELECT, VIRTIO_INPUT_CFG_ID_NAME);
    virtio.write_device_config8(VIRTIO_INPUT_CFG_OFF_SUBSEL, 0);
    let size = virtio.read_device_config8(VIRTIO_INPUT_CFG_OFF_SIZE) as usize;
    let n = core::cmp::min(size, VIRTIO_INPUT_BITMAP_MAX);
    let mut bytes = alloc::vec::Vec::with_capacity(n);
    for i in 0..n {
        let b = virtio.read_device_config8(VIRTIO_INPUT_CFG_OFF_DATA + i as u16);
        if b == 0 { break; }
        bytes.push(b);
    }
    alloc::string::String::from_utf8(bytes).unwrap_or_else(|_| alloc::string::String::from("virtio-input"))
}

/// Maximum number of input events buffered between the device IRQ and
/// userspace `read()`.  Mouse + keyboard at typical rates produce <100
/// events/sec; 256 gives ~2.5s of buffering before drops.
const MAX_QUEUED_EVENTS: usize = 256;

/// Global event sink — populated by the IRQ handler, drained by the
/// `/dev/input/eventN` device file's `read`.  One queue per registered
/// device (mouse, keyboard, tablet, ...).
static INPUT_DEVICES: SpinLock<Vec<Arc<InputDevice>>> = SpinLock::new(Vec::new());

/// One physical/virtual input device.  Each `InputDevice` corresponds
/// to one virtio-input MMIO device and one `/dev/input/eventN` node.
pub struct InputDevice {
    /// Human-readable kind, e.g. "QEMU Virtio Keyboard".  Read from the
    /// device's config space at probe time, primarily for diagnostics
    /// and for `EVIOCGNAME` ioctl from userspace evdev clients.
    pub name: SpinLock<alloc::string::String>,
    /// Per-event-type capability bitmaps from the device's virtio
    /// config space.  Indexed by event type (0..=31): EV_SYN, EV_KEY,
    /// EV_REL, EV_ABS, etc.  An empty Vec means the device doesn't
    /// support that event type.  Used to satisfy EVIOCGBIT honestly,
    /// so Xorg's xf86-input-evdev disambiguates keyboard from mouse.
    pub ev_bits: [SpinLock<alloc::vec::Vec<u8>>; 32],
    /// Pending events ready for userspace `read`.
    queue: SpinLock<VecDeque<VirtioInputEvent>>,
    /// Used by `/dev/input/eventN`'s `poll()`.  Bumped (with Release)
    /// inside the IRQ handler when an event is enqueued, so the
    /// epoll-LT path observes the change.
    poll_gen: core::sync::atomic::AtomicU64,
    /// EPOLLET watcher count so the epoll cache invalidation gating in
    /// `kernel/fs/epoll.rs::poll_cached` works the same way it does for
    /// pipes / Unix sockets.
    et_watcher_count: core::sync::atomic::AtomicU32,
}

impl InputDevice {
    pub fn new(name: alloc::string::String) -> Arc<Self> {
        // Const-init each per-event-type bitmap as an empty Vec.  The
        // probe path fills the relevant slots from virtio config space.
        Arc::new(InputDevice {
            name: SpinLock::new(name),
            ev_bits: [
                SpinLock::new(alloc::vec::Vec::new()), SpinLock::new(alloc::vec::Vec::new()),
                SpinLock::new(alloc::vec::Vec::new()), SpinLock::new(alloc::vec::Vec::new()),
                SpinLock::new(alloc::vec::Vec::new()), SpinLock::new(alloc::vec::Vec::new()),
                SpinLock::new(alloc::vec::Vec::new()), SpinLock::new(alloc::vec::Vec::new()),
                SpinLock::new(alloc::vec::Vec::new()), SpinLock::new(alloc::vec::Vec::new()),
                SpinLock::new(alloc::vec::Vec::new()), SpinLock::new(alloc::vec::Vec::new()),
                SpinLock::new(alloc::vec::Vec::new()), SpinLock::new(alloc::vec::Vec::new()),
                SpinLock::new(alloc::vec::Vec::new()), SpinLock::new(alloc::vec::Vec::new()),
                SpinLock::new(alloc::vec::Vec::new()), SpinLock::new(alloc::vec::Vec::new()),
                SpinLock::new(alloc::vec::Vec::new()), SpinLock::new(alloc::vec::Vec::new()),
                SpinLock::new(alloc::vec::Vec::new()), SpinLock::new(alloc::vec::Vec::new()),
                SpinLock::new(alloc::vec::Vec::new()), SpinLock::new(alloc::vec::Vec::new()),
                SpinLock::new(alloc::vec::Vec::new()), SpinLock::new(alloc::vec::Vec::new()),
                SpinLock::new(alloc::vec::Vec::new()), SpinLock::new(alloc::vec::Vec::new()),
                SpinLock::new(alloc::vec::Vec::new()), SpinLock::new(alloc::vec::Vec::new()),
                SpinLock::new(alloc::vec::Vec::new()), SpinLock::new(alloc::vec::Vec::new()),
            ],
            queue: SpinLock::new(VecDeque::with_capacity(MAX_QUEUED_EVENTS)),
            poll_gen: core::sync::atomic::AtomicU64::new(1),
            et_watcher_count: core::sync::atomic::AtomicU32::new(0),
        })
    }

    /// Pop up to `n` events into `out`.  Returns the count actually
    /// drained.  Called from the `/dev/input/eventN::read` path.
    pub fn drain(&self, out: &mut Vec<VirtioInputEvent>, n: usize) -> usize {
        let mut q = self.queue.lock();
        let mut taken = 0;
        while taken < n {
            match q.pop_front() {
                Some(e) => {
                    out.push(e);
                    taken += 1;
                }
                None => break,
            }
        }
        taken
    }

    pub fn has_pending(&self) -> bool {
        !self.queue.lock().is_empty()
    }

    pub fn poll_gen(&self) -> u64 {
        if self.et_watcher_count.load(core::sync::atomic::Ordering::Relaxed) > 0 {
            self.poll_gen.load(core::sync::atomic::Ordering::Acquire)
        } else {
            0
        }
    }

    pub fn notify_epoll_et(&self, added: bool) {
        if added {
            self.et_watcher_count
                .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        } else {
            self.et_watcher_count
                .fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
        }
    }

    fn push(&self, ev: VirtioInputEvent) {
        let mut q = self.queue.lock();
        if q.len() >= MAX_QUEUED_EVENTS {
            // Drop the oldest event; preferring fresh input over old.
            q.pop_front();
        }
        q.push_back(ev);
        // Bump the generation so epoll watchers see an edge.
        if self.et_watcher_count.load(core::sync::atomic::Ordering::Relaxed) > 0 {
            self.poll_gen
                .fetch_add(1, core::sync::atomic::Ordering::Release);
        }
    }
}

/// List of all registered input devices.  `kernel/fs/devfs/input.rs`
/// uses this to build `/dev/input/eventN` files.
pub fn registered_devices() -> alloc::vec::Vec<Arc<InputDevice>> {
    INPUT_DEVICES.lock().clone()
}

/// Driver-private state for a single virtio-input device.  Owns the
/// virtqueue + buffer-pool and the IRQ refill loop.
struct VirtioInputDriver {
    virtio: SpinLock<Virtio>,
    /// Base of the buffer pool; one 8-byte slot per descriptor.
    buf_pool: VAddr,
    num_descs: u16,
    /// User-facing queue.
    device: Arc<InputDevice>,
}

impl VirtioInputDriver {
    fn new(transport: Arc<dyn VirtioTransport>, name: alloc::string::String)
        -> Result<Self, VirtioAttachError>
    {
        let mut virtio = Virtio::new(transport);
        // Two virtqueues: eventq + statusq.  Negotiate VIRTIO_F_VERSION_1
        // (bit 32) so the device runs in modern (v1.0+) mode and uses
        // the modern queue layout we set up.  QEMU's
        // `-global virtio-mmio.force-legacy=false` requires the
        // driver to negotiate this bit; without it the device may
        // operate in transitional mode and refuse to deliver events.
        const VIRTIO_F_VERSION_1: u64 = 1 << 32;
        virtio.initialize(VIRTIO_F_VERSION_1, 2)?;

        let virtq_depth = virtio.virtq_mut(VIRTIO_INPUT_EVENTQ).num_descs();
        // Each buffer is 8 bytes (one virtio-input event).  Cap the
        // active descriptor count at what fits in our buffer pool to
        // avoid overflow.  Allocating 2 pages (8 KiB) covers up to
        // 1024 buffers — comfortable headroom.
        const POOL_PAGES: usize = 2;
        let buf_pool = alloc_pages(POOL_PAGES, AllocPageFlags::KERNEL)
            .expect("virtio-input: failed to allocate buffer pool")
            .as_vaddr();
        let max_buffers = (POOL_PAGES * PAGE_SIZE) / EVENT_SIZE;
        let num_descs = core::cmp::min(virtq_depth as usize, max_buffers) as u16;

        // Pre-fill the eventq with one 8-byte writable buffer per
        // descriptor.  Device fills, returns via used queue, IRQ
        // handler drains and re-submits.
        for i in 0..num_descs {
            let buf_paddr = buf_pool.add(i as usize * EVENT_SIZE).as_paddr();
            virtio.virtq_mut(VIRTIO_INPUT_EVENTQ).enqueue(&[
                VirtqDescBuffer::WritableFromDevice {
                    addr: buf_paddr,
                    len: EVENT_SIZE,
                },
            ]);
        }
        virtio.virtq_mut(VIRTIO_INPUT_EVENTQ).notify();

        // Replace the placeholder with the real device name from
        // config space ("QEMU Virtio Keyboard", "QEMU Virtio Mouse",
        // etc.) and read each event-type's capability bitmap so
        // EVIOCGBIT can report honestly.
        let real_name = read_config_name(&virtio);
        let display_name = if real_name.is_empty() { name } else { real_name };
        let device = InputDevice::new(display_name);
        for ev_type in 0u8..32 {
            let bits = read_config_bitmap(&virtio, VIRTIO_INPUT_CFG_EV_BITS, ev_type);
            if !bits.is_empty() {
                *device.ev_bits[ev_type as usize].lock() = bits;
            }
        }
        INPUT_DEVICES.lock().push(device.clone());

        Ok(VirtioInputDriver {
            virtio: SpinLock::new(virtio),
            buf_pool,
            num_descs,
            device,
        })
    }

    fn handle_irq(&self) {
        let mut virtio = self.virtio.lock();
        // Read the ISR register to ack the IRQ — without this the
        // device will keep re-asserting it.
        let _ = virtio.read_isr_status();

        let mut events_drained = 0u32;
        loop {
            let chain = match virtio.virtq_mut(VIRTIO_INPUT_EVENTQ).pop_used() {
                Some(c) => c,
                None => break,
            };
            events_drained += 1;
            // One descriptor per chain in our setup; each is 8 bytes.
            for desc in &chain.descs {
                if let VirtqDescBuffer::WritableFromDevice { addr, len } = desc {
                    if *len < EVENT_SIZE {
                        continue;
                    }
                    let v = addr.as_vaddr();
                    let ev = VirtioInputEvent {
                        ty: unsafe { v.read_volatile::<u16>() },
                        code: unsafe { v.add(2).read_volatile::<u16>() },
                        value: unsafe { v.add(4).read_volatile::<u32>() },
                    };
                    self.device.push(ev);

                    // Re-submit the same buffer for the next event.
                    virtio.virtq_mut(VIRTIO_INPUT_EVENTQ).enqueue(&[
                        VirtqDescBuffer::WritableFromDevice {
                            addr: *addr,
                            len: EVENT_SIZE,
                        },
                    ]);
                }
            }
        }
        virtio.virtq_mut(VIRTIO_INPUT_EVENTQ).notify();

        // One-shot trace at first event so we can confirm IRQ path is
        // live without spamming the log on every keystroke.
        static TRACED: core::sync::atomic::AtomicBool =
            core::sync::atomic::AtomicBool::new(false);
        if events_drained > 0 && !TRACED.swap(true, core::sync::atomic::Ordering::Relaxed) {
            let name = self.device.name.lock().clone();
            info!("virtio-input: first IRQ drained {} events from {}",
                  events_drained, name);
        }

        // Quiet the unused-field warnings.  Both fields are used at
        // probe time for buffer placement; keep them to make a future
        // shutdown / reset path simpler.
        let _ = (self.buf_pool, self.num_descs);
    }
}

struct VirtioInputProber;

impl DeviceProber for VirtioInputProber {
    #[cfg(target_arch = "x86_64")]
    fn probe_pci(&self, _: &kevlar_api::driver::pci::PciDevice) {
        // virtio-input over PCI exists on x86 but the arm64 desktop
        // path is the immediate use case; leave x86 PCI for later.
    }

    fn probe_virtio_mmio(&self, mmio_device: &VirtioMmioDevice) {
        let mmio = mmio_device.mmio_base.as_vaddr();
        let magic = unsafe { mmio.mmio_read32() };
        let virtio_version = unsafe { mmio.add(4).mmio_read32() };
        let device_id = unsafe { mmio.add(8).mmio_read32() };

        if magic != 0x74726976 || virtio_version != 2 || device_id != VIRTIO_ID_INPUT {
            return;
        }

        // Naming heuristic: QEMU exposes virtio-keyboard-device,
        // virtio-mouse-device, virtio-tablet-device — they all show up
        // as device_id=18.  Differentiate by reading the device's
        // config-space "name" string (selector=0x01).  See virtio spec
        // 5.8.5.  Skip for now — the in-kernel name is informational
        // and we just number them event0/event1/...
        let count = INPUT_DEVICES.lock().len();
        let name = alloc::format!("virtio-input{}", count);

        let transport = Arc::new(VirtioMmio::new(mmio_device.mmio_base));
        let driver = match VirtioInputDriver::new(transport, name.clone()) {
            Ok(d) => Arc::new(d),
            Err(e) => {
                warn!("virtio-input: attach failed: {:?}", e);
                return;
            }
        };

        let driver_for_irq = driver.clone();
        attach_irq(mmio_device.irq, move || {
            driver_for_irq.handle_irq();
        });

        info!(
            "virtio-input: registered {} (mmio={:#x}, irq={}, num_descs={})",
            name,
            mmio_device.mmio_base.value(),
            mmio_device.irq,
            driver.num_descs,
        );

        // Keep the driver alive: leak the Arc.  Devices are never
        // unregistered in Kevlar.
        let _ = Arc::into_raw(driver);
        let _: PAddr = mmio_device.mmio_base; // silence unused if cfg
    }
}

pub fn init() {
    info!("kext: Loading virtio_input...");
    register_driver_prober(Box::new(VirtioInputProber));
}
