// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! PS/2 mouse driver for the i8042 auxiliary port.
//!
//! Initializes the PS/2 mouse via the i8042 controller, handles IRQ12
//! interrupts, and accumulates mouse packets into a ring buffer that
//! /dev/input/mice can read (ImPS/2 protocol: 3 bytes per packet).

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use x86::io::{inb, outb};

// ─── i8042 Controller Ports ─────────────────────────────────────────────────

const I8042_DATA: u16 = 0x60;
const I8042_STATUS: u16 = 0x64;
const I8042_CMD: u16 = 0x64;

// i8042 commands
const CMD_READ_CONFIG: u8 = 0x20;
const CMD_WRITE_CONFIG: u8 = 0x60;
const CMD_ENABLE_AUX: u8 = 0xA8;
const CMD_WRITE_AUX: u8 = 0xD4;

// Mouse commands (sent via CMD_WRITE_AUX)
const MOUSE_SET_DEFAULTS: u8 = 0xF6;
const MOUSE_ENABLE_DATA: u8 = 0xF4;
const MOUSE_SET_SAMPLE_RATE: u8 = 0xF3;

// ─── Mouse Packet Ring Buffer ───────────────────────────────────────────────

const RING_SIZE: usize = 512; // bytes (170 packets of 3 bytes)
static RING_BUF: [core::sync::atomic::AtomicU8; RING_SIZE] = {
    const ZERO: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
    [ZERO; RING_SIZE]
};
static RING_WRITE: AtomicUsize = AtomicUsize::new(0);
static RING_READ: AtomicUsize = AtomicUsize::new(0);
static MOUSE_INITIALIZED: AtomicBool = AtomicBool::new(false);

// Packet assembly state (3 bytes per ImPS/2 packet)
static PACKET_BYTE: AtomicUsize = AtomicUsize::new(0);
static PACKET_BUF: [core::sync::atomic::AtomicU8; 3] = {
    const ZERO: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
    [ZERO; 3]
};

/// Returns true if the mouse driver is initialized and receiving data.
pub fn is_initialized() -> bool {
    MOUSE_INITIALIZED.load(Ordering::Relaxed)
}

/// Number of bytes available to read from the mouse ring buffer.
pub fn available() -> usize {
    let w = RING_WRITE.load(Ordering::Acquire);
    let r = RING_READ.load(Ordering::Relaxed);
    w.wrapping_sub(r)
}

/// Read up to `buf.len()` bytes from the mouse packet buffer.
/// Returns the number of bytes actually read.
pub fn read(buf: &mut [u8]) -> usize {
    let mut count = 0;
    let w = RING_WRITE.load(Ordering::Acquire);
    let mut r = RING_READ.load(Ordering::Relaxed);
    while count < buf.len() && r != w {
        buf[count] = RING_BUF[r % RING_SIZE].load(Ordering::Relaxed);
        r = r.wrapping_add(1);
        count += 1;
    }
    RING_READ.store(r, Ordering::Release);
    count
}

// ─── i8042 Helpers ──────────────────────────────────────────────────────────

#[allow(unsafe_code)]
fn wait_input_ready() {
    // Wait for input buffer to be clear (bit 1 = 0)
    for _ in 0..10000 {
        if unsafe { inb(I8042_STATUS) } & 2 == 0 {
            return;
        }
    }
}

#[allow(unsafe_code)]
fn wait_output_ready() -> bool {
    // Wait for output buffer to have data (bit 0 = 1)
    for _ in 0..10000 {
        if unsafe { inb(I8042_STATUS) } & 1 != 0 {
            return true;
        }
    }
    false
}

#[allow(unsafe_code)]
fn send_command(cmd: u8) {
    wait_input_ready();
    unsafe { outb(I8042_CMD, cmd); }
}

#[allow(unsafe_code)]
fn send_data(data: u8) {
    wait_input_ready();
    unsafe { outb(I8042_DATA, data); }
}

#[allow(unsafe_code)]
fn read_data() -> u8 {
    if wait_output_ready() {
        unsafe { inb(I8042_DATA) }
    } else {
        0
    }
}

fn mouse_write(byte: u8) {
    send_command(CMD_WRITE_AUX);
    send_data(byte);
    // Read ACK (0xFA)
    let _ack = read_data();
}

// ─── IRQ Handler ────────────────────────────────────────────────────────────

/// Called from IRQ12 interrupt handler.
#[allow(unsafe_code)]
pub fn ps2_mouse_irq_handler() {
    let status = unsafe { inb(I8042_STATUS) };
    // Bit 0 = output buffer full, bit 5 = mouse data (not keyboard)
    if status & 0x21 != 0x21 {
        // Not mouse data — drain anyway
        let _ = unsafe { inb(I8042_DATA) };
        return;
    }

    let byte = unsafe { inb(I8042_DATA) };
    let idx = PACKET_BYTE.load(Ordering::Relaxed);

    // First byte of packet must have bit 3 set (always-1 bit in PS/2 protocol)
    if idx == 0 && byte & 0x08 == 0 {
        // Out of sync — skip until we see a valid first byte
        return;
    }

    PACKET_BUF[idx].store(byte, Ordering::Relaxed);

    if idx == 2 {
        // Complete 3-byte packet — push to ring buffer
        let w = RING_WRITE.load(Ordering::Relaxed);
        for i in 0..3 {
            RING_BUF[(w + i) % RING_SIZE].store(
                PACKET_BUF[i].load(Ordering::Relaxed),
                Ordering::Relaxed,
            );
        }
        RING_WRITE.store(w.wrapping_add(3), Ordering::Release);
        PACKET_BYTE.store(0, Ordering::Relaxed);

        // Wake any process blocked on /dev/input/mice read
        crate::handler().handle_mouse_event();
    } else {
        PACKET_BYTE.store(idx + 1, Ordering::Relaxed);
    }
}

// ─── Initialization ─────────────────────────────────────────────────────────

/// Initialize the PS/2 mouse via the i8042 controller.
#[allow(unsafe_code)]
pub fn init() {
    // Enable the auxiliary (mouse) port
    send_command(CMD_ENABLE_AUX);

    // Read controller config, enable IRQ12 (bit 1) and aux clock (clear bit 5)
    send_command(CMD_READ_CONFIG);
    let config = read_data();
    send_command(CMD_WRITE_CONFIG);
    send_data((config | 0x02) & !0x20); // Set bit 1 (aux IRQ), clear bit 5 (aux clock disable)

    // Reset mouse to defaults
    mouse_write(MOUSE_SET_DEFAULTS);

    // Set sample rate (for ImPS/2 detection sequence: 200, 100, 80)
    // This enables the scroll wheel if the mouse supports it,
    // but we only use 3-byte packets for now.
    mouse_write(MOUSE_SET_SAMPLE_RATE);
    mouse_write(200);
    mouse_write(MOUSE_SET_SAMPLE_RATE);
    mouse_write(100);
    mouse_write(MOUSE_SET_SAMPLE_RATE);
    mouse_write(80);

    // Enable data reporting
    mouse_write(MOUSE_ENABLE_DATA);

    MOUSE_INITIALIZED.store(true, Ordering::Release);
}
