# STM32F407G-DISC1 (Discovery)

Reference board for **G3** — second Cortex-M target in Rugus.

## Identity

| Field | Value |
|-------|-------|
| Board | STM32F407G-DISC1 |
| MCU | STM32F407VGT6 (LQFP100, 1 MB flash, 192 KB SRAM) |
| Core | ARM Cortex-M4 with FPU (ARMv7E-M) |
| Rust target | `thumbv7em-none-eabihf` |
| probe-rs chip | `STM32F407VG` |
| User manual | [UM1472](https://www.st.com/resource/en/user_manual/um1472-discovery-kit-with-stm32f407vg-mcu-stmicroelectronics.pdf) |
| HAL crate | `rugus-hal-stm32f4` (feature `stm32f407`) |

## Clocks

| Source | Frequency | Notes |
|--------|-----------|-------|
| HSE | 8 MHz | On-board crystal (PH0/PH1) |
| SYSCLK | 168 MHz | F407 maximum; PLL from HSE |
| AHB | 168 MHz | HPRE /1 |
| APB1 | 42 MHz | PPRE1 /4 (max 42 MHz) |
| APB2 | 84 MHz | PPRE2 /2 (max 84 MHz) |

RCC setup lives in `crates/rugus-hal-stm32f4/src/rcc.rs` (HSE 8 MHz → PLL →
168 MHz, flash wait states WS5).

## User LEDs (UM1472 §6.4)

All four user LEDs are on **GPIOD**, active high:

| LED | Colour | Pin | Enum (`DiscoLed`) |
|-----|--------|-----|-------------------|
| LD3 | Orange | PD13 | `Orange` |
| LD4 | Green | PD12 | `Green` |
| LD5 | Red | PD14 | `Red` |
| LD6 | Blue | PD15 | `Blue` |

The G3 blink example toggles **LD4 (green, PD12)**.

## Debug and flash

- **On-board ST-Link/V2-A** (USB micro-B). No external probe required for
  development.
- SWD: `probe-rs run --chip STM32F407VG`
- With multiple ST-Links connected, set `PROBE_RS_PROBE=VID:PID:Serial` (see
  `probe-rs list`) or export it in the verify script environment.
- RTT logging via onboard ST-Link SWD (same workflow as F769 DISCO).

## Memory (blink example)

| Region | Origin | Size |
|--------|--------|------|
| FLASH | `0x0800_0000` | 1024 KiB |
| RAM | `0x2000_0000` | 128 KiB (main SRAM; CCM not used in G3 blink) |

No external SDRAM on this board (unlike F769I-DISCO).

## Rugus capabilities on this board

| Feature | G3 blink | Notes |
|---------|----------|-------|
| GPIO / LEDs | Yes | `rugus-hal-stm32f4::gpio` |
| RCC 168 MHz | Yes | `rugus-hal-stm32f4::rcc` |
| MPU | Hardware yes | Software support from G2 arch crate; not exercised in G3 blink |
| I/D-cache | N/A | Cortex-M4 has no cache |
| SDRAM | No | Not populated |

## Example

```bash
cd examples/blink-stm32f407g-disco
cargo run --release
# or
../../tools/verify-blink-stm32f407g-disco.sh
```

## Related

- Roadmap: `docs/ROADMAP.md` (G3)
- Porting: `docs/PORTING.md`
