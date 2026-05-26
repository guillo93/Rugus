# Supported and planned boards

Rugus documents each **physical board** separately from chip family HAL crates.
An example name uses the board slug (e.g. `blink-stm32f407g-disco`), not only
the MCU part number.

## Status legend

| Status | Meaning |
|--------|---------|
| **Supported** | Example flashed and verified on real hardware |
| **In progress** | HAL / example under active development (G3) |
| **Planned** | Documented target; no Rugus example yet |

## Boards

| Board | MCU | Arch | Rugus status | Doc |
|-------|-----|------|--------------|-----|
| [STM32F769I-DISCO](stm32f769-disco.md) | STM32F769NIH6 | Cortex-M7 | Supported (G0–G4) | [stm32f769-disco.md](stm32f769-disco.md) |
| [STM32F407G-DISC1](stm32f407g-disco.md) | STM32F407VGT6 | Cortex-M4 | In progress (G3) | [stm32f407g-disco.md](stm32f407g-disco.md) |
| [STM32F103C8 Blue Pill](stm32f103c8-bluepill.md) | STM32F103C8T6 | Cortex-M3 | Planned (post-G3) | [stm32f103c8-bluepill.md](stm32f103c8-bluepill.md) |

## Adding a board

Follow `docs/PORTING.md` (case B — new chip, same arch). Add a row here and a
dedicated markdown file with MCU, probe-rs chip string, clock source, LEDs, and
debug adapter notes before merging the first HW-verified example.
