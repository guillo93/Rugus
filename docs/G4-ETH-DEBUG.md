# G4 Ethernet L2 debug checklist (STM32F769I-DISCO)

## Root cause (2026-05-26)

Two pinmux bugs in `configure_disco_pins()` prevented L2 traffic:

1. **Wrong TX pins** — code used PB11/PB12/PB13 (USB ULPI on this board).  
   **Correct (UM2033 Table 14, ST Cube `HAL_ETH_MspInit`):** PG11 `ETH_TX_EN`, PG13 `ETH_TXD0`, PG14 `ETH_TXD1`.

2. **REF_CLK / RX as GPIO input** — PA1, PA7, PC4, PC5 were `MODER=00`.  
   RMII signals must be **AF11** (`MODER=10`); otherwise REF_CLK never reaches the MAC and RX DMA stays stopped (`rps=0`).

Overnight MPU / `.eth_dma` / cache fixes were necessary but **not sufficient** without correct RMII mux.

## After fix — expected RTT

```
PHY link up (autoneg done)
ETH regs maccr=... mmc_rx=... mmc_tx=...
ETH DMA restarted ... rps=3 tps=6    ← RX running (was rps=0 before AF11 fix)
ETH rx=N tx=M ... (ping 192.168.0.50 now)   ← N,M > 0 during host ping/arping
```

Verified 2026-05-26: `arping -I enp1s0 192.168.0.50` → replies from `02:00:52:55:47:01`; `ping 192.168.0.50` → ICMP replies (~9 ms).

## Hardware

| Item | Value |
|------|-------|
| RJ45 | **CN10** (not CN3 — CN3 is power jumper) |
| PHY | LAN8742A @ address 0 |
| IP (examples) | `192.168.0.50/24` |
| MAC | `02:00:52:55:47:01` |

Cable: same L2 segment as the host (switch or direct PC↔CN10).

## User test (Windows or Linux)

1. Reflash: `./tools/verify-eth-link-stm32f769-disco.sh` (9/9 PASS).
2. PC static IP same subnet, e.g. `192.168.0.112/24`, gateway optional for direct link.
3. Ping: `ping 192.168.0.50`
4. Watch RTT 30 s: `rx` and `mmc_rx` should increase during ping/ARP.

If ping fails but RTT shows `rx>0`: stack/ARP issue (unlikely after fix).  
If `rx=0` and `mmc_rx=0` during ping: cable/port/segment — not firmware.

## Register dump checklist (RTT or debugger)

| Register | Address / access | Notes |
|----------|------------------|-------|
| SYSCFG PMC | `SYSCFG->PMC` bit 23 | RMII selected |
| MACCR | `ETH->MACCR` | FES+DM match PHY speed after `sync_mac_speed_from_phy` |
| DMABMR | `ETH->DMABMR` | Bus mode programmed |
| DMASR | `ETH->DMASR` | RBUS/TBUS sticky — cleared by `service_dma()` |
| DMAOMR | `ETH->DMAOMR` | SR+ST set after link-up |
| PHY BMSR | MII reg 1 | bit 2 = link |
| PHY SSR | MII reg 31 | speed/duplex after autoneg |
| MMC RGUFCR | `ETH_MMC->MMCRGUFCR` | HW RX unicast count |

## Honest HW suspects if software checks pass

- Cable in CN10, same switch/VLAN as host.
- Direct cable: PC may need auto-MDIX; try known-good patch cable.
- PoE / power jumper CN3 on wrong position (board runs but PHY isolated).
- Damaged magnetics or LAN8742 (link-up via MDIO can still lie if partner is a link partner on wrong port).
