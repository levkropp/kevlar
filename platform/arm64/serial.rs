// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
//! PL011 UART driver for QEMU virt machine.
//! Base address: 0x0900_0000, IRQ 33 (SPI 1).
use crate::{
    handler,
    print::{set_debug_printer, set_printer, Printer},
};

use super::KERNEL_BASE_ADDR;

/// PL011 physical base on QEMU virt.
const PL011_BASE_PHYS: usize = 0x0900_0000;
pub const UART_IRQ: u8 = 33; // SPI 1 = GIC IRQ 33

// PL011 register offsets.
const UARTDR: usize = 0x000;
const UARTFR: usize = 0x018;
const UARTIBRD: usize = 0x024;
const UARTFBRD: usize = 0x028;
const UARTLCR_H: usize = 0x02C;
const UARTCR: usize = 0x030;
const UARTIMSC: usize = 0x038;
const UARTICR: usize = 0x044;

// Flag register bits.
const FR_TXFF: u32 = 1 << 5; // TX FIFO full
const FR_RXFE: u32 = 1 << 4; // RX FIFO empty

fn uart_base() -> usize {
    // Before MMU: physical address. After MMU: virtual via straight map.
    // We always use the virtual address since serial_init is called after MMU.
    KERNEL_BASE_ADDR + PL011_BASE_PHYS
}

unsafe fn mmio_read(addr: usize) -> u32 {
    core::ptr::read_volatile(addr as *const u32)
}

unsafe fn mmio_write(addr: usize, val: u32) {
    core::ptr::write_volatile(addr as *mut u32, val);
}

struct Pl011;

impl Pl011 {
    fn putc(&self, ch: u8) {
        let base = uart_base();
        unsafe {
            // Wait until TX FIFO is not full.
            while mmio_read(base + UARTFR) & FR_TXFF != 0 {}
            mmio_write(base + UARTDR, ch as u32);
        }
    }

    fn getc(&self) -> Option<u8> {
        let base = uart_base();
        unsafe {
            if mmio_read(base + UARTFR) & FR_RXFE != 0 {
                return None;
            }
            Some((mmio_read(base + UARTDR) & 0xFF) as u8)
        }
    }

    fn print_char(&self, ch: u8) {
        if ch == b'\n' {
            self.putc(b'\r');
        }
        self.putc(ch);
    }
}

static PL011: Pl011 = Pl011;

struct Pl011Printer;

impl Printer for Pl011Printer {
    fn print_bytes(&self, s: &[u8]) {
        for ch in s {
            PL011.print_char(*ch);
        }
    }
}

pub fn uart_irq_handler() {
    while let Some(ch) = PL011.getc() {
        if ch == b'\r' {
            handler().handle_console_rx(b'\n');
        } else {
            handler().handle_console_rx(ch);
        }
    }
    // Clear all interrupt status.
    let base = uart_base();
    unsafe {
        mmio_write(base + UARTICR, 0x7FF);
    }
}

pub unsafe fn early_init() {
    let base = uart_base();

    // Disable UART.
    mmio_write(base + UARTCR, 0);

    // Set baud rate (115200 with 24MHz clock: IBRD=13, FBRD=1).
    mmio_write(base + UARTIBRD, 13);
    mmio_write(base + UARTFBRD, 1);

    // 8N1, enable FIFO.
    mmio_write(base + UARTLCR_H, (0b11 << 5) | (1 << 4));

    // Enable UART, TX, RX.
    mmio_write(base + UARTCR, (1 << 0) | (1 << 8) | (1 << 9));

    // Enable RX interrupt.
    mmio_write(base + UARTIMSC, 1 << 4);

    set_printer(&Pl011Printer);
    set_debug_printer(&Pl011Printer);

    PL011.print_char(b'\n');
}

pub fn init() {
    super::gic::enable_irq(UART_IRQ);
}
