# STM32F769I-DISCO

Evaluation board for **STM32F769NIH6** (Cortex-M7 @ 216 MHz). Rugus genesis
product board (G0–G4).

## Quick reference

| Item | Value |
|------|-------|
| MCU | STM32F769NIH6 |
| HSE | 25 MHz (PH0/PH1) |
| probe-rs chip | `STM32F769NIHx` |
| Onboard ST-Link | USB; V2-1 composite (`0483:374b` when F407 also connected) |
| User LEDs | LD1 PJ13 (red), LD2 PJ5 (green), LD3 PA12, LD4 PD4 |
| External SDRAM | 16 MB @ 0xC0_0000_00 (FMC) |
| QSPI NOR flash | Macronix **MX25L51245G** 512 Mbit/64 MiB (JEDEC `C2 20 1A`) — CLK PB2, NCS PB6, IO0 PC9, IO1 PC10, IO2 PE2, IO3 PD13 |

## Ethernet (G4)

| Item | Value |
|------|-------|
| PHY | SMSC **LAN8742A** (RMII), address **0** |
| Connector | RJ45 **CN10** (Ethernet LAN) |
| RMII pins | REF_CLK PA1, CRS_DV PA7, TX_EN PG11, TXD0 PG13, TXD1 PG14, RXD0 PC4, RXD1 PC5, MDIO PA2, MDC PC1 |
| HAL module | `rugus-hal-stm32f7::eth` |
| Examples | `eth-link-stm32f769-disco`, `https-get-stm32f769-disco` |
| Verify | `./tools/verify-eth-link-stm32f769-disco.sh`, `./tools/verify-https-get-stm32f769-disco.sh` |

Connect an Ethernet cable to **CN10** and a switch/router (or direct PC link)
before expecting `link up` in RTT logs. Default static IPv4 in examples:
`192.168.0.50/24` (gateway `192.168.0.1`). HTTPS example expects a LAN server
at `192.168.0.112:8443` — see
[`examples/https-get-stm32f769-disco/README.md`](../../examples/https-get-stm32f769-disco/README.md).

## Examples (verified)

| Example | Milestone |
|---------|-----------|
| `blink-stm32f769-disco` | G0 |
| `dual-blink-stm32f769-disco` | G1 |
| `app-sandbox-stm32f769-disco` | G2 |
| `eth-link-stm32f769-disco` | G4 step 1 |
| `https-get-stm32f769-disco` | G4 |
| `net-service-stm32f769-disco` | F5.B.1 (pila IP como servicio) |
| `net-userland-stm32f769-disco` | F5.B.2 (sockets UDP+TCP cliente userland por syscall+IPC bajo MPU) |
| `qspi-probe-stm32f769-disco` | F5.C.1 (driver QSPI NOR MX25L51245G como `BlockDevice`) |
| `fs-probe-stm32f769-disco` | F5.C.2 (almacén log-structured `rugus-fs` sobre QSPI; set/get + remontaje + contador de arranques persistente) |
| `fs-userland-stm32f769-disco` | F5.C.3 (API de ficheros userland open/read/write/close por syscall+IPC bajo MPU sobre `rugus-fs`; persiste config + log circular de faults) |
| `tickless-stm32f769-disco` | F5.A.1 (tick dinámico: el scheduler reprograma SysTick al próximo plazo en idle; LED 1 Hz exacto con ~16 IRQs/s vs 1000 fijas, sin deriva del reloj) |
| `stop-mode-stm32f769-disco` | F5.A.2 (modo STOP con wake por RTC/LSI: en plazos ≥ umbral el idle apaga HSE/PLL y entra en STOP, despertando con el wakeup timer del RTC y restaurando 216 MHz; LED 2 s con +1 entrada a STOP por ciclo y `now_ms` +2001 ms, LSI dentro de ±5 %) + F5.A.3 (contabilidad de energía: `time::idle_ms` acumula el tiempo dormido y la consola `power` expone uptime / idle % / systick_irqs / stop_entries vía `PowerStats`; ~99 % idle medido) |

## Multi-board lab

When **STM32F407G-DISC1** is also connected, select the F769 probe explicitly:

```bash
export PROBE_RS_PROBE=0483:374b:066EFF524853837267102836
```

See also [STM32F407G-DISC1](stm32f407g-disco.md).
