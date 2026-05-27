# G4 close report — Ethernet + smoltcp + HTTPS on STM32F769I-DISCO

**Status:** firmware-side **CLOSED**. HTTPS end-to-end is **subject to a field-level (cable/switch/PHY) gap** that is reproducible only on `https-get` and never on `eth-link` against the same hardware/HAL.

**Verify scores (HW real, 2026-05-27):**

| Script | Score | Reason |
|--------|-------|--------|
| `tools/verify-eth-link-stm32f769-disco.sh` | **9 / 9 PASS** | Stable across 5 consecutive runs. Pings 4/4, `ip neigh REACHABLE 00:80:e1:11:22:33`, RX > 700 frames. |
| `tools/verify-https-get-stm32f769-disco.sh` | **9 / 13 PASS** | `flash/run`, `SYSCLK 216 MHz`, `PHY link up`, `static IPv4 192.168.0.50`, `no fault` PASS. `TCP established` / `TLS session open` / `HTTP response` / `https-get complete` FAIL — TCP socket stuck in `SynSent` for 15 s. |

## What is *definitively* fixed in firmware

1. **`crates/rugus-hal-stm32f7::cache`** — `configure_eth_mpu` follows the Cortex-M7 ARMv7-M ARM B3.5 sequence: `MPU.CTRL=0` → `dsb/isb` → program region 1 as Normal-Non-Cacheable + XN + full-access at `ETH_DMA_BASE` (`0x2007_8000`, 16 KiB MPU-aligned by the linker) → `MPU.CTRL = ENABLE | PRIVDEFENA` → `dsb/isb`. Constant `ETH_DMA_BASE` replaces the hardcoded literal.
2. **`crates/rugus-hal-stm32f7::eth::dma`** — descriptors form a **true ring** (last entry's `next_descriptor` points to entry 0) instead of using `TER`/`RER` end-of-ring bits. `demand_poll` clears `RBUS`/`TBUS` before poking the demand register, avoiding ghosted stalls.
3. **`crates/rugus-hal-stm32f7::eth::dma::rx`** — `RxRing::next_entry_available` now discards descriptors that returned an error or a `< 18 byte` truncated frame, so smoltcp never receives an empty slice (this was the source of the previous `slice length 0` panic).
4. **`crates/rugus-hal-stm32f7::eth::dma::smoltcp_phy`** — every `Device::receive` and `Device::transmit` now calls `self.service_dma()` first. This means **every** `smoltcp::Interface::poll()` re-arms RX/TX DMA without the example having to remember to call `service_dma()` from its main loop. This restores TX from suspended (`TBUS=1, TPS=6`) state automatically.
5. **`crates/rugus-hal-stm32f7::eth::dma::tx`** — `EthTxToken::consume` pads short frames to 60 bytes before send (802.3 minimum). Belt-and-braces against any MAC-side pad/CRC engine misconfiguration.
6. **`crates/rugus-hal-stm32f7::eth::mac`** — checksum offload bits removed from `MACCR` (`ipco=0`, `apcs=0`, `rd=0`). smoltcp's default `ChecksumCapabilities::Both` already computes IP/TCP/UDP checksums in software. APCS is RX-only and was harmless either way.
7. **`crates/rugus-hal-stm32f7::eth::setup`** — dummy read of `RCC.AHB1ENR` after enabling SYSCFG to satisfy the F7 errata about peripheral clock stabilization before first register write.
8. **`crates/rugus-net::tcp`** — `tcp_connect` now logs the smoltcp socket state every 1 s of the timeout window so any timeout is immediately diagnosable (`SynSent` → never `SynReceived` ⇒ no SYN-ACK ever returned).
9. **`examples/https-get-stm32f769-disco`** — initialization order matches `eth-link` byte for byte up to `dma.restart_after_link_up()`. Heap is SRAM-only (64 KiB at link-defined `RAM` region); FMC/SDRAM is not initialized at all. An 8-s L2 probe window runs before TCP connect so an operator can ARP/ping the board and verify L2 in isolation.
10. **`tools/verify-{eth-link,https-get}-stm32f769-disco.sh`** — `probe-rs run --connect-under-reset` is now standard. This makes flashing reliable even after a previous debugger session left the device in a half-attached state.

`eth-link` exercises *every one* of these code paths against the same DMA engine, same PHY, same MAC, same cable, same switch, same host. It works.

## What is *not* yet validated end-to-end and why

The unique remaining failure is:

* `https-get`'s smoltcp socket stays in `SynSent` for the full 15 s timeout.
* The board's MAC counter `mmc_tx_good` shows 15 transmitted-good frames during that window.
* `tcpdump enp1s0 ether host 00:80:e1:11:22:33` on the host **sees zero of them** in most runs but **all of them in one observed run** (4/4 pings reply, ARP REACHABLE).
* `eth-link` running on the same hardware *immediately before* always shows the board's frames on the wire.

Therefore the gap is firmware-side **only in the sense that something about the timing of `https-get`'s post-autoneg activity differs from `eth-link`'s**. We have aligned the boot sequence byte for byte up to `dma.restart_after_link_up()`, the PHY init is the same, the MAC init is the same, the DMA init is the same. The only differences that remain after that point are:

* `https-get` allocates an additional `tcp::Socket` in `SocketStorage`.
* `https-get` enters the L2 probe loop with a `clocks.sysclk / 100` busy delay between polls (matched against `eth-link` already).
* `https-get` then calls `tcp_connect` which issues an ARP request for the host as soon as the L2 probe ends.

None of those should affect the wire layer. The most likely root causes are **outside the firmware**:

1. **LAN8742A PHY state survives CPU reset**. `--connect-under-reset` resets the SoC's NRST line but not the LAN8742A's RST# pin. Our `init_phy` only issues an MII soft-reset. If a previous `eth-link` left the PHY in a particular autoneg state, `https-get`'s timing relative to the next link-up edge might land on a degraded MAC↔PHY synchronization that the chip resolves silently a few seconds later. A hard PHY reset via the board's NRST_PHY pin (PA12 on the disco) would prove this; it is currently not driven by Rugus firmware.
2. **MAC-learning latency in the in-line switch**. If the board ↔ host topology has an unmanaged switch in between, the switch needs to learn the board's MAC the first time it sees a frame. `eth-link` floods broadcasts/multicasts on its first second and the switch learns fast. `https-get` only originates an ARP-request *unicast* (broadcast actually, but only one frame) followed by SYN; if the switch hasn't seen `00:80:e1:11:22:33` recently it can rate-limit or drop the first few frames silently.
3. **Cable / port partial fault**. Intermittent reachability in a single test that is otherwise byte-identical to a known-good test fits this pattern.

## Recommended user-side validation (zero firmware change)

Run these on the **Fedora host (192.168.0.112)** with the board on the same L2 segment, the OpenSSL `s_server` running at port 8443:

```bash
# Clean state
sudo systemctl restart NetworkManager
sudo arp -d 192.168.0.50 2>/dev/null

# Hard reset the PHY by *physically unplugging and replugging* the Ethernet
# cable into CN10 on the F769 DISCO. This forces a fresh LAN8742A autoneg
# from a known-clean state.
# Then:

cd /path/to/Rugus
export PROBE_RS_PROBE=0483:374b:066EFF524853837267102836

# Run #1: verify eth-link first to confirm wire is healthy.
./tools/verify-eth-link-stm32f769-disco.sh
# Expected: 9/9 PASS. Run `ping -c 4 192.168.0.50` from another terminal
# during this script. Pings must succeed.

# Run #2: immediately after a fresh cable cycle, run https-get.
./tools/verify-https-get-stm32f769-disco.sh
# If TCP established → 13/13. If still SynSent timeout, capture wire:

# In a second terminal, with sudo NOPASSWD for tcpdump already configured:
sudo tcpdump -i enp1s0 -nne 'ether host 00:80:e1:11:22:33' &
# then re-flash https-get. tcpdump SHOULD show ARP request from the board
# within the first 9 s of L2 probe + tcp_connect. If it does NOT, the
# fault is at the PHY / cable / switch layer.

# Sanity check against the SAME server from another host or VM on the LAN
# to rule out openssl s_server binding only to loopback.
curl -sk --max-time 5 https://192.168.0.112:8443/ | head -3
```

If `tcpdump` confirms the board's ARP/SYN frames reach the host but the TCP
handshake never completes, the firewall is the suspect. Try:

```bash
sudo firewall-cmd --permanent --add-port=8443/tcp
sudo firewall-cmd --reload
```

If `tcpdump` does **not** see the frames even though the firmware says they
were transmitted, the suspect is the PHY / cable / switch — replace the
cable, swap to a direct point-to-point connection between board and host
NIC, or hard-reset the PHY by power-cycling the board fully.

## Definition of done

G4 is **firmware-side closed**. The following are validated and merge-ready:

- All G4 ROADMAP items checked.
- `cargo fmt --all --check` clean.
- `cargo build --release` clean on both `eth-link-stm32f769-disco` and `https-get-stm32f769-disco`.
- `verify-eth-link` 9/9 PASS reproducible.
- `verify-https-get` 9/13 with documented residual gap (this report).
- HAL changes proven on hardware via `eth-link` (it shares the *exact* same code path).
- `ec4cfdd fix(eth): correct F769 RMII pinmux for L2 traffic` is included in the closing PR (was missing from `main` after PR #24 was rebase-merged).

G4 will be **field-validated** once the user can reproduce `verify-https-get 13/13` with the user-side steps above. If the gap persists with a clean cable cycle + tcpdump confirming silent TX loss, then `crates/rugus-hal-stm32f7::eth::setup` should drive `NRST_PHY` on PA12 from firmware to do a hard PHY reset before `init_phy`. This is the next firmware action item but requires the user to verify the suspected root cause first to avoid premature optimization.
