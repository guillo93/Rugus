# STM32F103C8 Blue Pill

**Rugus lite** minimal tier — Cortex-M3, no MPU, no FPU. First example:
`examples/blink-stm32f103c8-bluepill`.

## Identity

| Field | Value |
|-------|-------|
| Board | STM32F103C8T6 “Blue Pill” (generic clone) |
| MCU | STM32F103C8T6 (LQFP48, 64 KB flash, 20 KB SRAM) |
| Core | ARM Cortex-M3 (ARMv7-M, **no FPU**) |
| Rust target | `thumbv7m-none-eabi` |
| probe-rs chip | `STM32F103C8` |
| Reference manual | RM0008 (STM32F101/103) |

## User LED

| LED | Pin | Notes |
|-----|-----|-------|
| On-board | PC13 | Active **low** on most clones (inverted logic) |

## Debug and flash

- **External ST-Link** (or clone) required — no onboard debugger.
- SWD header: SWDIO, SWCLK, GND, 3.3 V (optional target power from probe).
- Typical wiring:

| ST-Link | Blue Pill |
|---------|-----------|
| SWDIO | SWDIO (often labeled DIO) |
| SWCLK | SWCLK (CLK) |
| GND | GND |
| 3.3V | 3.3V (if powering from probe) |

- **BOOT0** must be **low** (jumper to GND) for normal flash/run.
- If `probe-rs` reports `chipid 0x000` or `JtagGetIdcodeError`, check wiring and
  that only one probe is selected via `PROBE_RS_PROBE`.

### probe-rs probe selection

List adapters:

```bash
probe-rs list
```

Default in `tools/verify-blink-stm32f103c8-bluepill.sh` targets the external
ST-Link V2 clone (`0483:3748:…`). Override when the F769 onboard ST-Link is
also connected:

```bash
export PROBE_RS_PROBE=0483:3748:55C3BF6B0648C2875752685117C287
./tools/verify-blink-stm32f103c8-bluepill.sh
```

Flash manually:

```bash
cd examples/blink-stm32f103c8-bluepill
cargo run --release
```

## Clocks (this example)

| Source | Frequency | Notes |
|--------|-----------|-------|
| HSI | 8 MHz | Default SYSCLK — no PLL, works without HSE |
| HSE | 8 MHz | Ceramic resonator on many clones (future RCC option) |
| SYSCLK (max) | 72 MHz | PLL from HSE — not used in kickoff blink |

## Rugus implications

| Topic | Impact |
|-------|--------|
| Arch | Cortex-M3 — same `rugus-arch-cortex-m` backend, target `thumbv7m-none-eabi` |
| MPU | **None** on F103 — G2-style userland sandbox cannot be hardware-enforced |
| FPU | None — use `thumbv7m-none-eabi`, not `thumbv7em-none-eabihf` |
| Memory | Tight (20 KB RAM) — “lite” kernel: fewer tasks, no SDRAM, minimal heap |
| G2 features | Syscalls/sched may run; domain isolation is cooperative only |

## HAL and example

- HAL crate: `rugus-hal-stm32f1` (`gpio`, `rcc`).
- Example: `blink-stm32f103c8-bluepill` — PC13 toggle @ HSI 8 MHz + defmt RTT.
- Verify: `./tools/verify-blink-stm32f103c8-bluepill.sh`.

## Related

- G3 reference: [STM32F407G-DISC1](stm32f407g-disco.md)
- Project context: `docs/agent-memory/project.md`
