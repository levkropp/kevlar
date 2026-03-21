# M10.5 Phase 5: Network Drivers

**Goal:** Load Ethernet and WiFi drivers from Linux 6.18. Real NIC gets
a DHCP address; `apk update` downloads packages over real hardware network.

---

## Target drivers

| Driver | Devices | .ko files |
|--------|---------|-----------|
| e1000e | Intel Gigabit Ethernet (server/workstation NICs) | `e1000e.ko` |
| r8169 | Realtek Gigabit (most consumer motherboards) | `r8169.ko` |
| igc | Intel I225/I226 2.5GbE | `igc.ko` |
| iwlwifi | Intel WiFi 6/6E/7 | `iwlwifi.ko`, `iwlmvm.ko` |
| ath11k | Qualcomm WiFi 6 | `ath11k.ko`, `ath11k_pci.ko` |
| mt7921 | MediaTek WiFi 6 | `mt7921e.ko` |

Start with `r8169` — it's the most common consumer Ethernet driver (~3K lines),
widely tested, and has simple hardware. `e1000e` for server/VM NICs.
WiFi after Ethernet is working.

---

## net_device shim

Linux network drivers register a `struct net_device` with the kernel:

```c
struct net_device {
    char name[IFNAMSIZ];    // "eth0", "wlan0"
    unsigned long state;    // LINK_STATE_START, etc.
    netdev_features_t features;
    const struct net_device_ops *netdev_ops;
    const struct ethtool_ops *ethtool_ops;
    unsigned int mtu;
    unsigned char dev_addr[MAX_ADDR_LEN];  // MAC address
    // ... ~300 more fields
};

struct net_device_ops {
    int (*ndo_open)(struct net_device *dev);
    int (*ndo_stop)(struct net_device *dev);
    netdev_tx_t (*ndo_start_xmit)(struct sk_buff *skb, struct net_device *dev);
    // ... rx config, stats, etc.
};
```

When a packet arrives from hardware, the driver calls `netif_rx()` or
`napi_gro_receive()`. When the kernel wants to send, it calls `ndo_start_xmit()`.

### Key functions

| Function | Implementation |
|----------|----------------|
| `alloc_etherdev(sizeof_priv)` | Allocate `net_device` + private data |
| `free_netdev(dev)` | Free |
| `register_netdev(dev)` | Register with network stack |
| `unregister_netdev(dev)` | Unregister |
| `netif_carrier_on/off(dev)` | Signal link up/down |
| `netif_rx(skb)` | Deliver received packet to network stack |
| `netif_napi_add(dev, napi, poll)` | Register NAPI poll handler |
| `napi_enable/disable(napi)` | Enable/disable NAPI polling |
| `napi_schedule(napi)` | Schedule NAPI poll (from IRQ handler) |
| `napi_complete_done(napi, work)` | Finish NAPI poll cycle |
| `netdev_alloc_skb(dev, len)` | Allocate socket buffer |
| `dev_kfree_skb(skb)` | Free socket buffer |

---

## Socket buffer (sk_buff)

`struct sk_buff` is Linux's network packet representation. It's a complex
structure with headroom, data, tailroom, and a chain of linear/paged fragments:

```c
struct sk_buff {
    struct sk_buff *next, *prev;
    unsigned char *head, *data, *tail, *end;
    unsigned int len, data_len;
    __be16 protocol;
    // ... 50+ more fields
};
```

For kcompat, we need enough of sk_buff for drivers to:
1. Fill the data region with packet bytes (RX)
2. Read the data region for transmission (TX)
3. Chain skbs for large packets (rare in Tier 1 drivers)

The mapping between kcompat sk_buff and Kevlar's network stack:
- RX: driver fills sk_buff → kcompat extracts bytes → smoltcp ingests packet
- TX: smoltcp produces packet bytes → kcompat allocates sk_buff → driver transmits

---

## NAPI (New API)

NAPI is Linux's interrupt coalescing mechanism for high-throughput NICs:

1. First packet: hardware IRQ fires, driver calls `napi_schedule()`
2. IRQ disabled for this NIC
3. Network stack calls `poll()` in a kernel thread context
4. Driver processes up to `budget` packets per poll call
5. If `budget` exhausted: return (more work pending, poll again)
6. If done: re-enable IRQ, `napi_complete_done()`

kcompat implements NAPI as a kernel thread per NAPI context that runs when
`napi_schedule()` is called.

---

## Ethernet → smoltcp bridge

Kevlar uses smoltcp as its network stack. kcompat bridges native drivers
to smoltcp:

```rust
// RX path (driver → smoltcp)
fn netif_rx(skb: *mut sk_buff) {
    let data = skb_data_slice(skb);
    smoltcp_interface.receive(data);  // inject Ethernet frame
    dev_kfree_skb(skb);
}

// TX path (smoltcp → driver)
impl smoltcp::phy::TxToken for KcompatTxToken {
    fn consume(self, len: usize, f: impl FnOnce(&mut [u8])) {
        let skb = netdev_alloc_skb(self.dev, len);
        f(skb_put(skb, len));
        self.dev.netdev_ops.ndo_start_xmit(skb, self.dev);
    }
}
```

This replaces Kevlar's existing virtio-net with native drivers on real hardware.
On QEMU, virtio-net continues to work as before.

---

## WiFi: cfg80211 / mac80211

WiFi drivers use a more complex stack:

```
iwlwifi.ko (hardware driver)
        │
        ▼
iwlmvm.ko (multi-radio management)
        │
        ▼
mac80211.ko (802.11 MAC sublayer: association, crypto, rate control)
        │
        ▼
cfg80211.ko (regulatory, NL80211 userspace interface)
```

`cfg80211` exposes the `NL80211` netlink interface — this is how `iw`,
`wpa_supplicant`, and NetworkManager configure WiFi.

For M10.5, the minimum viable WiFi:
1. Implement cfg80211 + mac80211 kcompat (~15K lines of shim)
2. Load iwlwifi + iwlmvm
3. WiFi interface appears (`wlan0`)
4. `iw dev wlan0 scan` → network list
5. `wpa_supplicant -c /etc/wpa_supplicant.conf` → associates to AP
6. DHCP → IP address

WiFi is harder than Ethernet (firmware loading, crypto, 802.11 state machine)
but essential for laptop support. The firmware files (`iwlwifi-*.ucode`) must
be available at `/lib/firmware/`.

### Firmware loading

Drivers load firmware via `request_firmware(fw, name, dev)`:

```c
ret = request_firmware(&fw, "iwlwifi-so-a0-gf-a0-89.ucode", &pdev->dev);
```

kcompat implements `request_firmware` by reading from Kevlar's filesystem
(firmware files pre-packaged in initramfs or loaded from ext4 `/lib/firmware/`).

---

## Verification

### Ethernet (real hardware)

```bash
insmod e1000e.ko   # or r8169.ko for Realtek
ip link show       # eth0 appears
ip link set eth0 up
udhcpc -i eth0     # DHCP
ping 8.8.8.8       # internet connectivity
apk update         # download package lists
```

### WiFi

```bash
insmod iwlwifi.ko iwlmvm.ko
ip link show       # wlan0 appears
iw dev wlan0 scan
wpa_supplicant -B -i wlan0 -c /etc/wpa_supplicant.conf
udhcpc -i wlan0
ping 8.8.8.8
```

---

## Files to create/modify

- `kernel/kcompat/net_device.rs` — `struct net_device`, NAPI, sk_buff
- `kernel/kcompat/cfg80211.rs` — WiFi cfg80211/mac80211 shim
- `kernel/kcompat/nl80211.rs` — NL80211 netlink interface
- `kernel/kcompat/firmware.rs` — `request_firmware` from filesystem
- `kernel/kcompat/symbols_6_18.rs` — add net/WiFi symbols
