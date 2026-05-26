# Changelog

Todos los cambios notables de este proyecto se documentarán en este archivo.

El formato sigue [Keep a Changelog](https://keepachangelog.com/es-ES/1.1.0/)
y este proyecto se adhiere a [Semantic Versioning](https://semver.org/lang/es/).

Mientras la versión sea pre-`1.0`, **breaking changes son permitidos entre
minor versions** (`0.1.x` → `0.2.0` puede romper API). A partir de `1.0`,
SemVer estricto.

## [Unreleased]

### Added

- **G4 step 1 — Ethernet link + smoltcp (STM32F769I-DISCO).**
  - `rugus-hal-stm32f7::eth` — ETH MAC + DMA + LAN8742A RMII.
  - `rugus-hal::EthMac` trait + `EthMacPort` adapter.
  - Crate `rugus-net` — smoltcp wrapper (static IPv4 + DHCP helpers).
  - Ejemplo `examples/eth-link-stm32f769-disco` — link up, IP 192.168.1.50/24.
  - Script `tools/verify-eth-link-stm32f769-disco.sh`.

---

## [0.4.0] — 2026-05-25 — G3

Segundo chip Cortex-M: STM32F407G-DISC1 con HAL F4, dual-blink cooperativo y sandbox MPU en SRAM interna.

### Added

- **G3 — STM32F407G-DISC1 (Cortex-M4F).**
  - Crate `rugus-hal-stm32f4` — GPIO (LD3–LD6), RCC HSE 8 MHz → PLL 168 MHz.
  - Ejemplo `examples/blink-stm32f407g-disco` — LD4 toggle + defmt RTT.
  - Ejemplo `examples/dual-blink-stm32f407g-disco` — LD4/LD6 en paralelo vía scheduler; heap 32 KiB SRAM.
  - Ejemplo `examples/app-sandbox-stm32f407g-disco` — kernel + 2 apps userland, MemManage controlado (sin SDRAM).
  - Scripts `tools/verify-{blink,dual-blink,app-sandbox}-stm32f407g-disco.sh` con `PROBE_RS_PROBE` por defecto.
  - Docs `docs/boards/{README,stm32f407g-disco,stm32f103c8-bluepill}.md`.

### Validated

- **G3 en HW real (STM32F407G-DISC1, probe-rs).** Blink LD4 @ 168 MHz; dual-blink tasks A/B;
  app-sandbox MemManage + task kill; verify scripts **8/8**, **10/10**, **12/12 PASS** (2026-05-25).

---

## [0.3.0] — 2026-05-25 — G2

MPU sandbox, syscalls SVC, fault handlers con report domain+PC, ejemplo app-sandbox en STM32F769I-DISCO.

### Added

- **G2 — MPU + dominios + syscalls SVC + sandbox.**
  - `rugus-arch-cortex-m::mpu` — 8 regiones Cortex-M7 (Drivers, SDRAM, kernel RAM, flash, app stack).
  - `rugus-arch-cortex-m` — SVC handler, exception handlers (MemoryManagement/Bus/Usage/HardFault).
  - `rugus-core::domain`, `rugus-core::fault`, `rugus-core::syscall` dispatch + trampolines userland.
  - `rugus-core::sched` — `spawn_user`, `kill_current_and_resume`, remapeo MPU en switch.
  - Ejemplo `examples/app-sandbox-stm32f769-disco` — kernel + 2 apps userland, MemManage controlado.
  - Script `tools/verify-app-sandbox-stm32f769-disco.sh`.

### Validated

- **G2 en HW real (STM32F769I-DISCO, probe-rs).** MemManage en app userland reporta dominio App + PC;
  kernel mata tarea faultante; LD1 sigue parpadeando; verify script **12/12 PASS** (2026-05-25).

### Fixed

- `rugus-core::sched::pick_next` — no re-elegir la tarea actual en round-robin (kernel + apps).

---

## [0.2.0] — 2026-05-25 — G1

Clocks, SDRAM/FMC, heap, scheduler cooperativo y ejemplo dual-blink en STM32F769I-DISCO.

### Added

- **G1 — clocks, SDRAM/FMC, heap, scheduler cooperativo, dual-blink.**
  - `rugus-hal-stm32f7::rcc` — HSE 25 MHz → PLL 216 MHz (VOSRDY fix).
  - `rugus-hal-stm32f7::cache` — I/D-cache M7.
  - `rugus-hal-stm32f7::fmc` — SDRAM 16 MB @ 0xC000_0000 (init + verify).
  - `rugus-core::heap` — `linked_list_allocator` sobre región configurable.
  - `rugus-core::sched` — cooperativo round-robin, 4 tareas, 3 bandas de prioridad.
  - `rugus-arch-cortex-m::switch` — PendSV handler ASM + bootstrap primera tarea.
  - Ejemplo `examples/dual-blink-stm32f769-disco` — LD1/LD2 en paralelo vía scheduler.
  - Scripts `tools/verify-blink-stm32f769-disco.sh` y `tools/verify-dual-blink-stm32f769-disco.sh`.

### Validated

- **G1 en HW real (STM32F769I-DISCO, probe-rs).** `dual-blink-stm32f769-disco`:
  SDRAM OK @ 0xC000_0000, heap en SDRAM, tasks A/B alternan por RTT sin HardFault;
  verify script **10/10 PASS** (2026-05-25, post PR #16).

### Fixed

- `rugus-hal-stm32f7::fmc` — mux FMC pins vía `GPIOx::ptr()` (AF12); `SDCR1` RBURST/RPIPE;
  deshabilitar FMC NOR bank1; init SDRAM antes de D-cache en dual-blink (PR #16).

---

---

## [0.1.0] — 2026-05-24 — Génesis G0

Primer release del workspace de Rugus. Establece la estructura
multi-arquitectura y entrega el primer ejemplo en HW real.

### Added

- **Workspace Cargo** con 5 crates publicables a futuro:
  - `rugus-core` — arch-agnostic; trait `Arch`, scheduler stub, syscall ABI v0.1, `Errno`.
  - `rugus-arch-cortex-m` — impl `Arch` para ARMv7-M / v7E-M / v8-M (stub en G0; real en G1).
  - `rugus-hal` — solo traits, `#![forbid(unsafe_code)]`: `GpioPin`, `SerialPort`.
  - `rugus-hal-stm32f7` — impl HAL STM32F7 family (gpio mínimo, features f769/f779).
  - `rugus-runtime` — panic-probe + defmt-rtt + entry macro re-export para targets Cortex-M.
- **Ejemplo** `examples/blink-stm32f769-disco/` — binario standalone que
  parpadea LD1 (PJ13) y emite logs `defmt` por SWD/RTT.
- **Documentación** densa en `docs/`: `ARCHITECTURE`, `ROADMAP`
  (G0..G7 + G∞), `PORTING`, `HAL_TRAITS`, `SECURITY_MODEL`,
  `SYSCALL_ABI`, `INVARIANTS`.
- **Memoria de agente** versionada en `docs/agent-memory/` para que
  cualquier asistente IA arranque con contexto.
- **Infra**: dual licensing MIT/Apache-2.0, `CONTRIBUTING.md`,
  `rustfmt.toml`, `.gitattributes`, CI (`fmt + clippy + build dev/release + doc`),
  `SECURITY.md`, `CODE_OF_CONDUCT.md`.
- **`AGENT_LOG.md`** bitácora cronológica de sesiones IA.
- Posicionamiento RTOS↔OS explícito en README y ARCHITECTURE: Rugus
  cambia de personalidad según el chip (RTOS en MCUs, OS general-purpose
  en SoCs) usando un único codebase via trait `Arch`.

### Specification

- **ABI Version**: `0x0001` (v0.1, borrador). Estabilización en G2.

### Known limitations

- `rugus-arch-cortex-m::switch_context` es no-op stub. Implementación
  real (PendSV + naked ASM en ITCM) llega en G1.
- Solo backend Cortex-M soportado. AVR/RISC-V/Cortex-A planificados.
- Sin scheduler, sin MPU, sin red, sin TLS — todo eso es trabajo G1-G4.
- `rugus-hal-stm32f7` solo expone GPIO; el resto de drivers (RCC, FMC,
  LTDC, ETH, CRYP, JPEG) llegan por fase según se necesiten.

[Unreleased]: https://github.com/guillo93/Rugus/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/guillo93/Rugus/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/guillo93/Rugus/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/guillo93/Rugus/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/guillo93/Rugus/releases/tag/v0.1.0
