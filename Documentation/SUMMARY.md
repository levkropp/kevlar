# SUMMARY

- [Introduction](introduction.md)
- [Quickstart](quickstart.md)
- [Compatibility Status](compatibility.md)

# Architecture

- [Overview](architecture.md)
- [The Ringkernel Architecture](architecture/ringkernel.md)
- [Safety Profiles](architecture/safety-profiles.md)
- [HAL / Kernel Split](architecture/hal.md)
- [Memory Management](architecture/memory.md)
- [Process & Thread Model](architecture/process.md)
- [Signal Handling](architecture/signals.md)
- [Filesystems](architecture/filesystems.md)
- [Networking](architecture/networking.md)

# Provenance

- [Licensing](provenance/licensing.md)
- [Source Attribution](provenance/source-attribution.md)
- [Clean-Room Implementation Log](provenance/clean-room-log.md)

# Development

- [Kernel Parameters](kernel-parameters.md)
- [Logging](logging.md)
- [Contributing](contributing.md)
- [Debugging 101](hacking/debugging-101.md)

# Blog

- [Reviving Kerla: Forking a Dead Rust Kernel](blog/001-reviving-kerla.md)
- [The Road to 170 Syscalls](blog/002-syscall-roadmap.md)
- [Milestone 1: BusyBox Boots on Kevlar](blog/003-milestone-1-busybox-boots.md)
- [Milestone 1.5: ARM64 BusyBox Boots on Kevlar](blog/004-milestone-1.5-arm64.md)
- [Milestone 2: Dynamic Linking Works on Kevlar](blog/005-milestone-2-dynamic-linking.md)
- [Milestone 3: Terminal Control, Job Control, and the Road to Bash](blog/006-milestone-3-job-control-and-terminal.md)
- [Ringkernel Phase 1: Extracting the Platform](blog/007-ringkernel-phase-1-platform-extraction.md)
- [Ringkernel Phase 2: Core Traits and the Service Registry](blog/008-ringkernel-phase-2-core-traits.md)
- [Ringkernel Phase 3: Extracting Services](blog/009-ringkernel-phase-3-service-extraction.md)
- [Configurable Safety: Choose Your Own Tradeoff](blog/010-safety-profiles.md)
- [Optimized Usercopy and Copy-Semantic Frames](blog/011-safety-profiles-phase-3-4.md)
- [Panic Containment and Capability Tokens](blog/012-catch-unwind-and-capabilities.md)
- [Benchmarks, CI Matrix, and Smarter Tooling](blog/013-benchmarks-and-ci.md)
- [Fixing Fork: Two Bugs, One Wild Pointer](blog/014-fixing-fork.md)
- [The 8-Byte Copy That Should Have Been 4](blog/015-debug-tooling-and-usercopy-fix.md)
- [From 13µs to 200ns: Closing the KVM Performance Gap](blog/016-kvm-performance.md)
- [Beating Linux: Syscall Performance in a Rust Kernel](blog/017-beating-linux-syscall-performance.md)
- [M4 Phase 1: epoll and I/O Readiness](blog/018-m4-epoll.md)
- [M4 Phase 2: Event FDs and Timer FDs](blog/019-event-fds.md)
- [M4 Phase 3: Unix Domain Sockets](blog/020-unix-sockets.md)
- [M4 Phase 4: Filesystem Mounting](blog/021-filesystem-mounting.md)
- [M4 Phase 5: Process Capabilities](blog/022-process-capabilities.md)
- [M4 Phase 6: Integration Testing](blog/023-integration-testing.md)
- [M5 Phase 1: File Metadata and Extended I/O](blog/024-file-metadata.md)
- [M5 Phase 2: inotify File Change Notifications](blog/025-inotify.md)
- [M5 Phase 3: Zero-Copy I/O](blog/027-zero-copy-io.md)
- [M5 Phase 4: /proc & /sys Completeness](blog/026-proc-sys.md)
