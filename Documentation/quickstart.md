# Quickstart

## Prerequisites

- **Rust nightly** — install via [rustup](https://rustup.rs/), then:
  ```
  rustup override set nightly
  rustup component add llvm-tools-preview rust-src
  ```
- **cargo-binutils** and **rustfilt**:
  ```
  cargo install cargo-binutils rustfilt
  ```
- **Docker** — used to build the initramfs; `docker run hello-world` must work without sudo
- **Python 3**
- **QEMU** with x86\_64 and/or aarch64 targets

### Linux (Debian/Ubuntu)
```
sudo apt install qemu-system-x86 qemu-system-arm gdb python3
```

### macOS
```
brew install qemu gdb python3
brew install --cask docker
```

## Building and Running

```
git clone https://github.com/levkropp/kevlar && cd kevlar
make run
```

`make run` builds the kernel and initramfs, then boots in QEMU. A BusyBox shell
appears in the terminal.

### Common Make Targets

| Command | Description |
|---|---|
| `make` | Build (debug) |
| `make RELEASE=1` | Build (release, optimized) |
| `make run` | Build and boot in QEMU (x86\_64) |
| `make run GDB=1` | Boot with GDB stub on `localhost:7789` |
| `make run LOG=trace` | Boot with verbose trace logging |
| `make check` | Fast type-check (no link step) |
| `make bench` | Run micro-benchmarks in QEMU |
| `make bench-kvm` | Run benchmarks with KVM acceleration |
| `make debug` | Boot with GDB + structured debug events |

### ARM64

```
make ARCH=arm64 run
```

ARM64 uses `qemu-system-aarch64 -machine virt -cpu cortex-a72`. Debug builds are
slow under TCG emulation; use `make ARCH=arm64 RELEASE=1 run` for reasonable speed.

### Safety Profiles

```
make run PROFILE=fortress      # Maximum safety (catch_unwind, copy-semantic frames)
make run PROFILE=balanced      # Default (catch_unwind, optimized usercopy)
make run PROFILE=performance   # No catch_unwind, concrete service dispatch
make run PROFILE=ludicrous     # All safety layers off
make check-all-profiles        # Type-check all four profiles
```

See [Safety Profiles](architecture/safety-profiles.md) for details.

## QEMU Controls

Type **Ctrl+A then C** to enter the QEMU monitor. Useful monitor commands:

- `q` — quit
- `info registers` — dump CPU registers
- `info mem` — dump the active page table
- `info qtree` — list peripherals

## Customizing the Init Script

Edit `testing/Dockerfile` to add binaries to the initramfs, or set `INIT_SCRIPT`
to run a specific binary as PID 1:

```
make run INIT_SCRIPT=/bin/sh
make run INIT_SCRIPT=/bin/bench
```

## Networking

The VM gets a virtio-net interface. By default, DHCP is used (`ip=dhcp` kernel
parameter). To use a static address:

```
make run CMDLINE="ip4=10.0.2.15/24"
```

The host can reach the VM on the port forwarded by QEMU's user-mode networking.
