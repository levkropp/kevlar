// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![no_std]
#![no_main]
#![cfg_attr(not(feature = "profile-ludicrous"), deny(unsafe_code))]
#![cfg_attr(feature = "profile-ludicrous", allow(unsafe_code))]
#![allow(unsafe_op_in_unsafe_fn)]
#![feature(custom_test_frameworks)]
#![feature(alloc_error_handler)]
#![test_runner(crate::test_runner::run_tests)]
#![reexport_test_harness_main = "test_main"]
#![feature(trait_alias)]

#[macro_use]
extern crate alloc;

#[macro_use]
extern crate log;

#[macro_use]
extern crate kevlar_platform;

#[macro_use]
mod logger;
#[macro_use]
mod result;
#[macro_use]
mod arch;
#[macro_use]
mod user_buffer;
mod ctypes;
mod debug;
mod deferred_job;
mod fs;
mod interrupt;
mod lang_items;
mod mm;
mod net;
mod pipe;
mod poll;
mod prelude;
mod process;
mod random;
mod cgroups;
mod namespace;
mod services;
mod syscalls;
mod test_runner;
mod timer;
mod tty;

use crate::{
    fs::{devfs::SERIAL_TTY, tmpfs},
    fs::{
        devfs::{self, DEV_FS},
        initramfs::{self, INITRAM_FS},
        mount::{MountTable, RootFs},
        path::Path,
        procfs::{self, PROC_FS},
        sysfs::{self, SYS_FS},
    },
    process::{switch, Process},
    syscalls::SyscallHandler,
};
use alloc::{boxed::Box, sync::Arc};
use core::sync::atomic::{AtomicBool, Ordering};
use interrupt::attach_irq;
use kevlar_api::kernel_ops::KernelOps;
use kevlar_platform::{
    arch::{idle, start_ap_preemption_timer, PageFaultReason, PtRegs},
    bootinfo::BootInfo,
    profile::StopWatch,
    spinlock::SpinLock,
};
use kevlar_utils::once::Once;
use net::register_ethernet_driver;
use tmpfs::TMP_FS;

/// Set to `true` by the BSP after `process::init()` completes.
/// APs spin on this in `ap_kernel_entry` before calling `process::init_ap()`,
/// ensuring INITIAL_ROOT_FS and the global scheduler are ready.
static KERNEL_READY: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
use crate::test_runner::end_tests;

struct Handler;

impl kevlar_platform::Handler for Handler {
    fn handle_console_rx(&self, ch: u8) {
        SERIAL_TTY.input_char(ch);
    }

    fn handle_irq(&self, irq: u8) {
        crate::interrupt::handle_irq(irq);
    }

    fn handle_timer_irq(&self) -> bool {
        // Run deferred jobs — this processes network packets queued by
        // the virtio-net IRQ handler. Safe because SpinLock::lock() disables
        // interrupts, so the timer can't fire while SCHEDULER is held.
        crate::deferred_job::run_deferred_jobs();

        crate::timer::handle_timer_irq()
    }

    fn handle_ap_preempt(&self) -> bool {
        // Process deferred jobs (network packet processing, etc.) on every
        // LAPIC preempt tick. Required because the PIT timer may stop firing
        // after the LAPIC timer takes over for preemption.
        crate::deferred_job::run_deferred_jobs();

        // Poll the network stack directly on every preempt tick. This handles
        // the case where virtio-net doesn't deliver an IRQ for incoming packets
        // (e.g., after SYN-ACK arrives in response to a connect()).
        crate::net::poll_if_ready();

        // Tick interval timers (setitimer/alarm → SIGALRM) on every
        // LAPIC preempt tick, not just PIC timer IRQ.  The PIC timer
        // may not fire with pci=off or on AP CPUs.
        crate::syscalls::setitimer::tick_real_timers();

        if !kevlar_platform::arch::in_preempt() {
            process::switch()
        } else {
            false
        }
    }

    fn current_process_signal_pending(&self) -> u32 {
        crate::process::current_process().signal_pending_bits()
    }

    fn handle_interrupt_return(&self, frame: *mut PtRegs) {
        #[allow(unsafe_code)]
        let frame_ref = unsafe { &mut *frame };
        let result = crate::process::Process::try_delivering_signal(frame_ref);
        if let Err(e) = result {
            trace!("handle_interrupt_return: signal delivery failed: {:?}", e);
        }
    }

    fn handle_page_fault(
        &self,
        unaligned_vaddr: Option<kevlar_platform::address::UserVAddr>,
        ip: usize,
        reason: PageFaultReason,
    ) {
        crate::mm::page_fault::handle_page_fault(unaligned_vaddr, ip, reason);
    }

    fn handle_user_fault(&self, exception: &str, ip: usize) {
        let pid = crate::process::current_process().pid().as_i32();
        crate::debug::emit(
            crate::debug::DebugFilter::FAULT,
            &crate::debug::DebugEvent::UserFault {
                pid,
                exception,
                ip,
                signal_delivered: crate::process::signal::SIGSEGV,
            },
        );
        warn!(
            "USER FAULT: {} pid={} ip={:#x}",
            exception, pid, ip
        );
        crate::process::Process::exit_by_signal(crate::process::signal::SIGSEGV);
    }

    fn handle_syscall(
        &self,
        a1: usize,
        a2: usize,
        a3: usize,
        a4: usize,
        a5: usize,
        a6: usize,
        n: usize,
        frame: *mut PtRegs,
    ) -> isize {
        #[allow(unsafe_code)]
        let frame_ref = unsafe { &mut *frame };
        let mut handler = SyscallHandler::new(frame_ref);
        handler
            .dispatch(a1, a2, a3, a4, a5, a6, n)
            .unwrap_or_else(|err| -(err.errno() as isize))
    }

    #[cfg(debug_assertions)]
    fn usercopy_hook(&self) {
        use crate::process::current_process;

        // We should not hold the vm lock since we'll try to acquire it in the
        // page fault handler when copying caused a page fault.
        debug_assert!(!current_process().vm().as_ref().unwrap().is_locked());
    }
}

struct ApiOps;

impl KernelOps for ApiOps {
    fn attach_irq(&self, irq: u8, f: alloc::boxed::Box<dyn FnMut() + Send + Sync + 'static>) {
        attach_irq(irq, f);
    }

    fn register_ethernet_driver(&self, driver: Box<dyn kevlar_api::driver::net::EthernetDriver>) {
        register_ethernet_driver(driver)
    }

    fn receive_etherframe_packet(&self, pkt: &[u8]) {
        net::receive_ethernet_frame(pkt);
    }
}

pub static INITIAL_ROOT_FS: Once<Arc<SpinLock<RootFs>>> = Once::new();

#[unsafe(no_mangle)]
#[allow(unsafe_code)]
#[cfg_attr(test, allow(unreachable_code))]
pub fn boot_kernel(#[cfg_attr(debug_assertions, allow(unused))] bootinfo: &BootInfo) -> ! {
    logger::init();

    // Re-enable auto-wrap (SeaBIOS sends ESC[?7l which disables it).
    // Without this, lines >80 chars corrupt terminal row tracking in
    // Konsole, xterm, and other VT100 emulators.
    kevlar_platform::print!("\x1b[?7h");
    info!("Booting Kevlar...");
    let mut profiler = StopWatch::start();

    kevlar_platform::set_handler(&Handler);

    // Initialize structured debug event system.
    // Cmdline `debug=...` takes precedence over compile-time KEVLAR_DEBUG.
    let debug_str = if !bootinfo.debug_filter.is_empty() {
        Some(bootinfo.debug_filter.as_str())
    } else {
        option_env!("KEVLAR_DEBUG")
    };
    debug::init(debug_str);

    // Initialize memory allocators first.
    interrupt::init();
    profiler.lap_time("global interrupt init");

    // Pre-fill zeroed page pools so first faults don't pay memset cost.
    kevlar_platform::page_allocator::prefill_huge_page_pool();
    kevlar_platform::page_allocator::prefill_prezeroed_pages();
    profiler.lap_time("prezeroed pool warmup");

    #[cfg(test)]
    {
        crate::test_main();
        end_tests();
    }

    // Initialize wall clock from CMOS RTC.
    crate::timer::init_wall_clock();
    profiler.lap_time("wall clock init");

    // Initialize kernel subsystems.
    pipe::init();
    profiler.lap_time("pipe init");
    poll::init();
    profiler.lap_time("poll init");
    procfs::init();
    profiler.lap_time("procfs init");
    devfs::init();
    profiler.lap_time("devfs init");
    sysfs::init();
    profiler.lap_time("sysfs init");
    cgroups::init();
    profiler.lap_time("cgroups init");
    namespace::init();
    profiler.lap_time("namespace init");
    tmpfs::init();
    profiler.lap_time("tmpfs init");
    initramfs::init();
    profiler.lap_time("initramfs init");
    kevlar_api::kernel_ops::init(&ApiOps);
    profiler.lap_time("kevlar_api init");

    // Load kernel extensions.
    info!("kext: Loading virtio_blk...");
    virtio_blk::init();
    profiler.lap_time("virtio_blk init");
    info!("kext: Loading virtio_net...");
    virtio_net::init();
    profiler.lap_time("virtio_net init");

    // Register Bochs VGA framebuffer prober (before PCI scan).
    bochs_fb::init();

    // Initialize device drivers (PCI bus scan invokes registered probers).
    kevlar_api::kernel_ops::init_drivers(
        bootinfo.pci_enabled,
        &bootinfo.pci_allowlist,
        &bootinfo.virtio_mmio_devices,
    );
    profiler.lap_time("drivers init");

    // Populate sysfs with device entries now that drivers are initialized.
    sysfs::populate();
    profiler.lap_time("sysfs populate");

    // Connect to the network.
    net::init_and_start_dhcp_discover(bootinfo);
    services::register_network_stack(Arc::new(net::SmoltcpNetworkStack));
    profiler.lap_time("net init");

    // Prepare the root file system.
    let mut root_fs = RootFs::new(INITRAM_FS.clone()).unwrap();
    let proc_dir = root_fs
        .lookup_dir(Path::new("/proc"))
        .expect("failed to locate /proc");
    let dev_dir = root_fs
        .lookup_dir(Path::new("/dev"))
        .expect("failed to locate /dev");
    let tmp_dir = root_fs
        .lookup_dir(Path::new("/tmp"))
        .expect("failed to locate /tmp");
    let sys_dir = root_fs
        .lookup_dir(Path::new("/sys"))
        .expect("failed to locate /sys");
    root_fs
        .mount(proc_dir, PROC_FS.clone())
        .expect("failed to mount procfs");
    root_fs
        .mount(dev_dir, DEV_FS.clone())
        .expect("failed to mount devfs");
    root_fs
        .mount(tmp_dir, TMP_FS.clone())
        .expect("failed to mount tmpfs");
    root_fs
        .mount(sys_dir, SYS_FS.clone())
        .expect("failed to mount sysfs");

    // Mount cgroup2 at /sys/fs/cgroup (systemd needs this to detect unified cgroups).
    // Create the directory hierarchy /sys/fs/cgroup/ under sysfs.
    {
        use crate::fs::file_system::FileSystem;
        let sys_root = SYS_FS.as_ref().root_dir().expect("sysfs root");
        let fs_dir = sys_root
            .create_dir("fs", kevlar_vfs::stat::FileMode::new(0o755), kevlar_vfs::stat::UId::new(0), kevlar_vfs::stat::GId::new(0))
            .or_else(|e| {
                // If already exists, look it up instead.
                if e.errno() == kevlar_vfs::result::Errno::EEXIST {
                    match sys_root.lookup("fs")? {
                        kevlar_vfs::inode::INode::Directory(d) => return Ok(kevlar_vfs::inode::INode::Directory(d)),
                        _ => {}
                    }
                }
                Err(e)
            })
            .and_then(|inode| match inode {
                kevlar_vfs::inode::INode::Directory(d) => Ok(d),
                _ => Err(kevlar_vfs::result::Error::new(kevlar_vfs::result::Errno::ENOTDIR)),
            })
            .expect("failed to create /sys/fs");
        let cgroup_dir = fs_dir
            .create_dir("cgroup", kevlar_vfs::stat::FileMode::new(0o755), kevlar_vfs::stat::UId::new(0), kevlar_vfs::stat::GId::new(0))
            .or_else(|e| {
                if e.errno() == kevlar_vfs::result::Errno::EEXIST {
                    match fs_dir.lookup("cgroup")? {
                        kevlar_vfs::inode::INode::Directory(d) => return Ok(kevlar_vfs::inode::INode::Directory(d)),
                        _ => {}
                    }
                }
                Err(e)
            })
            .and_then(|inode| match inode {
                kevlar_vfs::inode::INode::Directory(d) => Ok(d),
                _ => Err(kevlar_vfs::result::Error::new(kevlar_vfs::result::Errno::ENOTDIR)),
            })
            .expect("failed to create /sys/fs/cgroup");
        root_fs
            .mount(cgroup_dir, cgroups::cgroupfs::CgroupFs::new_or_get())
            .expect("failed to mount cgroup2");
    }

    // Initialize mount table for /proc/mounts.
    MountTable::init();

    // Open /dev/console for the init process.
    let console = root_fs
        .lookup_path(Path::new("/dev/console"), true)
        .expect("failed to open /dev/console");

    // Open the init's executable.
    // Priority: patchable slot > cmdline `init=` > compile-time INIT_SCRIPT > /sbin/init
    //
    // KEVLAR_INIT_SLOT: a 128-byte NUL-padded buffer that compare-contracts.py
    // can find (via the "KEVLAR_INIT:" magic prefix) and overwrite with a
    // per-test init path, without rebuilding the kernel.  This is the primary
    // mechanism for ARM64 contract tests (where DTB cmdline is unavailable).
    #[used]
    #[unsafe(link_section = ".rodata")]
    static INIT_SLOT: [u8; 128] = {
        let mut buf = [0u8; 128];
        // Magic prefix "KEVLAR_INIT:" (12 bytes) — compare-contracts.py searches for this.
        buf[0] = b'K'; buf[1] = b'E'; buf[2] = b'V'; buf[3] = b'L';
        buf[4] = b'A'; buf[5] = b'R'; buf[6] = b'_'; buf[7] = b'I';
        buf[8] = b'N'; buf[9] = b'I'; buf[10] = b'T'; buf[11] = b':';
        buf
    };

    // Read the init slot using volatile reads to prevent the compiler from
    // constant-folding the all-zero initial value — compare-contracts.py
    // patches the ELF bytes after compilation.
    let mut init_slot_buf = [0u8; 116];
    let init_slot_len = {
        for i in 0..116 {
            init_slot_buf[i] = unsafe { core::ptr::read_volatile(&INIT_SLOT[12 + i]) };
        }
        init_slot_buf.iter().position(|&b| b == 0).unwrap_or(116)
    };
    let init_slot_path = if init_slot_len > 0 {
        core::str::from_utf8(&init_slot_buf[..init_slot_len]).ok()
    } else {
        None
    };
    let init_path_cmdline = bootinfo.init_path.as_deref().map(str::as_bytes);
    let argv0 = if let Some(path) = init_slot_path {
        path
    } else if let Some(path) = init_path_cmdline {
        core::str::from_utf8(path).unwrap_or("/sbin/init")
    } else if option_env!("INIT_SCRIPT").is_some() {
        "/bin/sh"
    } else {
        "/sbin/init"
    };
    let executable_path = root_fs
        .lookup_path(Path::new(argv0), true)
        .expect("failed to open the init executable");

    // We cannot initialize the process subsystem until INITIAL_ROOT_FS is initialized.
    INITIAL_ROOT_FS.init(|| Arc::new(SpinLock::new(root_fs)));

    profiler.lap_time("root fs init");

    process::init();
    // Signal to waiting APs that the kernel and scheduler are ready.
    KERNEL_READY.store(true, Ordering::Release);
    profiler.lap_time("process init");

    // Create the init process.
    if let Some(path) = init_slot_path {
        // Init slot (patched by compare-contracts.py): run binary directly as PID 1.
        info!("running init slot: {:?}", path);
        Process::new_init_process(
            INITIAL_ROOT_FS.clone(),
            executable_path,
            console,
            &[path.as_bytes()],
        )
        .expect("failed to execute init slot binary");
    } else if let Some(path) = bootinfo.init_path.as_deref() {
        // `init=` on kernel cmdline: run binary directly as PID 1 (no sh -c wrapper).
        info!("running cmdline init: {:?}", path);
        Process::new_init_process(
            INITIAL_ROOT_FS.clone(),
            executable_path,
            console,
            &[path.as_bytes()],
        )
        .expect("failed to execute cmdline init");
    } else if let Some(script) = option_env!("INIT_SCRIPT") {
        let argv = &[b"sh", b"-c", script.as_bytes()];
        info!("running init script: {:?}", script);
        Process::new_init_process(INITIAL_ROOT_FS.clone(), executable_path, console, argv)
            .expect("failed to execute the init script: ");
    } else {
        info!("running /sbin/init");
        Process::new_init_process(
            INITIAL_ROOT_FS.clone(),
            executable_path,
            console,
            &[b"/sbin/init"],
        )
        .expect("failed to execute /sbin/init");
    }

    profiler.lap_time("first process init");

    // We've done the kernel initialization. Switch into the init...
    switch();

    // We're now in the idle thread context.
    idle_thread();
}

/// Entry point for Application Processors after the platform trampoline
/// and per-CPU hardware setup are complete.  Waits for the BSP to finish
/// kernel initialization, then sets up the per-AP idle thread and enters
/// the scheduler.
#[unsafe(no_mangle)]
#[allow(unsafe_code)]
pub fn ap_kernel_entry() -> ! {
    // Spin until BSP has initialized the global scheduler and INITIAL_ROOT_FS.
    while !KERNEL_READY.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }

    // Create a per-CPU idle thread and set CURRENT.
    process::init_ap();

    // Start the LAPIC preemption timer now that CURRENT is valid.
    start_ap_preemption_timer();

    // Try to pick up runnable work immediately; fall back to idle loop.
    switch();
    idle_thread()
}

pub fn interval_work() {
    process::gc_exited_processes();
    // Refill the 4KB prezeroed page pool so page faults get instant
    // zeroed pages without inline memset.
    kevlar_platform::page_allocator::refill_prezeroed_pages();
}

fn idle_thread() -> ! {
    loop {
        interval_work();
        idle();
    }
}
