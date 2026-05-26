# STM32F103C8 Blue Pill

**Planned** minimal tier for Rugus after G3 — future **“Rugus lite”** personality.
No HAL crate or example in the repo yet; this document captures hardware facts
for when an agent or contributor picks it up.

## Identity

| Field | Value |
|-------|-------|
| Board | STM32F103C8T6 “Blue Pill” (generic clone) |
| MCU | STM32F103C8T6 (LQFP48, 64 KB flash, 20 KB SRAM) |
| Core | ARM Cortex-M3 (ARMv7-M, **no FPU**) |
| Rust target | `thumbv7m-none-eabi` |
| probe-rs chip | `STM32F103C8` (verify exact string with `probe-rs chip list`) |
| Reference manual | RM0008 (STM32F101/103) |

## User LED

| LED | Pin | Notes |
|-----|-----|-------|
| On-board | PC13 | Active **low** on most clones (inverted logic) |

## Debug and flash

- **External ST-Link** (or clone) required — no onboard debugger.
- SWD header: SWDIO, SWCLK, GND, 3.3 V.
- Some clones need **BOOT0** jumper for first flash; document per board.

## Clocks (typical)

| Source | Frequency | Notes |
|--------|-----------|-------|
| HSE | 8 MHz | Ceramic resonator on many clones (less accurate than crystal) |
| SYSCLK | 72 MHz | F103 maximum with PLL from HSE |

## Rugus implications

| Topic | Impact |
|-------|--------|
| Arch | Cortex-M3 — same `rugus-arch-cortex-m` backend, different target triple |
| MPU | **None** on F103 — G2-style userland sandbox cannot be hardware-enforced |
| FPU | None — use `thumbv7m-none-eabi`, not `thumbv7em-none-eabihf` |
| Memory | Tight (20 KB RAM) — “lite” kernel: fewer tasks, no SDRAM, minimal heap |
| G2 features | Syscalls/sched may run; domain isolation is cooperative only |

## Planned direction

- HAL crate: `rugus-hal-stm32f1` (not started).
- Example: `blink-stm32f103-bluepill` (not started).
- Positioned as **post-G3** minimal cost / education tier, not a G3 deliverable.

## Related

- G3 primary target: [STM32F407G-DISC1](stm32f407g-disco.md)
- Project context: `docs/agent-memory/project.md`
