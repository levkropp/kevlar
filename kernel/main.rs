// SPDX-License-Identifier: MIT OR Apache-2.0 OR BSD-2-Clause
#![no_std]
#![no_main]
#![cfg_attr(not(feature = "profile-ludicrous"), deny(unsafe_code))]
#![cfg_attr(feature = "profile-ludicrous", allow(unsafe_code))]
#![allow(unsafe_op_in_unsafe_fn)]
#![feature(custom_test_frameworks)]
#![feature(alloc_error_handler)]
#![feature(c_variadic)]
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
mod kabi;
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
        // (Removed: per-tick COW-bug stack scanner that took vm.lock_no_irq()
        // inside the timer IRQ. Under heavy munmap/brk activity it would
        // deadlock — timer IRQ on CPU A spinning on vm lock held by CPU B's
        // mm syscall. The historical COW corruption it chased was fixed by
        // the blog-199 / blog-202 / task-#4 round of TLB and atomicity
        // fixes. Reintroduce behind a debug flag + try_lock if needed.)

        crate::deferred_job::run_deferred_jobs();

        crate::timer::handle_timer_irq()
    }

    fn handle_ap_preempt(&self) -> bool {
        // Run the FULL timer handler on AP preempt ticks.  The PIT timer
        // (which normally drives handle_timer_irq on the BSP) can be
        // disabled by userspace processes with iopl(3) — Xorg writes to
        // PIT ports 0x40-0x43 during VGA initialization, reprogramming
        // or stopping the PIT.  By running handle_timer_irq from the
        // LAPIC preempt vector, ALL CPUs process sleep timers, poll
        // waiters, and preemption — regardless of PIT state.
        crate::timer::handle_timer_irq()
    }

    fn current_process_signal_pending(&self) -> u64 {
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
        // BREAKPOINT (int3) delivers SIGTRAP per POSIX/Linux.
        // All other user faults deliver SIGSEGV.
        let signal = if exception == "BREAKPOINT" {
            crate::process::signal::SIGTRAP
        } else {
            crate::process::signal::SIGSEGV
        };
        crate::debug::emit(
            crate::debug::DebugFilter::FAULT,
            &crate::debug::DebugEvent::UserFault {
                pid,
                exception,
                ip,
                signal_delivered: signal,
            },
        );
        warn!(
            "USER FAULT: {} pid={} ip={:#x}",
            exception, pid, ip
        );
        // Task #25 diagnostic: if this is GENERAL_PROTECTION_FAULT or
        // INVALID_OPCODE at a user IP, the faulting instruction is
        // almost certainly in a text-segment mmap that got its bytes
        // stomped after fault-in.  Re-read the same offset from the
        // file-backed VMA and print the diff so we can tell whether
        // the corruption happened at mmap time or later.
        if exception == "GENERAL_PROTECTION_FAULT" || exception == "INVALID_OPCODE" {
            crate::mm::page_fault::verify_text_page_at_ip(ip);
        }
        crate::process::Process::exit_by_signal(signal);
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
        let _svc_span = crate::debug::tracer::span_guard(
            crate::debug::tracer::span::SVC_HANDLE);
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

    #[cfg(target_arch = "aarch64")]
    fn current_task_fp_state_ptr(&self) -> u64 {
        match crate::process::current_process_option() {
            Some(p) => p.arch().fp_state_ptr() as u64,
            None => 0,
        }
    }

    #[cfg(target_arch = "aarch64")]
    fn mark_current_task_fp_loaded(&self) {
        use core::sync::atomic::Ordering;
        if let Some(p) = crate::process::current_process_option() {
            p.arch().fp_loaded.store(true, Ordering::Release);
        }
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

    // Per-PID structured syscall trace: when `strace-pid=N` is on the
    // cmdline, every syscall PID N makes is emitted as a JSONL event
    // to serial. Consumed by `tools/strace-diff.py` to compare Kevlar
    // syscall behaviour against Linux on the same rootfs.
    if let Some(pid) = bootinfo.strace_pid {
        info!("strace: enabling structured trace for pid={}", pid);
        syscalls::set_strace_pid(pid);
    }
    // strace-comm=NAME — trace syscalls for any process whose comm
    // matches.  Useful when the target's PID isn't known at boot
    // (e.g. pcmanfm spawned by an init script after dbus / Xorg /
    // openbox / tint2).
    if let Some(ref comm) = bootinfo.strace_comm {
        info!("strace: enabling structured trace for comm={}", comm.as_str());
        syscalls::set_strace_comm(comm.as_bytes());
    }

    // Per-fd epoll activity trace: when `epoll-trace-fd=N` is on the
    // cmdline, every iteration of `collect_ready` that touches fd=N
    // (and fd=N+1) logs the registered events, current poll status,
    // and computed ready bits.  Used to debug listener starvation.
    if let Some(fd) = bootinfo.epoll_trace_fd {
        info!("epoll: enabling trace for fd={}", fd);
        fs::epoll::EPOLL_TRACE_FD.store(
            fd, core::sync::atomic::Ordering::Relaxed);
    }

    // Expose the real cmdline via /proc/cmdline so userspace tools can
    // read flags like `strace-exec=...`.
    fs::procfs::system::set_cmdline(bootinfo.raw_cmdline.as_str());

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
    virtio_input::init();
    profiler.lap_time("virtio_input init");

    // K1 demo — load /lib/modules/hello.ko via the kabi loader and
    // call its `my_init` function.  Validates the full module-load
    // pipeline: ELF parse → section layout → memory copy →
    // relocation → kernel-symbol resolution → entry call.
    //
    // K1's module is purely synchronous (no scheduler interaction),
    // so it's safe to run before process::init().  The K2 demo
    // (which does sleep + wake) loads later, after the scheduler.
    #[cfg(target_arch = "aarch64")]
    {
        info!("kabi: loading /lib/modules/hello.ko");
        match kabi::load_module("/lib/modules/hello.ko", "my_init") {
            Ok(m) => match m.call_init() {
                Some(rc) => {
                    info!("kabi: my_init returned {}", rc);
                }
                None => warn!("kabi: my_init not found in module"),
            },
            Err(e) => warn!("kabi: load_module failed: {:?}", e),
        }
        profiler.lap_time("kabi hello.ko load");
    }

    // Register Bochs VGA framebuffer prober (before PCI scan).
    bochs_fb::init();

    // ARM64 QEMU virt has no legacy PCI, so the bochs_fb PCI prober never
    // fires.  Without a framebuffer `/dev/fb0`'s ioctls return ENODEV and
    // Xorg's fbdev driver reports "No devices detected" and dies.
    //
    // Provision a RAM-backed framebuffer so `/dev/fb0` has a real backing.
    // The region isn't scanned out by QEMU (no display pipe on virt without
    // ramfb/virtio-gpu), so nothing appears in the QEMU window — but Xorg,
    // i3, xterm and i3status all run against it and the test harness can
    // dump the rendered pixels back for analysis.
    #[cfg(target_arch = "aarch64")]
    {
        use kevlar_api::mm::{alloc_pages, AllocPageFlags};
        const FB_W: u32 = 1024;
        const FB_H: u32 = 768;
        const FB_BPP: u32 = 32;
        // 1024*768*4 = 3 MiB = 768 4 KiB pages.
        let num_pages = ((FB_W * FB_H * (FB_BPP / 8)) as usize).div_ceil(4096);
        match alloc_pages(num_pages, AllocPageFlags::KERNEL | AllocPageFlags::DIRTY_OK) {
            Ok(paddr) => {
                bochs_fb::init_ram_backed(paddr, FB_W, FB_H, FB_BPP);
                // If QEMU exposes fw_cfg AND was started with `-device
                // ramfb`, hand the fb paddr+geometry to ramfb so the
                // QEMU display backend (SDL/cocoa/VNC) scans it out
                // and the user actually sees the rendered i3 desktop.
                if let Some(fw_cfg_base) = bootinfo.fw_cfg_base {
                    ramfb::init(fw_cfg_base, paddr, FB_W, FB_H);
                }
            }
            Err(_) => {
                warn!("bochs-fb: failed to allocate RAM-backed framebuffer ({} pages)", num_pages);
            }
        }
    }

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
    // Register the switch function for deferred rescheduling from preempt_enable().
    kevlar_platform::arch::set_resched_fn(process::switch);
    // Register BSP's APIC ID for NMI watchdog targeting.
    kevlar_platform::arch::register_cpu_apic_id(0);

    // Start the LAPIC timer on the BSP too.  The PIT (ports 0x40-0x43) can be
    // reprogrammed by userspace with iopl(3) — Xorg does this during VGA init.
    // The LAPIC timer is memory-mapped in kernel space, immune to iopl.
    // Both BSP and APs now process timers via the LAPIC preempt vector.
    start_ap_preemption_timer();
    // Signal to waiting APs that the kernel and scheduler are ready.
    KERNEL_READY.store(true, Ordering::Release);

    // Enable runtime lock dependency checker.
    kevlar_platform::lockdep::enable();

    // Enable interrupt state tracker (IF transition ring buffer).
    kevlar_platform::arch::if_trace_enable();

    // Preemption safety checker disabled for now — too many false positives
    // from current_process() being called with IF=1 in syscall return paths.
    // Re-enable after fixing the violations incrementally.
    // kevlar_platform::arch::enable_preempt_check();

    // Enable NMI watchdog after all CPUs are online and timers are running.
    // APs will have registered their APIC IDs by the time they call
    // start_ap_preemption_timer(), which happens before switch().
    kevlar_platform::arch::watchdog_enable();

    // Enable the assembly-level usercopy trace so PAGE_ZERO_MISS can dump
    // recent copy_to_user / copy_from_user events.  Task #17: identify
    // which kernel code path is writing stale data to freed paddrs via
    // stale user TLB entries.
    #[cfg(target_arch = "x86_64")]
    kevlar_platform::usercopy_trace::enable();
    profiler.lap_time("process init");

    // K2 — kABI runtime: spawns the workqueue worker kthread, then
    // loads /lib/modules/k2.ko which exercises kmalloc + wait_queue
    // + completion + work_struct end-to-end.  Must run after
    // process::init() so kthread spawn + sleep/wake work.
    #[cfg(target_arch = "aarch64")]
    {
        kabi::init();
        info!("kabi: loading /lib/modules/k2.ko");
        match kabi::load_module("/lib/modules/k2.ko", "init_module") {
            Ok(m) => match m.call_init() {
                Some(rc) => {
                    info!("kabi: k2 init_module returned {}", rc);
                }
                None => warn!("kabi: init_module not found in k2.ko"),
            },
            Err(e) => warn!("kabi: k2 load_module failed: {:?}", e),
        }
        profiler.lap_time("kabi k2.ko load");
    }

    // K3 — load /lib/modules/k3.ko, exercising the device-model
    // spine (platform_device + platform_driver + bind/probe).
    #[cfg(target_arch = "aarch64")]
    {
        info!("kabi: loading /lib/modules/k3.ko");
        match kabi::load_module("/lib/modules/k3.ko", "init_module") {
            Ok(m) => match m.call_init() {
                Some(rc) => {
                    info!("kabi: k3 init_module returned {}", rc);
                }
                None => warn!("kabi: init_module not found in k3.ko"),
            },
            Err(e) => warn!("kabi: k3 load_module failed: {:?}", e),
        }
        profiler.lap_time("kabi k3.ko load");
    }

    // K4 — load /lib/modules/k4.ko (file_operations + char-device
    // bridge).  Then read /dev/k4-demo through the registered
    // adapter to verify end-to-end open/read/release dispatch.
    #[cfg(target_arch = "aarch64")]
    {
        info!("kabi: loading /lib/modules/k4.ko");
        match kabi::load_module("/lib/modules/k4.ko", "init_module") {
            Ok(m) => match m.call_init() {
                Some(rc) => {
                    info!("kabi: k4 init_module returned {}", rc);
                }
                None => warn!("kabi: init_module not found in k4.ko"),
            },
            Err(e) => warn!("kabi: k4 load_module failed: {:?}", e),
        }
        let mut buf = [0u8; 32];
        match kabi::cdev::read_dev_for_test("k4-demo", &mut buf) {
            Ok(n) => {
                let s = core::str::from_utf8(&buf[..n]).unwrap_or("?");
                info!("kabi: k4 /dev/k4-demo read {} bytes: {:?}", n, s);
            }
            Err(e) => warn!("kabi: k4 /dev/k4-demo read failed: {:?}", e),
        }
        profiler.lap_time("kabi k4.ko load");
    }

    // K5 — load /lib/modules/k5.ko (ioremap + readl/writel +
    // dma_alloc_coherent).  The module verifies cross-pointer
    // visibility internally; success = init_module returns 0.
    #[cfg(target_arch = "aarch64")]
    {
        info!("kabi: loading /lib/modules/k5.ko");
        match kabi::load_module("/lib/modules/k5.ko", "init_module") {
            Ok(m) => match m.call_init() {
                Some(rc) => {
                    info!("kabi: k5 init_module returned {}", rc);
                }
                None => warn!("kabi: init_module not found in k5.ko"),
            },
            Err(e) => warn!("kabi: k5 load_module failed: {:?}", e),
        }
        profiler.lap_time("kabi k5.ko load");
    }

    // K6 — load /lib/modules/k6.ko (variadic printk format strings).
    #[cfg(target_arch = "aarch64")]
    {
        info!("kabi: loading /lib/modules/k6.ko");
        match kabi::load_module("/lib/modules/k6.ko", "init_module") {
            Ok(m) => match m.call_init() {
                Some(rc) => {
                    info!("kabi: k6 init_module returned {}", rc);
                }
                None => warn!("kabi: init_module not found in k6.ko"),
            },
            Err(e) => warn!("kabi: k6 load_module failed: {:?}", e),
        }
        profiler.lap_time("kabi k6.ko load");
    }

    // K7 — load /lib/modules/k7.ko: a Linux-source-shape hello-world
    // module compiled against testing/linux/ compat headers.
    #[cfg(target_arch = "aarch64")]
    {
        info!("kabi: loading /lib/modules/k7.ko");
        match kabi::load_module("/lib/modules/k7.ko", "init_module") {
            Ok(m) => match m.call_init() {
                Some(rc) => {
                    info!("kabi: k7 init_module returned {}", rc);
                }
                None => warn!("kabi: init_module not found in k7.ko"),
            },
            Err(e) => warn!("kabi: k7 load_module failed: {:?}", e),
        }
        profiler.lap_time("kabi k7.ko load");
    }

    // K8 — load /lib/modules/k8.ko: a Linux-source-shape hello-world
    // compiled against Ubuntu 26.04's prebuilt Linux 7.0 headers.
    // First module in the kABI arc that uses *Linux's actual UAPI*
    // headers (not Kevlar compat shims).
    #[cfg(target_arch = "aarch64")]
    {
        info!("kabi: loading /lib/modules/k8.ko");
        match kabi::load_module("/lib/modules/k8.ko", "init_module") {
            Ok(m) => match m.call_init() {
                Some(rc) => {
                    info!("kabi: k8 init_module returned {}", rc);
                }
                None => warn!("kabi: init_module not found in k8.ko"),
            },
            Err(e) => warn!("kabi: k8 load_module failed: {:?}", e),
        }
        profiler.lap_time("kabi k8.ko load");
    }

    // K9 — load /lib/modules/bman-test.ko: a real prebuilt Linux 7.0
    // module from Ubuntu 26.04's `linux-modules-7.0.0-14-generic.deb`.
    // Smallest viable target (0 undefined symbols, init_module just
    // returns 0).  First Canonical-built binary to run in Kevlar.
    #[cfg(target_arch = "aarch64")]
    {
        info!("kabi: loading /lib/modules/bman-test.ko (Ubuntu 26.04)");
        match kabi::load_module("/lib/modules/bman-test.ko", "init_module") {
            Ok(m) => match m.call_init() {
                Some(rc) => {
                    info!("kabi: bman-test init_module returned {}", rc);
                }
                None => warn!("kabi: init_module not found in bman-test.ko"),
            },
            Err(e) => warn!("kabi: bman-test load_module failed: {:?}", e),
        }
        profiler.lap_time("kabi bman-test.ko load");
    }

    // K10 — load /lib/modules/xor-neon.ko: arm64 NEON-accelerated XOR
    // template (used by RAID5/6 parity).  First Ubuntu binary that
    // depends on a Linux export we add (`cpu_have_feature`).
    // Establishes the LinuxKPI-style iterate-on-missing-symbols
    // pattern for the rest of the kABI ascent.
    #[cfg(target_arch = "aarch64")]
    {
        info!("kabi: loading /lib/modules/xor-neon.ko (Ubuntu 26.04)");
        match kabi::load_module("/lib/modules/xor-neon.ko", "init_module") {
            Ok(m) => match m.call_init() {
                Some(rc) => {
                    info!("kabi: xor-neon init_module returned {}", rc);
                }
                None => warn!("kabi: init_module not found in xor-neon.ko"),
            },
            Err(e) => warn!("kabi: xor-neon load_module failed: {:?}", e),
        }
        profiler.lap_time("kabi xor-neon.ko load");
    }

    // K11 — load /lib/modules/dummy.ko: Ubuntu's network dummy device.
    // 23 undef symbols across rtnl/netdev/ethtool/skb subsystems — the
    // first milestone with subsystem-shaped stub work.
    #[cfg(target_arch = "aarch64")]
    {
        info!("kabi: loading /lib/modules/dummy.ko (Ubuntu 26.04)");
        match kabi::load_module("/lib/modules/dummy.ko", "init_module") {
            Ok(m) => match m.call_init() {
                Some(rc) => {
                    info!("kabi: dummy init_module returned {}", rc);
                }
                None => warn!("kabi: init_module not found in dummy.ko"),
            },
            Err(e) => warn!("kabi: dummy load_module failed: {:?}", e),
        }
        profiler.lap_time("kabi dummy.ko load");
    }

    // K12 — load /lib/modules/virtio_input.ko: Ubuntu's virtio
    // keyboard/mouse driver.  30 undef symbols across input core +
    // virtio bus + infra (kmalloc renames, spinlocks, ubsan, etc.).
    // Probe doesn't fire (no virtio bus walking yet); init_module
    // just registers the driver and returns.
    #[cfg(target_arch = "aarch64")]
    {
        info!("kabi: loading /lib/modules/virtio_input.ko (Ubuntu 26.04)");
        match kabi::load_module("/lib/modules/virtio_input.ko", "init_module") {
            Ok(m) => match m.call_init() {
                Some(rc) => {
                    info!("kabi: virtio_input init_module returned {}", rc);
                }
                None => warn!("kabi: init_module not found in virtio_input.ko"),
            },
            Err(e) => warn!("kabi: virtio_input load_module failed: {:?}", e),
        }
        profiler.lap_time("kabi virtio_input.ko load");
    }

    // K13 — load /lib/modules/drm_buddy.ko: Ubuntu's DRM buddy-
    // allocator helper, the first DRM-stack module.  21 undefs
    // across slab (kmem_cache_*), rbtree (rb_*), list debug
    // (__list_*_or_report), drm_printf, __sw_hweight64, and the
    // already-stubbed kmalloc/sched/ubsan surface.  init_module
    // does almost nothing (publishes drm_buddy_init via
    // EXPORT_SYMBOL); the heavy code only runs when a real DRM
    // driver calls into it (K14+).
    #[cfg(target_arch = "aarch64")]
    {
        info!("kabi: loading /lib/modules/drm_buddy.ko (Ubuntu 26.04)");
        match kabi::load_module("/lib/modules/drm_buddy.ko", "init_module") {
            Ok(m) => match m.call_init() {
                Some(rc) => info!("kabi: drm_buddy init_module returned {}", rc),
                None => warn!("kabi: init_module not found in drm_buddy.ko"),
            },
            Err(e) => warn!("kabi: drm_buddy load_module failed: {:?}", e),
        }
        profiler.lap_time("kabi drm_buddy.ko load");
    }

    // K14 — load /lib/modules/drm_exec.ko: Ubuntu's DRM
    // transactional buffer-reservation helper.  11 undefs (9 net
    // new) across ww_mutex (4), dma_resv (1), drm_gem (1),
    // refcount (1), kvmalloc renames (2).  Pure library module —
    // no init_module — so we log "library module" on the None
    // arm.
    #[cfg(target_arch = "aarch64")]
    {
        info!("kabi: loading /lib/modules/drm_exec.ko (Ubuntu 26.04)");
        match kabi::load_module("/lib/modules/drm_exec.ko", "init_module") {
            Ok(m) => match m.call_init() {
                Some(rc) => info!("kabi: drm_exec init_module returned {}", rc),
                None => info!("kabi: drm_exec is a library module (no init_module)"),
            },
            Err(e) => warn!("kabi: drm_exec load_module failed: {:?}", e),
        }
        profiler.lap_time("kabi drm_exec.ko load");
    }

    // K15 — load /lib/modules/drm_ttm_helper.ko: Ubuntu's DRM
    // framebuffer-emulation helper.  47 undefs (40 net new) across
    // drm_fb_helper (11), drm_client (5), fb (5), fb raster (3),
    // ttm_bo (3), drm_format (2), kernel mutex (2), module
    // refcount (2), and assorted misc (drm dbg / printk warn /
    // dev_driver_string / vzalloc rename / memcpy / memcpy_toio).
    // Pure library module — no init_module.
    #[cfg(target_arch = "aarch64")]
    {
        info!("kabi: loading /lib/modules/drm_ttm_helper.ko (Ubuntu 26.04)");
        match kabi::load_module("/lib/modules/drm_ttm_helper.ko", "init_module") {
            Ok(m) => match m.call_init() {
                Some(rc) => info!("kabi: drm_ttm_helper init_module returned {}", rc),
                None => info!("kabi: drm_ttm_helper is a library module (no init_module)"),
            },
            Err(e) => warn!("kabi: drm_ttm_helper load_module failed: {:?}", e),
        }
        profiler.lap_time("kabi drm_ttm_helper.ko load");
    }

    // K16 — load /lib/modules/drm_dma_helper.ko: Ubuntu's DRM
    // helper layer for DMA-coherent GEM buffers.  79 undefs (32
    // net new) across DMA API (10), DRM GEM (9), DRM prime (3),
    // DRM atomic (2), drm_client extension (2), drm_format
    // extension (2), and mm helpers (4).  Pure library module —
    // no init_module.
    #[cfg(target_arch = "aarch64")]
    {
        info!("kabi: loading /lib/modules/drm_dma_helper.ko (Ubuntu 26.04)");
        match kabi::load_module("/lib/modules/drm_dma_helper.ko", "init_module") {
            Ok(m) => match m.call_init() {
                Some(rc) => info!("kabi: drm_dma_helper init_module returned {}", rc),
                None => info!("kabi: drm_dma_helper is a library module (no init_module)"),
            },
            Err(e) => warn!("kabi: drm_dma_helper load_module failed: {:?}", e),
        }
        profiler.lap_time("kabi drm_dma_helper.ko load");
    }

    // K17 — load /lib/modules/cirrus-qemu.ko: Ubuntu's KMS driver
    // for QEMU's emulated Cirrus VGA.  88 undefs (81 net new)
    // across drm core / lifecycle (12), drm_kms atomic + objects
    // (31), drm_gem shadow + shmem (11), PCI surface (6),
    // mmio tracepoints (8), drm helper extensions (9), and misc
    // (_dev_warn / noop_llseek / logic_outb).  init_module
    // registers a PCI driver and returns 0 (probe doesn't fire —
    // no PCI bus walking yet, K20+).
    #[cfg(target_arch = "aarch64")]
    {
        info!("kabi: loading /lib/modules/cirrus-qemu.ko (Ubuntu 26.04)");
        match kabi::load_module("/lib/modules/cirrus-qemu.ko", "init_module") {
            Ok(m) => match m.call_init() {
                Some(rc) => info!("kabi: cirrus init_module returned {}", rc),
                None => warn!("kabi: init_module not found in cirrus-qemu.ko"),
            },
            Err(e) => warn!("kabi: cirrus-qemu load_module failed: {:?}", e),
        }
        profiler.lap_time("kabi cirrus-qemu.ko load");
    }

    // K18 — load /lib/modules/bochs.ko: Ubuntu's KMS driver for
    // QEMU Bochs Display Adapter.  107 undefs (18 net new) across
    // EDID (5), drm core extensions (5), PCI/IO resources (4),
    // port I/O (3), drm error log (1).  Same shape as cirrus-qemu
    // — registers PCI driver via __pci_register_driver and
    // returns 0.  Probe doesn't fire (no PCI bus walking yet).
    #[cfg(target_arch = "aarch64")]
    {
        info!("kabi: loading /lib/modules/bochs.ko (Ubuntu 26.04)");
        match kabi::load_module("/lib/modules/bochs.ko", "init_module") {
            Ok(m) => match m.call_init() {
                Some(rc) => info!("kabi: bochs init_module returned {}", rc),
                None => warn!("kabi: init_module not found in bochs.ko"),
            },
            Err(e) => warn!("kabi: bochs load_module failed: {:?}", e),
        }
        profiler.lap_time("kabi bochs.ko load");
    }

    // K33 Phase 2 — attempt to load /lib/modules/erofs.ko.
    // Erofs is the proof-of-concept for K33's filesystem-via-kABI
    // playbook: a block-based read-only fs that exercises the
    // same kABI surface as ext4 minus jbd2.  271 undef symbols
    // total; the K33 scaffolding (block/filemap/fs_register/
    // jbd2_stubs/fs_stubs) resolves all of them.
    //
    // Currently disabled at boot because erofs's partial init
    // (it returns -ENOMEM at our null-returning kmem_cache stub)
    // leaves state that later wedges virtio_input probe.  Wire
    // back on once the slab/kmem_cache stubs return real handles.
    // Enable via the cmdline `kabi-load-erofs=1` for debug runs.
    #[cfg(target_arch = "aarch64")]
    if bootinfo.raw_cmdline.as_str().contains("kabi-fill-super=1") {
        // K34 Day 2 gate: only enabled when explicitly requested
        // via cmdline.  See kernel/kabi/fs_synth.rs ALLOW_FILL_SUPER
        // comment.
        unsafe { kabi::fs_synth::ALLOW_FILL_SUPER = true; }
        info!("kabi: ALLOW_FILL_SUPER set — erofs fill_super dispatch enabled");
    }
    #[cfg(target_arch = "aarch64")]
    if bootinfo.raw_cmdline.as_str().contains("kabi-load-erofs=1") {
        info!("kabi: loading /lib/modules/erofs.ko (Ubuntu 26.04, K33 Phase 2)");
        match kabi::load_module("/lib/modules/erofs.ko", "init_module") {
            Ok(m) => match m.call_init() {
                Some(rc) => {
                    info!("kabi: erofs init_module returned {}", rc);
                    info!(
                        "kabi: registered filesystems = {}",
                        kabi::fs_register::registered_count(),
                    );
                    // K33 Phase 3 routing probe — exercise the
                    // kabi_mount_filesystem path.  We expect
                    // Err(ENOSYS) at v1: lookup hits the registry,
                    // but the module ->mount op dispatch is Phase 3b.
                    // The interesting log lines are:
                    //   "kabi: kabi_mount_filesystem(erofs): registry hit, ..."
                    //   "kabi: KabiFileSystem(...).root_dir() — not yet implemented"
                    match kabi::fs_adapter::kabi_mount_filesystem(
                        "erofs", None, 0, core::ptr::null(),
                    ) {
                        Ok(_) => info!("kabi: erofs mount route Ok (unexpected at v1)"),
                        Err(e) => info!(
                            "kabi: erofs mount route returned {:?} (Phase 3 v1 expected)",
                            e,
                        ),
                    }
                }
                None => warn!("kabi: init_module not found in erofs.ko"),
            },
            Err(e) => warn!("kabi: erofs load_module failed: {:?}", e),
        }
        profiler.lap_time("kabi erofs.ko load");
    }

    // K28 — pre-allocate the DRM DUMB-buffer pool BEFORE
    // walk_and_probe fires drm_dev_register, so /dev/dri/cardN
    // is installed with a real `mmap_phys_base`.  Userspace
    // mmap'ing a DUMB buffer ends up at this region.
    #[cfg(target_arch = "aarch64")]
    {
        kabi::drm_dev::init_dumb_pool();
        profiler.lap_time("kabi DRM DUMB pool init");
    }

    // K19 — walk registered PCI drivers + fire probe on matching
    // fake devices.  First milestone where an Ubuntu kernel
    // module's callback runs inside Kevlar.
    #[cfg(target_arch = "aarch64")]
    {
        kabi::pci::walk_and_probe();
        profiler.lap_time("kabi PCI walk");
    }

    // K21 — verify DRM ioctl dispatch end-to-end via a kernel-side
    // smoke test that issues DRM_IOCTL_VERSION against the
    // dispatcher.  Confirms /dev/dri/card0's ioctl path returns
    // the expected name + version.
    #[cfg(target_arch = "aarch64")]
    {
        kabi::drm_dev::ioctl_smoke_test();
        profiler.lap_time("kabi DRM ioctl smoke");
    }

    // K23 — walk registered virtio drivers + fire probe on
    // matching fake devices.  Mirror of K19's PCI walker, but
    // for the virtio bus.  First milestone where virtio_input's
    // probe runs inside Kevlar.
    #[cfg(target_arch = "aarch64")]
    {
        kabi::virtio::walk_and_probe();
        profiler.lap_time("kabi virtio walk");
    }

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

    // Register this AP's APIC ID for NMI watchdog targeting.
    kevlar_platform::arch::register_cpu_apic_id(kevlar_platform::arch::cpu_id());

    // Start the LAPIC preemption timer now that CURRENT is valid.
    start_ap_preemption_timer();

    // Try to pick up runnable work immediately; fall back to idle loop.
    switch();
    idle_thread()
}

/// Counter for how many times interval_work has run on this CPU.
/// Used to limit diagnostic output to the first few calls.
static INTERVAL_WORK_COUNT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

pub fn interval_work() {
    let iw_count = INTERVAL_WORK_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // LAPIC-DIAG disabled to keep serial output clean for strace
    let _ = iw_count;

    process::gc_exited_processes();
    // Catch the XFCE stack-corruption bug at the moment of corruption:
    // walk every suspended task and verify its saved-by-do_switch_thread
    // RIP is a valid kernel pointer. A kernel stack zeroed by a stale
    // user-TLB write produces saved_rip=0/2 long before the switch_in
    // that crashes — this scanner catches it within ~10 ms of the write.
    if iw_count % 5 == 0 {
        process::scan_suspended_task_corruption();
        // Disabled: live-stack scanner fires on page-recycling residue
        // (harmless) and the warn!() output interleaves with structured
        // DBG strace events, corrupting them. Keep off unless debugging.
        // kevlar_platform::stack_cache::scan_live_stack_corruption();
    }
    // Refill the 4KB prezeroed page pool so page faults get instant
    // zeroed pages without inline memset.
    kevlar_platform::page_allocator::refill_prezeroed_pages();

    // Check kernel stack guard patterns for overflow (every 100 idles ≈ 1s).
    if iw_count % 100 == 0 {
        kevlar_platform::stack_cache::check_all_guards();
    }

    // Sweep the prezeroed pool for in-pool corruption.  Catches the
    // "PAGE_ZERO_MISS site=PREZEROED_POOL" event close to the wall-clock
    // moment a kernel writer corrupted a queued page (rather than waiting
    // for a user to pop and execute it).  Cheap-ish per sweep — 512 pages
    // × 4 KiB of volatile reads ~1 ms — but the pool lock is ALSO held
    // by every alloc_page(prezeroed=true) on every CPU, so frequent sweeps
    // contend hard with normal traffic and trip the SPIN_CONTENTION
    // watchdog (5M-spin threshold) under heavy fault-in bursts.  Every
    // 500 idles ≈ 5 s is plenty for catching corruption.
    if iw_count % 500 == 0 {
        kevlar_platform::page_allocator::sweep_prezeroed_pool_for_corruption();
    }
}

fn idle_thread() -> ! {
    loop {
        interval_work();
        idle();
    }
}
