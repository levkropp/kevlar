// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
use arrayvec::{ArrayString, ArrayVec};

use crate::address::PAddr;

pub struct RamArea {
    pub base: PAddr,
    pub len: usize,
}

pub struct VirtioMmioDevice {
    pub mmio_base: PAddr,
    pub irq: u8,
}

pub struct AllowedPciDevice {
    pub bus: u8,
    pub slot: u8,
}

pub struct BootInfo {
    pub ram_areas: ArrayVec<RamArea, 8>,
    pub virtio_mmio_devices: ArrayVec<VirtioMmioDevice, 32>,
    /// QEMU `fw_cfg` MMIO base, discovered from the DTB on arm64 virt
    /// (typically `0x9020000`).  Populated only on platforms that
    /// expose fw_cfg; `None` on x64 and on arm64 boots without DTB.
    /// Used by `exts/ramfb` to set up scan-out of `/dev/fb0`'s backing
    /// memory through QEMU's `-device ramfb`.
    pub fw_cfg_base: Option<PAddr>,
    pub log_filter: ArrayString<64>,
    pub pci_enabled: bool,
    pub pci_allowlist: ArrayVec<AllowedPciDevice, 4>,
    pub use_second_serialport: bool,
    pub dhcp_enabled: bool,
    pub ip4: Option<ArrayString<18>>,
    pub gateway_ip4: Option<ArrayString<15>>,
    /// CPU identifiers parsed from firmware (MPIDRs on ARM64, empty on x86).
    pub cpu_mpdirs: ArrayVec<u64, 8>,
    /// Override init binary from kernel cmdline (`init=/path/to/binary`).
    /// When set, runs this binary directly as PID 1 instead of INIT_SCRIPT.
    pub init_path: Option<ArrayString<128>>,
    /// Debug filter from kernel cmdline (`debug=syscall,fault,...`).
    /// When set, overrides the compile-time KEVLAR_DEBUG env var.
    pub debug_filter: ArrayString<64>,
    /// Per-PID structured syscall trace from kernel cmdline (`strace-pid=N`).
    /// When set, every syscall made by that PID is emitted as a `DBG` JSONL
    /// line to serial — consumed by `tools/strace-diff.py` to compare
    /// Kevlar's syscall behaviour against Linux on an identical rootfs.
    pub strace_pid: Option<i32>,
    /// Per-comm structured syscall trace (`strace-comm=NAME`, max 15 chars).
    /// When set, every syscall made by any process whose `comm` matches NAME
    /// is logged.  Useful for tracing a target program when its PID isn't
    /// known at boot time (e.g. tracing pcmanfm spawned by an init script).
    pub strace_comm: Option<ArrayString<16>>,
    /// Trace `collect_ready` activity for this fd (and fd+1) — used to
    /// debug AF_UNIX listener starvation.  When set, every iteration
    /// over the fd in either the blocking or non-blocking epoll path
    /// logs the registered events, current poll status, and computed
    /// ready bits.
    pub epoll_trace_fd: Option<i32>,
    /// Full raw kernel cmdline (as seen at boot). Exposed via /proc/cmdline
    /// so userspace tools can inspect flags like `strace-exec=...`.
    pub raw_cmdline: ArrayString<512>,
}
