# Kernel Parameters

Kernel parameters configure Kevlar at boot time.

## Available Parameters

| Name | Description | Example |
|------|-------------|---------|
| `log` | Logging level (see [Logging](logging.md)) | `log=trace` |
| `serial1` | Send kernel log to secondary serial port | `serial1=on` |
| `dhcp` | Set to `off` to disable the DHCP client | `dhcp=off` |
| `ip4` | Static IPv4 address with prefix length | `ip4=10.0.0.123/24` |
| `gateway_ip4` | Static gateway IPv4 address | `gateway_ip4=10.0.0.1` |
| `pci` | Set to `off` to skip PCI device discovery | `pci=off` |
| `pci_device` | Allowlist specific PCI devices (`bus:slot`); repeatable | `pci_device=0:1` |
| `virtio_mmio.device` | virtio devices connected over MMIO; repeatable | `virtio_mmio.device=@0xf000:12` |
| `debug` | Enable structured debug events (see [Debugging 101](hacking/debugging-101.md)) | `debug=all` |

## How to Set Kernel Parameters

### make

Pass parameters via `CMDLINE=`:

```
make run CMDLINE="dhcp=off ip4=10.0.0.5/24"
```

### GRUB2

Append parameters after the kernel image path:

```
menuentry "Kevlar" {
    multiboot2 /boot/kevlar.elf dhcp=off ip4=10.0.0.5/24
}
```
