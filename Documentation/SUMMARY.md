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
