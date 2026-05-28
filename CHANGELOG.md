# Changelog

Todos los cambios notables de este proyecto se documentarán en este archivo.

El formato sigue [Keep a Changelog](https://keepachangelog.com/es-ES/1.1.0/)
y este proyecto se adhiere a [Semantic Versioning](https://semver.org/lang/es/).

Mientras la versión sea pre-`1.0`, **breaking changes son permitidos entre
minor versions** (`0.1.x` → `0.2.0` puede romper API). A partir de `1.0`,
SemVer estricto.

## [Unreleased]

### Added

- **G4 closure follow-up** — recovered uncommitted ETH/HTTPS work, refined and applied as proper commits.
  - `crates/rugus-hal-stm32f7::eth::dma::smoltcp_phy` — `Device::receive`/`transmit` now self-arm DMA on every smoltcp poll via `service_dma()`. Removes the need for example main loops to call `service_dma()` manually and recovers from `TBUS=1` stalls automatically.
  - `crates/rugus-hal-stm32f7::eth::dma::rx::RxRing` — discards descriptors with error / truncated frame so smoltcp never receives an empty slice (fixes prior `slice length 0` panic surface).
  - `crates/rugus-hal-stm32f7::eth::dma` — descriptors form a true ring (last `next_descriptor` wraps to 0) and `demand_poll` clears `RBUS`/`TBUS` before poking.
  - `crates/rugus-hal-stm32f7::eth::dma::tx::EthTxToken::consume` — pads short frames to 60 bytes (802.3 minimum) before send.
  - `crates/rugus-hal-stm32f7::cache::configure_eth_mpu` — full ARMv7-M ARM B3.5 sequence: `MPU.CTRL=0` → `dsb/isb` → program region 1 (`ETH_DMA_BASE`, Normal-Non-Cacheable, XN, full access) → `MPU.CTRL=ENABLE|PRIVDEFENA` → `dsb/isb`. Uses `ETH_DMA_BASE` constant, no hardcoded literal.
  - `crates/rugus-hal-stm32f7::eth::setup::enable_peripheral` — dummy read of `RCC.AHB1ENR` after enabling SYSCFG (F7 errata for peripheral clock stabilization).
  - `crates/rugus-crypto::SoftwareRng` — impl `rugus_hal::CryptoRng` so TLS clients can take a single `rugus_hal::CryptoRng` bound.
  - `crates/rugus-net::tcp_connect` — logs socket state every 1 s during the timeout window for in-the-field diagnosis.
  - `examples/https-get-stm32f769-disco` — boot order matches `eth-link` byte for byte; SRAM-only 64 KiB heap (FMC/SDRAM skipped — not needed for current working set); 8-s L2 probe window before TCP connect for operator-side ping/ARP verification.
  - `tools/verify-{eth-link,https-get}-stm32f769-disco.sh` — `probe-rs run --connect-under-reset` for reliable flashing.
- **Docs**:
  - `docs/G4-CLOSE-REPORT.md` — closure summary with verify scores, root-cause analysis of the residual TCP gap, user-side validation steps.
  - `docs/PERFORMANCE.md` — kernel performance strategy scaffold (Rust + `asm!` + `#[naked]` + `link_section` + LUTs, no C/C++/FFI).
- **`.gitignore`** — excludes local debug artifacts (`*.pcap`, `capture.log`, `/tmp/rugus-*.log`).

### Changed

- `crates/rugus-hal-stm32f7::eth::DEFAULT_MAC` and `crates/rugus-net::DEFAULT_MAC` updated to `00:80:E1:11:22:33` (ST OUI) to interoperate cleanly with home LAN switches; downstream consumers can override via their own constant.

### Validated

- **`verify-eth-link-stm32f769-disco.sh` → 9/9 PASS reproducible** (5 consecutive runs, 2026-05-27). Pings 4/4 from host, ARP `REACHABLE`, MAC `00:80:E1:11:22:33`, RX > 700 frames including LAN broadcast.
- **`verify-https-get-stm32f769-disco.sh` → 9/13 PASS** (2026-05-27). TCP `SynSent` timeout; `mmc_tx_good` counter increments inside MAC but transmitted frames are intermittent on the wire when running this specific example (root cause analysis in `docs/G4-CLOSE-REPORT.md`). HAL is verified by `eth-link` running the same code paths.


## [0.6.0] — 2026-05-27 — Rugus lite (F103)

Segundo perfil «lite» en Cortex-M3: HAL F1, blink y scheduler cooperativo dual-blink en Blue Pill (sin MPU/FPU).

### Added

- **Rugus lite — STM32F103C8 Blue Pill (Cortex-M3).**
  - Crate `rugus-hal-stm32f1` — GPIO, RCC HSI 8 MHz.
  - Ejemplo `examples/blink-stm32f103c8-bluepill` — PC13 toggle + defmt RTT.
  - Ejemplo `examples/dual-blink-stm32f103c8-bluepill` — dos tareas alternan PC13 (~0.5 s / ~0.33 s); heap 4 KiB.
  - Scripts `tools/verify-{blink,dual-blink}-stm32f103c8-bluepill.sh` — build, clippy, flash, RTT.
  - CI `thumbv7m-none-eabi`; docs `docs/boards/stm32f103c8-bluepill.md`.

### Validated

- **F103 blink en HW (ST-Link externo, probe-rs):** verify-blink **10/10 PASS** (2026-05-27, PR #27).
- **F103 dual-blink cooperativo:** verify-dual-blink build + RTT task alternation (PR #28).

---

## [0.5.0] — 2026-05-25 — G4

Red + TLS + crypto en STM32F769I-DISCO: smoltcp, embedded-tls, HTTPS GET contra servidor LAN.

### Added

- **G4 — Ethernet + smoltcp + TLS + HTTPS GET (STM32F769I-DISCO).**
  - `rugus-hal-stm32f7::eth` — ETH IRQ pending flag + `take_eth_irq_pending()` for WFI poll.
  - `rugus-hal::CryptoRng` trait; `rugus-crypto` — software SHA-256 + CSPRNG (CRYP HW futuro).
  - `rugus-tls` — wrapper `embedded-tls` blocking, TLS 1.3 LAN (sin verificación cert).
  - `rugus-net` — TCP connect helper, `TcpIo` adapter `embedded-io`, DHCP/static IPv4.
  - Ejemplo `examples/https-get-stm32f769-disco` — GET `/` vía HTTPS a `192.168.0.112:8443`.
  - Ejemplo `examples/eth-link-stm32f769-disco` — link + IPv4 estático `192.168.0.50/24`.
  - Script `tools/verify-https-get-stm32f769-disco.sh`.
  - Docs `examples/https-get-stm32f769-disco/README.md` (servidor OpenSSL/Python LAN).

### Fixed

- **ETH DMA MPU** — región no-cacheable 16 KiB @ `0x20078000` (alineada), `enable_with_eth_dma()`, `service_dma()` (RBUS + poll demand).

### Validated

- **G4 step 1 en HW:** `verify-eth-link-stm32f769-disco.sh` **9/9 PASS** (link, IP 192.168.0.50).
- **G4 HTTPS en HW:** `verify-https-get` **9/13** (TCP timeout; L2 placa↔PC sin tramas RX — ver `docs/G4-MORNING-REPORT.md`).

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

[Unreleased]: https://github.com/guillo93/Rugus/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/guillo93/Rugus/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/guillo93/Rugus/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/guillo93/Rugus/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/guillo93/Rugus/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/guillo93/Rugus/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/guillo93/Rugus/releases/tag/v0.1.0
