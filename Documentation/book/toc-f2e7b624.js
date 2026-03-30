// Populate the sidebar
//
// This is a script, and not included directly in the page, to control the total size of the book.
// The TOC contains an entry for each page, so if each page includes a copy of the TOC,
// the total size of the page becomes O(n**2).
class MDBookSidebarScrollbox extends HTMLElement {
    constructor() {
        super();
    }
    connectedCallback() {
        this.innerHTML = '<ol class="chapter"><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="introduction.html">Introduction</a></span></li><li class="chapter-item expanded "><li class="part-title">User Guide</li></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="contributing.html"><strong aria-hidden="true">1.</strong> Contributing</a></span></li><li class="chapter-item expanded "><li class="part-title">Architecture</li></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="architecture.html"><strong aria-hidden="true">2.</strong> Overview</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="architecture/ringkernel.html"><strong aria-hidden="true">3.</strong> Ringkernel</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="architecture/safety-profiles.html"><strong aria-hidden="true">4.</strong> Safety Profiles</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="architecture/memory.html"><strong aria-hidden="true">5.</strong> Memory Management</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="architecture/process.html"><strong aria-hidden="true">6.</strong> Process Model</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="architecture/signals.html"><strong aria-hidden="true">7.</strong> Signals</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="architecture/filesystems.html"><strong aria-hidden="true">8.</strong> Filesystems</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="architecture/networking.html"><strong aria-hidden="true">9.</strong> Networking</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="architecture/hal.html"><strong aria-hidden="true">10.</strong> Hardware Abstraction</a></span></li><li class="chapter-item expanded "><li class="part-title">Blog</li></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><span><strong aria-hidden="true">11.</strong> Ringkernel</span></span><ol class="section"><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/007-ringkernel-phase-1-platform-extraction.html"><strong aria-hidden="true">11.1.</strong> Phase 1: Extracting the Platform</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/008-ringkernel-phase-2-core-traits.html"><strong aria-hidden="true">11.2.</strong> Phase 2: Core Traits and Service Registry</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/009-ringkernel-phase-3-service-extraction.html"><strong aria-hidden="true">11.3.</strong> Phase 3: Extracting Services</a></span></li></ol><li class="chapter-item expanded "><span class="chapter-link-wrapper"><span><strong aria-hidden="true">12.</strong> Safety Profiles</span></span><ol class="section"><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/010-safety-profiles.html"><strong aria-hidden="true">12.1.</strong> Configurable Safety: Choose Your Own Tradeoff</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/011-safety-profiles-phase-3-4.html"><strong aria-hidden="true">12.2.</strong> Optimized Usercopy and Copy-Semantic Frames</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/012-catch-unwind-and-capabilities.html"><strong aria-hidden="true">12.3.</strong> Panic Containment and Capability Tokens</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/013-benchmarks-and-ci.html"><strong aria-hidden="true">12.4.</strong> Benchmarks, CI Matrix, and Smarter Tooling</a></span></li></ol><li class="chapter-item expanded "><span class="chapter-link-wrapper"><span><strong aria-hidden="true">13.</strong> Performance</span></span><ol class="section"><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/014-fixing-fork.html"><strong aria-hidden="true">13.1.</strong> Fixing Fork: Two Bugs, One Wild Pointer</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/015-debug-tooling-and-usercopy-fix.html"><strong aria-hidden="true">13.2.</strong> The 8-Byte Copy That Should Have Been 4</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/016-kvm-performance.html"><strong aria-hidden="true">13.3.</strong> From 13µs to 200ns: KVM Performance</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/017-beating-linux-syscall-performance.html"><strong aria-hidden="true">13.4.</strong> Beating Linux: Syscall Performance</a></span></li></ol><li class="chapter-item expanded "><span class="chapter-link-wrapper"><span><strong aria-hidden="true">14.</strong> M4: Epoll and Sockets</span></span><ol class="section"><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/018-m4-epoll.html"><strong aria-hidden="true">14.1.</strong> Epoll for systemd</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/019-event-fds.html"><strong aria-hidden="true">14.2.</strong> Event Source FDs</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/020-unix-sockets.html"><strong aria-hidden="true">14.3.</strong> Unix Domain Sockets</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/021-filesystem-mounting.html"><strong aria-hidden="true">14.4.</strong> Filesystem Mounting</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/022-process-capabilities.html"><strong aria-hidden="true">14.5.</strong> Process Capabilities</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/023-integration-testing.html"><strong aria-hidden="true">14.6.</strong> Integration Testing</a></span></li></ol><li class="chapter-item expanded "><span class="chapter-link-wrapper"><span><strong aria-hidden="true">15.</strong> M5: Storage</span></span><ol class="section"><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/024-file-metadata.html"><strong aria-hidden="true">15.1.</strong> File Metadata and Extended I/O</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/025-inotify.html"><strong aria-hidden="true">15.2.</strong> inotify File Change Notifications</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/026-proc-sys.html"><strong aria-hidden="true">15.3.</strong> /proc &amp; /sys Completeness</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/027-zero-copy-io.html"><strong aria-hidden="true">15.4.</strong> Zero-Copy I/O</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/028-virtio-block.html"><strong aria-hidden="true">15.5.</strong> VirtIO Block Driver</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/029-ext2.html"><strong aria-hidden="true">15.6.</strong> Read-Only ext2 Filesystem</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/030-m5-integration.html"><strong aria-hidden="true">15.7.</strong> Integration Testing</a></span></li></ol><li class="chapter-item expanded "><span class="chapter-link-wrapper"><span><strong aria-hidden="true">16.</strong> M6: SMP and Threading</span></span><ol class="section"><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/031-smp-boot.html"><strong aria-hidden="true">16.1.</strong> SMP Boot</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/032-smp-scheduler.html"><strong aria-hidden="true">16.2.</strong> SMP Scheduler</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/033-x86-boot-protocol-investigation.html"><strong aria-hidden="true">16.3.</strong> x86 Boot Protocol Investigation</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/034-smp-threading.html"><strong aria-hidden="true">16.4.</strong> Threading</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/035-smp-debug-tooling.html"><strong aria-hidden="true">16.5.</strong> SMP Debug Tooling</a></span></li></ol><li class="chapter-item expanded "><span class="chapter-link-wrapper"><span><strong aria-hidden="true">17.</strong> M6.5: Linux Contracts</span></span><ol class="section"><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/036-syscall-trace-diffing.html"><strong aria-hidden="true">17.1.</strong> Syscall Trace Diffing</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/037-scheduling-contracts.html"><strong aria-hidden="true">17.2.</strong> Scheduling Contracts</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/038-signal-contracts.html"><strong aria-hidden="true">17.3.</strong> Signal Contracts</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/039-subsystem-contracts.html"><strong aria-hidden="true">17.4.</strong> Subsystem Contracts</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/040-program-compatibility.html"><strong aria-hidden="true">17.5.</strong> Program Compatibility</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/041-m6.5-milestone-complete.html"><strong aria-hidden="true">17.6.</strong> Milestone 6.5 Complete</a></span></li></ol><li class="chapter-item expanded "><span class="chapter-link-wrapper"><span><strong aria-hidden="true">18.</strong> M6.6: Benchmarking</span></span><ol class="section"><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/042-m6.6-benchmarks.html"><strong aria-hidden="true">18.1.</strong> Syscall Performance Benchmarking</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/043-mmap-fault-investigation.html"><strong aria-hidden="true">18.2.</strong> The mmap_fault Investigation</a></span></li></ol><li class="chapter-item expanded "><span class="chapter-link-wrapper"><span><strong aria-hidden="true">19.</strong> M7: /proc and glibc</span></span><ol class="section"><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/044-m7-phase1-proc-enumeration.html"><strong aria-hidden="true">19.1.</strong> /proc PID Enumeration</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/045-m7-phase2-proc-global-files.html"><strong aria-hidden="true">19.2.</strong> Global /proc Files</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/046-m7-phase3-proc-pid-enrichment.html"><strong aria-hidden="true">19.3.</strong> Per-process /proc Enrichment</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/047-m7-phase4-proc-maps.html"><strong aria-hidden="true">19.4.</strong> /proc/[pid]/maps</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/048-m7-phase6-glibc-stubs.html"><strong aria-hidden="true">19.5.</strong> glibc Syscall Stubs</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/049-m7-phase7-futex-ops.html"><strong aria-hidden="true">19.6.</strong> Futex Operations</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/050-m7-phase8-glibc-integration.html"><strong aria-hidden="true">19.7.</strong> glibc Integration</a></span></li></ol><li class="chapter-item expanded "><span class="chapter-link-wrapper"><span><strong aria-hidden="true">20.</strong> M8: cgroups and Namespaces</span></span><ol class="section"><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/051-m8-phase1-cgroups-v2.html"><strong aria-hidden="true">20.1.</strong> cgroups v2 Unified Hierarchy</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/052-m8-phase2-namespaces.html"><strong aria-hidden="true">20.2.</strong> Namespaces: UTS, PID, Mount</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/053-m8-phase3-pivot-root.html"><strong aria-hidden="true">20.3.</strong> pivot_root and Filesystem Isolation</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/054-m8-phase4-integration.html"><strong aria-hidden="true">20.4.</strong> M8 Integration Testing</a></span></li></ol><li class="chapter-item expanded "><span class="chapter-link-wrapper"><span><strong aria-hidden="true">21.</strong> M9: Init System</span></span><ol class="section"><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/055-m9-phase1-syscall-gaps.html"><strong aria-hidden="true">21.1.</strong> Syscall Gap Closure</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/056-m9-phase2-init-sequence.html"><strong aria-hidden="true">21.2.</strong> Systemd-Compatible Init Sequence</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/057-m9-phase3.1-build-systemd.html"><strong aria-hidden="true">21.3.</strong> Building systemd</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/058-m9-phase3.2-systemd-boots.html"><strong aria-hidden="true">21.4.</strong> systemd Boots</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/059-m9-phase4-services.html"><strong aria-hidden="true">21.5.</strong> Service Management — M9 Complete</a></span></li></ol><li class="chapter-item expanded "><span class="chapter-link-wrapper"><span><strong aria-hidden="true">22.</strong> M9.5–M10: Alpine Boot</span></span><ol class="section"><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/060-fork-exit-optimization.html"><strong aria-hidden="true">22.1.</strong> Fork/Exit Optimization</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/061-m9.5-huge-pages-and-mmap-parity.html"><strong aria-hidden="true">22.2.</strong> Huge Pages and mmap Parity</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/062-m10-getty-login.html"><strong aria-hidden="true">22.3.</strong> Getty Login</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/063-m10-openrc-boot.html"><strong aria-hidden="true">22.4.</strong> OpenRC Boot</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/064-m10-ext4-and-networking.html"><strong aria-hidden="true">22.5.</strong> Networking and ext4</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/065-m10-boot-fixes.html"><strong aria-hidden="true">22.6.</strong> Boot Polish</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/066-m10-userspace-networking.html"><strong aria-hidden="true">22.7.</strong> Userspace Networking</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/067-ext2-read-write.html"><strong aria-hidden="true">22.8.</strong> ext2 Read-Write</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/068-crash-diagnostics.html"><strong aria-hidden="true">22.9.</strong> Crash Diagnostics</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/069-mount-key-collision.html"><strong aria-hidden="true">22.10.</strong> The Mount Key Collision</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/070-busybox-benchmarks.html"><strong aria-hidden="true">22.11.</strong> BusyBox Tests and Benchmarks</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/071-page-cache-and-exec-prefaulting.html"><strong aria-hidden="true">22.12.</strong> Page Cache and Exec Prefaulting</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/072-rdrand-and-exec-parity.html"><strong aria-hidden="true">22.13.</strong> RDRAND and Exec Parity</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/073-benchmark-regression-hunt.html"><strong aria-hidden="true">22.14.</strong> Benchmark Regression Hunt</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/074-huge-page-prefault-and-refcount-redesign.html"><strong aria-hidden="true">22.15.</strong> Huge Page Prefault &amp; Refcount Redesign</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/075-huge-page-assembly-fix.html"><strong aria-hidden="true">22.16.</strong> Huge Page Assembly Fix</a></span></li></ol><li class="chapter-item expanded "><span class="chapter-link-wrapper"><span><strong aria-hidden="true">23.</strong> Contract Testing &amp; ABI Parity</span></span><ol class="section"><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/076-contract-test-expansion.html"><strong aria-hidden="true">23.1.</strong> Contract Test Expansion — 31 to 86</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/077-epoll-pipe-hang-and-edge-triggered.html"><strong aria-hidden="true">23.2.</strong> Epoll Pipe Hang Fix</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/078-ownership-guided-lock-elision.html"><strong aria-hidden="true">23.3.</strong> Ownership-Guided Lock Elision</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/079-contract-test-80-percent-abi.html"><strong aria-hidden="true">23.4.</strong> 86 to 112 Tests — 80% ABI Coverage</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/080-systemd-drop-in-validation.html"><strong aria-hidden="true">23.5.</strong> Systemd Drop-In Validation</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/081-contract-divergence-and-mremap.html"><strong aria-hidden="true">23.6.</strong> SIGSEGV Delivery and mremap</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/082-openrc-boot-and-proc-self-exe.html"><strong aria-hidden="true">23.7.</strong> OpenRC Boot — Shebang Bug Fix</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/083-benchmark-regression-fixes.html"><strong aria-hidden="true">23.8.</strong> Benchmark Regression Fixes</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/084-ghost-fork-signal-masking.html"><strong aria-hidden="true">23.9.</strong> Ghost-Fork Signal Masking</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/085-m10-epolloneshot-nanosecond-timers-multiuser.html"><strong aria-hidden="true">23.10.</strong> EPOLLONESHOT and Multi-User</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/086-m99-vdso-acceleration-hotfd-fix.html"><strong aria-hidden="true">23.11.</strong> vDSO Acceleration</a></span></li></ol><li class="chapter-item expanded "><span class="chapter-link-wrapper"><span><strong aria-hidden="true">24.</strong> Alpine Bring-Up</span></span><ol class="section"><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/087-ktrace-wall-clock-apk-diagnosis.html"><strong aria-hidden="true">24.1.</strong> ktrace, Wall-Clock, APK Diagnosis</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/088-heap-vma-corruption-apk-network.html"><strong aria-hidden="true">24.2.</strong> Heap VMA Corruption</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/089-apk-update-seven-bugs-tcp-http.html"><strong aria-hidden="true">24.3.</strong> Nine Bugs to apk update</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/090-five-test-fixes-full-green.html"><strong aria-hidden="true">24.4.</strong> Five Fixes — Full Green</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/091-arm64-compilation-fix-twelve-stubs.html"><strong aria-hidden="true">24.5.</strong> ARM64 Back from the Dead</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/092-ktrace-arm64-semihosting-standalone-repo.html"><strong aria-hidden="true">24.6.</strong> ktrace Multi-Arch</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/093-arm64-contract-tests-zero-to-89.html"><strong aria-hidden="true">24.7.</strong> ARM64 Contract Tests</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/094-setsockopt-fix-socket-crash-discovery.html"><strong aria-hidden="true">24.8.</strong> SO_RCVBUF Fix</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/095-arm64-neon-signal-delivery-fixes.html"><strong aria-hidden="true">24.9.</strong> ARM64 NEON Signal Fix</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/096-vm-drop-cow-fix-exec-parity.html"><strong aria-hidden="true">24.10.</strong> Vm::Drop CoW Fix</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/097-vfs-path-resolution-overhaul.html"><strong aria-hidden="true">24.11.</strong> VFS Path Resolution Overhaul</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/098-stale-prefault-template-pipe-stack-overflow.html"><strong aria-hidden="true">24.12.</strong> Stale Prefault + Pipe Fix</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/099-unix-socket-fix-ext4-write-permissions.html"><strong aria-hidden="true">24.13.</strong> Unix Socket Fix + ext4 Writes</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/100-alpine-boot-ext4-verified.html"><strong aria-hidden="true">24.14.</strong> Alpine Boots on Kevlar</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/101-alpine-pipe-crash-fix-login-prompt.html"><strong aria-hidden="true">24.15.</strong> PIE Relocation Pre-Faulting</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/102-alpine-login-openrc-boot.html"><strong aria-hidden="true">24.16.</strong> Alpine Root Login</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/103-alpine-apk-install-packages.html"><strong aria-hidden="true">24.17.</strong> APK Installs Packages</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/104-contract-tests-151-pass-zero-xfail.html"><strong aria-hidden="true">24.18.</strong> 151 Contract Tests</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/105-apk-zero-errors-curl-works.html"><strong aria-hidden="true">24.19.</strong> APK Zero Errors — Curl Works</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/106-gcc-compiles-c-on-kevlar.html"><strong aria-hidden="true">24.20.</strong> GCC Compiles C</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/107-openrc-crash-fixed-brk-pie.html"><strong aria-hidden="true">24.21.</strong> OpenRC Crash — brk() Fix</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/108-gcc-compiles-links-runs-alpine.html"><strong aria-hidden="true">24.22.</strong> GCC Compiles and Links</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/109-hello-from-kevlar-gcc-full-pipeline.html"><strong aria-hidden="true">24.23.</strong> Hello from Kevlar</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/110-openrc-boots-clean-signal-frame-fix.html"><strong aria-hidden="true">24.24.</strong> OpenRC Boots Clean</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/111-buddy-allocator-bitmap-signal-nesting-apk.html"><strong aria-hidden="true">24.25.</strong> Buddy Allocator + Signal Nesting</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/112-ext4-mmap-writeback-openssl-sigsegv.html"><strong aria-hidden="true">24.26.</strong> ext4 mmap Writeback</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/113-ext4-performance-105x-faster-creates.html"><strong aria-hidden="true">24.27.</strong> ext4 Performance — 105x Faster</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/114-batch-virtio-blk-writes-26x-faster.html"><strong aria-hidden="true">24.28.</strong> Batch VirtIO — 26x Faster</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/115-159-contract-tests-sigaltstack-fix.html"><strong aria-hidden="true">24.29.</strong> 159/159 Contract Tests</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/116-openssl-tls-curl-https-alpine.html"><strong aria-hidden="true">24.30.</strong> OpenSSL TLS 1.3 HTTPS</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/117-openrc-invalid-opcode-investigation.html"><strong aria-hidden="true">24.31.</strong> OpenRC INVALID_OPCODE</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/118-openrc-crash-root-cause-dynamic-linker.html"><strong aria-hidden="true">24.32.</strong> Dynamic Linker Bug</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/119-openrc-fixed-vfork-signal-sharing.html"><strong aria-hidden="true">24.33.</strong> VFORK Signal Fix</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/120-mount-sharing-msync-cgroups-investigation.html"><strong aria-hidden="true">24.34.</strong> Mount Sharing + msync</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/121-https-python3-ext4-cache-fix.html"><strong aria-hidden="true">24.35.</strong> HTTPS + Python3 + ext4 Fix</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/122-dlopen-crash-investigation-huge-page-stale-pte.html"><strong aria-hidden="true">24.36.</strong> Python dlopen Investigation</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/123-python-dlopen-fixed-heap-mmap-overlap-cgroups.html"><strong aria-hidden="true">24.37.</strong> Python dlopen Fixed</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/124-https-cert-verification-works-61-tests-pass.html"><strong aria-hidden="true">24.38.</strong> HTTPS Cert Verification</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/125-utimes-flock-cgroups-fixes-66-tests.html"><strong aria-hidden="true">24.39.</strong> utimes, flock, cgroups</a></span></li></ol><li class="chapter-item expanded "><span class="chapter-link-wrapper"><span><strong aria-hidden="true">25.</strong> Phase 1–3: Drop-In Compatibility</span></span><ol class="section"><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/126-phase1-posix-gaps-sessions-fcntl-statx-rlimits.html"><strong aria-hidden="true">25.1.</strong> Phase 1: Core POSIX Gaps</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/127-phase2-socket-opts-ssh-benchmarks-syscall-bugfix.html"><strong aria-hidden="true">25.2.</strong> Phase 2: Socket Options + SSH + Syscall Bug</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/128-phase2-hardening-nginx-permissions-ipv6.html"><strong aria-hidden="true">25.3.</strong> Phase 2: nginx + Permissions + IPv6</a></span></li><li class="chapter-item expanded "><span class="chapter-link-wrapper"><a href="blog/129-phase3-build-tools-xattr-fdatasync-19-tests.html"><strong aria-hidden="true">25.4.</strong> Phase 3: Build Tools 19/19 PASS</a></span></li></ol></li></ol>';
        // Set the current, active page, and reveal it if it's hidden
        let current_page = document.location.href.toString().split('#')[0].split('?')[0];
        if (current_page.endsWith('/')) {
            current_page += 'index.html';
        }
        const links = Array.prototype.slice.call(this.querySelectorAll('a'));
        const l = links.length;
        for (let i = 0; i < l; ++i) {
            const link = links[i];
            const href = link.getAttribute('href');
            if (href && !href.startsWith('#') && !/^(?:[a-z+]+:)?\/\//.test(href)) {
                link.href = path_to_root + href;
            }
            // The 'index' page is supposed to alias the first chapter in the book.
            if (link.href === current_page
                || i === 0
                && path_to_root === ''
                && current_page.endsWith('/index.html')) {
                link.classList.add('active');
                let parent = link.parentElement;
                while (parent) {
                    if (parent.tagName === 'LI' && parent.classList.contains('chapter-item')) {
                        parent.classList.add('expanded');
                    }
                    parent = parent.parentElement;
                }
            }
        }
        // Track and set sidebar scroll position
        this.addEventListener('click', e => {
            if (e.target.tagName === 'A') {
                const clientRect = e.target.getBoundingClientRect();
                const sidebarRect = this.getBoundingClientRect();
                sessionStorage.setItem('sidebar-scroll-offset', clientRect.top - sidebarRect.top);
            }
        }, { passive: true });
        const sidebarScrollOffset = sessionStorage.getItem('sidebar-scroll-offset');
        sessionStorage.removeItem('sidebar-scroll-offset');
        if (sidebarScrollOffset !== null) {
            // preserve sidebar scroll position when navigating via links within sidebar
            const activeSection = this.querySelector('.active');
            if (activeSection) {
                const clientRect = activeSection.getBoundingClientRect();
                const sidebarRect = this.getBoundingClientRect();
                const currentOffset = clientRect.top - sidebarRect.top;
                this.scrollTop += currentOffset - parseFloat(sidebarScrollOffset);
            }
        } else {
            // scroll sidebar to current active section when navigating via
            // 'next/previous chapter' buttons
            const activeSection = document.querySelector('#mdbook-sidebar .active');
            if (activeSection) {
                activeSection.scrollIntoView({ block: 'center' });
            }
        }
        // Toggle buttons
        const sidebarAnchorToggles = document.querySelectorAll('.chapter-fold-toggle');
        function toggleSection(ev) {
            ev.currentTarget.parentElement.parentElement.classList.toggle('expanded');
        }
        Array.from(sidebarAnchorToggles).forEach(el => {
            el.addEventListener('click', toggleSection);
        });
    }
}
window.customElements.define('mdbook-sidebar-scrollbox', MDBookSidebarScrollbox);


// ---------------------------------------------------------------------------
// Support for dynamically adding headers to the sidebar.

(function() {
    // This is used to detect which direction the page has scrolled since the
    // last scroll event.
    let lastKnownScrollPosition = 0;
    // This is the threshold in px from the top of the screen where it will
    // consider a header the "current" header when scrolling down.
    const defaultDownThreshold = 150;
    // Same as defaultDownThreshold, except when scrolling up.
    const defaultUpThreshold = 300;
    // The threshold is a virtual horizontal line on the screen where it
    // considers the "current" header to be above the line. The threshold is
    // modified dynamically to handle headers that are near the bottom of the
    // screen, and to slightly offset the behavior when scrolling up vs down.
    let threshold = defaultDownThreshold;
    // This is used to disable updates while scrolling. This is needed when
    // clicking the header in the sidebar, which triggers a scroll event. It
    // is somewhat finicky to detect when the scroll has finished, so this
    // uses a relatively dumb system of disabling scroll updates for a short
    // time after the click.
    let disableScroll = false;
    // Array of header elements on the page.
    let headers;
    // Array of li elements that are initially collapsed headers in the sidebar.
    // I'm not sure why eslint seems to have a false positive here.
    // eslint-disable-next-line prefer-const
    let headerToggles = [];
    // This is a debugging tool for the threshold which you can enable in the console.
    let thresholdDebug = false;

    // Updates the threshold based on the scroll position.
    function updateThreshold() {
        const scrollTop = window.pageYOffset || document.documentElement.scrollTop;
        const windowHeight = window.innerHeight;
        const documentHeight = document.documentElement.scrollHeight;

        // The number of pixels below the viewport, at most documentHeight.
        // This is used to push the threshold down to the bottom of the page
        // as the user scrolls towards the bottom.
        const pixelsBelow = Math.max(0, documentHeight - (scrollTop + windowHeight));
        // The number of pixels above the viewport, at least defaultDownThreshold.
        // Similar to pixelsBelow, this is used to push the threshold back towards
        // the top when reaching the top of the page.
        const pixelsAbove = Math.max(0, defaultDownThreshold - scrollTop);
        // How much the threshold should be offset once it gets close to the
        // bottom of the page.
        const bottomAdd = Math.max(0, windowHeight - pixelsBelow - defaultDownThreshold);
        let adjustedBottomAdd = bottomAdd;

        // Adjusts bottomAdd for a small document. The calculation above
        // assumes the document is at least twice the windowheight in size. If
        // it is less than that, then bottomAdd needs to be shrunk
        // proportional to the difference in size.
        if (documentHeight < windowHeight * 2) {
            const maxPixelsBelow = documentHeight - windowHeight;
            const t = 1 - pixelsBelow / Math.max(1, maxPixelsBelow);
            const clamp = Math.max(0, Math.min(1, t));
            adjustedBottomAdd *= clamp;
        }

        let scrollingDown = true;
        if (scrollTop < lastKnownScrollPosition) {
            scrollingDown = false;
        }

        if (scrollingDown) {
            // When scrolling down, move the threshold up towards the default
            // downwards threshold position. If near the bottom of the page,
            // adjustedBottomAdd will offset the threshold towards the bottom
            // of the page.
            const amountScrolledDown = scrollTop - lastKnownScrollPosition;
            const adjustedDefault = defaultDownThreshold + adjustedBottomAdd;
            threshold = Math.max(adjustedDefault, threshold - amountScrolledDown);
        } else {
            // When scrolling up, move the threshold down towards the default
            // upwards threshold position. If near the bottom of the page,
            // quickly transition the threshold back up where it normally
            // belongs.
            const amountScrolledUp = lastKnownScrollPosition - scrollTop;
            const adjustedDefault = defaultUpThreshold - pixelsAbove
                + Math.max(0, adjustedBottomAdd - defaultDownThreshold);
            threshold = Math.min(adjustedDefault, threshold + amountScrolledUp);
        }

        if (documentHeight <= windowHeight) {
            threshold = 0;
        }

        if (thresholdDebug) {
            const id = 'mdbook-threshold-debug-data';
            let data = document.getElementById(id);
            if (data === null) {
                data = document.createElement('div');
                data.id = id;
                data.style.cssText = `
                    position: fixed;
                    top: 50px;
                    right: 10px;
                    background-color: 0xeeeeee;
                    z-index: 9999;
                    pointer-events: none;
                `;
                document.body.appendChild(data);
            }
            data.innerHTML = `
                <table>
                  <tr><td>documentHeight</td><td>${documentHeight.toFixed(1)}</td></tr>
                  <tr><td>windowHeight</td><td>${windowHeight.toFixed(1)}</td></tr>
                  <tr><td>scrollTop</td><td>${scrollTop.toFixed(1)}</td></tr>
                  <tr><td>pixelsAbove</td><td>${pixelsAbove.toFixed(1)}</td></tr>
                  <tr><td>pixelsBelow</td><td>${pixelsBelow.toFixed(1)}</td></tr>
                  <tr><td>bottomAdd</td><td>${bottomAdd.toFixed(1)}</td></tr>
                  <tr><td>adjustedBottomAdd</td><td>${adjustedBottomAdd.toFixed(1)}</td></tr>
                  <tr><td>scrollingDown</td><td>${scrollingDown}</td></tr>
                  <tr><td>threshold</td><td>${threshold.toFixed(1)}</td></tr>
                </table>
            `;
            drawDebugLine();
        }

        lastKnownScrollPosition = scrollTop;
    }

    function drawDebugLine() {
        if (!document.body) {
            return;
        }
        const id = 'mdbook-threshold-debug-line';
        const existingLine = document.getElementById(id);
        if (existingLine) {
            existingLine.remove();
        }
        const line = document.createElement('div');
        line.id = id;
        line.style.cssText = `
            position: fixed;
            top: ${threshold}px;
            left: 0;
            width: 100vw;
            height: 2px;
            background-color: red;
            z-index: 9999;
            pointer-events: none;
        `;
        document.body.appendChild(line);
    }

    function mdbookEnableThresholdDebug() {
        thresholdDebug = true;
        updateThreshold();
        drawDebugLine();
    }

    window.mdbookEnableThresholdDebug = mdbookEnableThresholdDebug;

    // Updates which headers in the sidebar should be expanded. If the current
    // header is inside a collapsed group, then it, and all its parents should
    // be expanded.
    function updateHeaderExpanded(currentA) {
        // Add expanded to all header-item li ancestors.
        let current = currentA.parentElement;
        while (current) {
            if (current.tagName === 'LI' && current.classList.contains('header-item')) {
                current.classList.add('expanded');
            }
            current = current.parentElement;
        }
    }

    // Updates which header is marked as the "current" header in the sidebar.
    // This is done with a virtual Y threshold, where headers at or below
    // that line will be considered the current one.
    function updateCurrentHeader() {
        if (!headers || !headers.length) {
            return;
        }

        // Reset the classes, which will be rebuilt below.
        const els = document.getElementsByClassName('current-header');
        for (const el of els) {
            el.classList.remove('current-header');
        }
        for (const toggle of headerToggles) {
            toggle.classList.remove('expanded');
        }

        // Find the last header that is above the threshold.
        let lastHeader = null;
        for (const header of headers) {
            const rect = header.getBoundingClientRect();
            if (rect.top <= threshold) {
                lastHeader = header;
            } else {
                break;
            }
        }
        if (lastHeader === null) {
            lastHeader = headers[0];
            const rect = lastHeader.getBoundingClientRect();
            const windowHeight = window.innerHeight;
            if (rect.top >= windowHeight) {
                return;
            }
        }

        // Get the anchor in the summary.
        const href = '#' + lastHeader.id;
        const a = [...document.querySelectorAll('.header-in-summary')]
            .find(element => element.getAttribute('href') === href);
        if (!a) {
            return;
        }

        a.classList.add('current-header');

        updateHeaderExpanded(a);
    }

    // Updates which header is "current" based on the threshold line.
    function reloadCurrentHeader() {
        if (disableScroll) {
            return;
        }
        updateThreshold();
        updateCurrentHeader();
    }


    // When clicking on a header in the sidebar, this adjusts the threshold so
    // that it is located next to the header. This is so that header becomes
    // "current".
    function headerThresholdClick(event) {
        // See disableScroll description why this is done.
        disableScroll = true;
        setTimeout(() => {
            disableScroll = false;
        }, 100);
        // requestAnimationFrame is used to delay the update of the "current"
        // header until after the scroll is done, and the header is in the new
        // position.
        requestAnimationFrame(() => {
            requestAnimationFrame(() => {
                // Closest is needed because if it has child elements like <code>.
                const a = event.target.closest('a');
                const href = a.getAttribute('href');
                const targetId = href.substring(1);
                const targetElement = document.getElementById(targetId);
                if (targetElement) {
                    threshold = targetElement.getBoundingClientRect().bottom;
                    updateCurrentHeader();
                }
            });
        });
    }

    // Takes the nodes from the given head and copies them over to the
    // destination, along with some filtering.
    function filterHeader(source, dest) {
        const clone = source.cloneNode(true);
        clone.querySelectorAll('mark').forEach(mark => {
            mark.replaceWith(...mark.childNodes);
        });
        dest.append(...clone.childNodes);
    }

    // Scans page for headers and adds them to the sidebar.
    document.addEventListener('DOMContentLoaded', function() {
        const activeSection = document.querySelector('#mdbook-sidebar .active');
        if (activeSection === null) {
            return;
        }

        const main = document.getElementsByTagName('main')[0];
        headers = Array.from(main.querySelectorAll('h2, h3, h4, h5, h6'))
            .filter(h => h.id !== '' && h.children.length && h.children[0].tagName === 'A');

        if (headers.length === 0) {
            return;
        }

        // Build a tree of headers in the sidebar.

        const stack = [];

        const firstLevel = parseInt(headers[0].tagName.charAt(1));
        for (let i = 1; i < firstLevel; i++) {
            const ol = document.createElement('ol');
            ol.classList.add('section');
            if (stack.length > 0) {
                stack[stack.length - 1].ol.appendChild(ol);
            }
            stack.push({level: i + 1, ol: ol});
        }

        // The level where it will start folding deeply nested headers.
        const foldLevel = 3;

        for (let i = 0; i < headers.length; i++) {
            const header = headers[i];
            const level = parseInt(header.tagName.charAt(1));

            const currentLevel = stack[stack.length - 1].level;
            if (level > currentLevel) {
                // Begin nesting to this level.
                for (let nextLevel = currentLevel + 1; nextLevel <= level; nextLevel++) {
                    const ol = document.createElement('ol');
                    ol.classList.add('section');
                    const last = stack[stack.length - 1];
                    const lastChild = last.ol.lastChild;
                    // Handle the case where jumping more than one nesting
                    // level, which doesn't have a list item to place this new
                    // list inside of.
                    if (lastChild) {
                        lastChild.appendChild(ol);
                    } else {
                        last.ol.appendChild(ol);
                    }
                    stack.push({level: nextLevel, ol: ol});
                }
            } else if (level < currentLevel) {
                while (stack.length > 1 && stack[stack.length - 1].level > level) {
                    stack.pop();
                }
            }

            const li = document.createElement('li');
            li.classList.add('header-item');
            li.classList.add('expanded');
            if (level < foldLevel) {
                li.classList.add('expanded');
            }
            const span = document.createElement('span');
            span.classList.add('chapter-link-wrapper');
            const a = document.createElement('a');
            span.appendChild(a);
            a.href = '#' + header.id;
            a.classList.add('header-in-summary');
            filterHeader(header.children[0], a);
            a.addEventListener('click', headerThresholdClick);
            const nextHeader = headers[i + 1];
            if (nextHeader !== undefined) {
                const nextLevel = parseInt(nextHeader.tagName.charAt(1));
                if (nextLevel > level && level >= foldLevel) {
                    const toggle = document.createElement('a');
                    toggle.classList.add('chapter-fold-toggle');
                    toggle.classList.add('header-toggle');
                    toggle.addEventListener('click', () => {
                        li.classList.toggle('expanded');
                    });
                    const toggleDiv = document.createElement('div');
                    toggleDiv.textContent = '❱';
                    toggle.appendChild(toggleDiv);
                    span.appendChild(toggle);
                    headerToggles.push(li);
                }
            }
            li.appendChild(span);

            const currentParent = stack[stack.length - 1];
            currentParent.ol.appendChild(li);
        }

        const onThisPage = document.createElement('div');
        onThisPage.classList.add('on-this-page');
        onThisPage.append(stack[0].ol);
        const activeItemSpan = activeSection.parentElement;
        activeItemSpan.after(onThisPage);
    });

    document.addEventListener('DOMContentLoaded', reloadCurrentHeader);
    document.addEventListener('scroll', reloadCurrentHeader, { passive: true });
})();

