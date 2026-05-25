# Changelog

Todos los cambios notables de este proyecto se documentarán en este archivo.

El formato sigue [Keep a Changelog](https://keepachangelog.com/es-ES/1.1.0/)
y este proyecto se adhiere a [Semantic Versioning](https://semver.org/lang/es/).

Mientras la versión sea pre-`1.0`, **breaking changes son permitidos entre
minor versions** (`0.1.x` → `0.2.0` puede romper API). A partir de `1.0`,
SemVer estricto.

## [Unreleased]

### Added

- **G1 — clocks, SDRAM/FMC, heap, scheduler cooperativo, dual-blink.**
  - `rugus-hal-stm32f7::rcc` — HSE 25 MHz → PLL 216 MHz (VOSRDY fix).
  - `rugus-hal-stm32f7::cache` — I/D-cache M7.
  - `rugus-hal-stm32f7::fmc` — SDRAM 16 MB @ 0xC000_0000 (init + verify).
  - `rugus-core::heap` — `linked_list_allocator` sobre región configurable.
  - `rugus-core::sched` — cooperativo round-robin, 4 tareas, 3 bandas de prioridad.
  - `rugus-arch-cortex-m::switch` — PendSV handler ASM + bootstrap primera tarea.
  - Ejemplo `examples/dual-blink-stm32f769-disco` — LD1/LD2 en paralelo vía scheduler.
  - Script `tools/verify-dual-blink-stm32f769-disco.sh`.

### Validated

- **G1 en HW real (STM32F769I-DISCO, probe-rs).** `blink-stm32f769-disco` sigue verde;
  `dual-blink-stm32f769-disco` alterna task A/B por RTT sin HardFault.

### Known limitations

- `fmc::init` verify read/write falla en placa actual (init sequence presente;
  heap cae a SRAM interna). MPU SDRAM completa en G2.

### Fixed

- `examples/blink-stm32f769-disco/build.rs` añadido: copia `memory.x` a
  `OUT_DIR` y lo expone al search path del linker. Sin esto, `cargo run`
  fallaba con `cannot find linker script memory.x` (setup canónico de
  cortex-m-rt que faltó en el commit génesis).

### Validated

- **G0 cerrado en HW real.** Firmware `blink-stm32f769-disco` flasheado en
  STM32F769I-DISCO vía STLink V2-1 (`probe-rs 0.31.0`); LD1 (PJ13) parpadea
  ~1 Hz; logs `defmt` por SWD/RTT visibles. Validación 2026-05-24.

### Added

- Issue templates (`bug_report`, `feature_request`, `port_request`) con
  campos estructurados YAML; `config.yml` redirige preguntas a Discussions
  y reportes de seguridad a GitHub Security Advisories.
- `PULL_REQUEST_TEMPLATE.md` con checklist y test plan.
- `.github/dependabot.yml` para auto-update semanal de Cargo deps y
  mensual de GitHub Actions (con grouping de patches/minors).
- Badges en README: CI status, licenses, MSRV, no_std, Discussions count.
- GitHub Discussions habilitadas para preguntas y diseño no-issue.
- Branch protection en `main` con `enforce_admins` activo (owner incluido
  en las reglas, sin bypass).

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

[Unreleased]: https://github.com/guillo93/Rugus/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/guillo93/Rugus/releases/tag/v0.1.0
