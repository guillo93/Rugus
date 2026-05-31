# Changelog

Todos los cambios notables de este proyecto se documentarán en este archivo.

El formato sigue [Keep a Changelog](https://keepachangelog.com/es-ES/1.1.0/)
y este proyecto se adhiere a [Semantic Versioning](https://semver.org/lang/es/).

Mientras la versión sea pre-`1.0`, **breaking changes son permitidos entre
minor versions** (`0.1.x` → `0.2.0` puede romper API). A partir de `1.0`,
SemVer estricto.

## [Unreleased]

### Changed (robustez)

- **RX del bus de módulos USART2 (HM-20/BLE) por interrupción + ring buffer** —
  la RX de USART2 era polled sobre el registro de 1 byte, igual que lo estaba
  USART1 antes de su fix: el descubrimiento IDENTIFY que llega por el puente BLE
  podía perder bytes cuando la tarea CLI cooperativa tardaba en sondear (p. ej.
  el heartbeat a mitad de una rebanada de retardo). Ahora un ISR `USART2`
  (RXNEIE) drena cada byte a un ring SPSC lock-free de 256 B, igual que USART1.
  La RX opera en dos modos sobre el mismo periférico: **polled** durante el init
  AT del HM-20 (boot single-thread, fiable) y **por interrupción** en runtime
  (`Usart2::enable_rx_irq` se llama tras el init); `try_read_byte` conmuta según
  el modo, así que el driver `hm20` y `poll_identify_usart2` usan la misma API.
  `set_baud` reasevera `RXNEIE` porque reescribe CR1. Validado en HW: arranque,
  IDENTIFY por USART1 e init HM-20 `ready` sin regresión tras el cambio.

### Fixed (cliente host)

- **`parse_signature` tolera el eco de la shell** — `rush` hace eco de los bytes
  del request antes de emitir la firma y sin salto de línea, así que la respuesta
  llegaba como `IDENTIFYRUGUS;tier=…`. El parser exigía el prefijo `RUGUS;` al
  inicio de la línea y rechazaba el dispositivo (`NotRugus`), de modo que la
  auto-detección de `rugus` no veía la placa pese a responder correctamente.
  Ahora localiza el prefijo en cualquier posición y descarta el eco previo.
  Validado en HW: `rugus --list` detecta el F103 vivo en `/dev/ttyUSB0`.

### Changed (cliente host)

- **Binario del cliente renombrado a `rugus`** — el ejecutable se invoca con la
  palabra `rugus` (antes `rugus-cli`).
- **BLE opcional tras la feature `ble` (off por defecto)** — `btleplug` arrastra
  `libdbus-sys` (requiere `libdbus-1-dev`/`dbus-devel`). El build por defecto es
  solo serie; recompila con `--features ble` para BLE. Sin la feature se enlaza
  un stub (`ble_stub.rs`) que no escanea y explica cómo habilitarlo.
- **`serialport` sin default-features** — evita `libudev-sys`
  (`libudev-dev`/`systemd-devel`) en Linux; la enumeración por nombre de puerto
  sigue funcionando. El build de `rugus` ahora compila sin dependencias de sistema.

### Fixed (robustez)

- **RX de la consola USART1 (F103) por interrupción + ring buffer (raíz)** — la
  recepción era polled sobre el registro RX de 1 byte sin FIFO, así que en
  ráfaga (a 115200 un byte llega cada ~87 µs y el scheduler cooperativo podía no
  sondear a tiempo) se perdían bytes: comandos como `ward` solo veían `w`. Ahora
  un ISR `USART1` (RXNEIE) drena cada byte a un ring buffer SPSC lock-free de
  256 B; `try_read_byte` lo consume desde la tarea CLI. Se cuentan los descartes
  (`rx_overruns`) por ring lleno u overrun HW. Validado en HW: ráfagas de
  `ecosystem`/`orbit`/`ward` enviadas sin pacing eco-completas y ejecutan
  correctamente. (USART2/HM-20 sigue polled; su tráfico AT es tolerante a
  tiempos y se sondea fuera del scheduler.)

### Changed

- **Allocator global fuera del TCB mínimo (feature `alloc`)** — `rugus-core`
  exponía siempre `#[global_allocator]` (`linked_list_allocator`), obligando a
  toda personalidad —incluida la lite/F103 sin heap— a enlazarlo y comprometer
  un allocator global. Ahora el módulo `heap` y la dependencia van tras la
  feature `alloc` (off por defecto); solo las placas con región de heap
  (full/F4/F7 y los demos con `extern crate alloc`) la activan. El TCB queda más
  pequeño y agnóstico de heap. Eliminada además la dependencia `heapless`, que
  no se usaba en `rugus-core`.
- **Layout de la MPU parametrizado por placa (`MpuLayout`)** — `mpu::init` y
  `platform_init` tenían el mapa de memoria del STM32F769 hardcodeado (SDRAM
  16 M, SRAM 512 K, flash 2 M), de modo que el ejemplo del F407 programaba
  regiones MPU con bases/tamaños de otra placa (SDRAM externa inexistente, RAM
  sobredimensionada). Ahora `platform_init(cp, &MpuLayout)` recibe el mapa de la
  placa; se proveen `MpuLayout::STM32F769` y `MpuLayout::STM32F407` (este último
  sin región SDRAM y con flash 1 M). La región SDRAM se deshabilita cuando
  `sdram_size == 0`. El F103 (sin MPU) no usa esta ruta. Build+clippy limpios en
  F407/F769; validación en HW pendiente de placa F4/F7 conectada.

### Security

- **Invariante del sandbox MPU forzada en el spawn (raíz)** — `spawn_user` no
  validaba que el stack de una tarea userland fuera apto para su región MPU
  dedicada (App-RW). ARMv7-M exige que esa región sea potencia de 2 y esté
  alineada a su tamaño; con un stack que no lo cumpla, el remapeo redondeaba la
  región y **cubría RAM del kernel adyacente**, dando a la app acceso de
  escritura fuera de su sandbox. Ahora `spawn_user` rechaza en origen con
  `SpawnError::UnalignedUserStack` cualquier stack que no sea potencia de 2
  (≥32 B) y alineado a su tamaño. Las tareas privilegiadas no se ven afectadas
  (no obtienen región propia). El F103 (Cortex-M3, sin MPU) solo usa tareas
  privilegiadas, así que su arranque es idéntico — verificado en HW.
- **Contrato de validación de punteros de syscall documentado** — `syscall::dispatch`
  recibe `args` no confiables del frame de una tarea potencialmente userland.
  Se documenta el contrato CRÍTICO: todo syscall futuro que reciba puntero/longitud
  debe validar el rango completo `[ptr, ptr+len)` contra la región MPU de la tarea
  llamante (sin overflow, fuera de RAM kernel/periféricos/flash) y rechazar con
  `Errno::Efault`, ya que en modo privilegiado el MPU no protege al kernel de sí
  mismo. Los syscalls actuales (`YieldNow`/`TaskId`) no toman punteros.

### Fixed

- **Política de fault del kernel lite (raíz)** — en el appliance F103 el hook de
  fault del kernel nunca se registraba (`set_fault_hook`) ni se habilitaban los
  handlers dedicados (`enable_fault_handlers`), así que una tarea que faulteaba
  caía en `fault_panic` → **panic global** que tumbaba todo el dispositivo, en
  vez de matar solo la tarea culpable. Ahora `main` habilita los handlers
  BusFault/UsageFault/HardFault y registra un hook que loguea el `FaultReport`
  (kind/dominio/pc/task) y llama a `kill_current_and_resume`: la tarea faultante
  muere y el scheduler reanuda la siguiente, manteniendo vivos heartbeat y
  watchdog. Validado en HW: `UsageFault` inyectado en la tarea CLI → kill+resume
  sin reset del dispositivo. También se registran los hooks de scheduler
  (`current_task_id`/`current_domain`) para que el reporte de fault sea preciso.
- **Código muerto en el TCB** — `Scheduler::kill_current_and_resume` tenía logs
  bajo `#[cfg(feature = "log")]`, una feature inexistente en `rugus-core` → el
  kill era silencioso. Eliminado; la observabilidad del fault vive en el hook de
  la plataforma (capa app), manteniendo el TCB mínimo y agnóstico del transporte.
- **Lint en `rugus-arch-cortex-m`** — `fault_panic` dejaba `kind/domain/pc` sin
  usar al compilar sin la feature `defmt`, rompiendo `clippy -D warnings` en el
  target M3. Corregido.
- **HM-20 BLE en F103 (raíz)** — el init forzaba el módulo a 115200 con sintaxis
  AT incoherente (`AT+NAME=` con `=` vs `AT+BAUD4` sin `=`), dejando MCU y módulo
  en baudios distintos sobre un HSI de 8 MHz sin calibrar → enlace BLE mudo.
  Ahora `hm20::init` **adopta el baud nativo del módulo** (9600 fábrica, error
  <0.1 % desde HSI), nombre/`NOTI1` son best-effort, y el sondeo de arranque
  alimenta el watchdog (`init_with_kick`). Eliminada la ruta de cambio de baud y
  el `set_baud` huérfano.
- **Sintaxis AT del HM-20** — corregida contra el datasheet oficial: los comandos
  van **sin terminador `\r\n`** y el nombre **sin `=`** (`AT+NAMERUGUS`, no
  `AT+NAME=...\r\n`); el código de baud 115200 es `AT+BAUD7` (no `AT+BAUD4`).
  Afecta driver `hm20`, `sonar`/lectura de módulo y `tools/provision-hm20.sh`.

## [0.7.0] — 2026-05-30 — Rugus lite appliance + rush + eco HM-20

Release del tier **lite** como appliance completo: shell `rush`, cliente host
`rugus-cli`, primer `.eco` (HM-20 BLE) y fail-safe reversible.

### Added

- **`rush`** — shell on-device (rename desde `rugus-cli` embebido): léxico v1,
  banners ANSI, protocolo `IDENTIFY`, comando `orbit` con ayuda.
- **`rugus-cli` (host)** — cliente PC (`rugus-proto` + serie/BLE auto-detect +
  TUI ratatui). Crates host excluidos del workspace embebido; CI job `host`.
- **Appliance F103 fases 1–6** — `examples/appliance-stm32f103c8-bluepill`:
  USART1 consola, I2C/SD/GPIO, scheduler cooperativo, USART2 módulos, WDT.
- **Primer `.eco`** — `examples/eco/hm20-ble.eco` (HM-10/HM-20 BLE UART
  transparente); driver `rugus-hal-stm32f1::hm20` (AT `+NAME=RUGUS`, 115200).
- **Docs** — `RUGUS-ECOSYSTEM.md`, `RUGUS-CLI-HOST.md`, wiring HM-20 en
  `RUGUS-LITE-APPLIANCE.md`, registry stub HM-20.

### Changed

- **`anchor off` / `anchor release`** — `sys_failsafe(1)` desactiva fail-safe
  (GPIO y syscalls vuelven a operar); ayuda en `orbit`.

### Fixed

- **`hm20` init** — poll no bloqueante en USART2 cuando no hay módulo BLE
  conectado (boot no cuelga).

### Validated

- **`verify-appliance-stm32f103c8-bluepill.sh` → 9/9 PASS** (ST-Link
  `0483:3748:…`, BOOT0=GND, 2026-05-30).

---

## [Unreleased — histórico G4]

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
