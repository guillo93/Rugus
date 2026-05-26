# Roadmap

Estimaciones a **ritmo fines-de-semana** (~8 h/sem). En dedicación full-time
divídelo por 4. Cada hito (G*) implica al menos un ejemplo en HW funcional.

## G0 — Génesis *(2-3 sem)*

**Entregable:** workspace compila, `examples/blink-stm32f769-disco` flashea
y parpadea LD1 con logs `defmt`.

- [x] Workspace Cargo, 5 crates, dependencias centralizadas.
- [x] Traits `Arch` y HAL esqueleto.
- [x] Crate `rugus-arch-cortex-m` stub.
- [x] Crate `rugus-hal-stm32f7` con driver GPIO mínimo.
- [x] Ejemplo blink con `memory.x` + `build.rs` correcto para F769NIH6.
- [x] `cargo build --workspace` verde en CI.
- [x] **`cargo run` flashea y parpadea en placa real.** *(validado 2026-05-24
      sobre STM32F769I-DISCO, STLink V2-1, probe-rs 0.31.0)*

✅ **G0 cerrado.** Próximo: G1.

## G1 — Clocks + heap + scheduler cooperativo *(6-8 sem)*

**Entregable:** segunda tarea corre en paralelo a la principal en Cortex-M7.

- [x] `rugus-hal-stm32f7::rcc`: HSE 25 MHz → PLL → SYSCLK 216 MHz, AHB/APB.
- [x] Activar I/D-Cache del M7 con barriers.
- [x] `rugus-hal-stm32f7::fmc`: SDRAM 16 MB inicializada y verificada. *(PR #16: GPIO via `GPIOx::ptr()`, 10/10 verify dual-blink)*
- [x] `rugus-core::heap`: linked-list allocator sobre región configurable.
- [x] `rugus-core::sched` cooperativo round-robin, max 4 tareas.
- [x] `rugus-arch-cortex-m::switch`: PendSV ASM.
- [x] Ejemplo `dual-blink-stm32f769-disco` con dos tareas.

✅ **G1 cerrado** (2026-05-25). **Próximo: G2.**

## G2 — MPU + dominios + syscalls *(8-10 sem)*

**Primer paso:** `rugus-arch-cortex-m::mpu` — configurar 8 regiones MPU (priv/user) antes de syscalls y sandbox.

**Entregable:** apps en modo usuario; faults reportan dominio + PC.

- [x] `rugus-arch-cortex-m::mpu`: 8 regiones, política priv/user.
- [x] `rugus-core::syscall`: SVC handler, dispatch por ID, ABI v0.1.
- [x] HardFault/MemManage/BusFault/UsageFault con report.
- [x] Política "app que faulta → kernel mata tarea, no panic global".
- [x] Ejemplo `app-sandbox-stm32f769-disco` con app userland que faulta
      controladamente.

✅ **G2 cerrado** (2026-05-25). **Próximo: G3.**

## G3 — Segundo chip Cortex-M *(4-6 sem)*

**Entregable:** `examples/blink-stm32f407g-disco` parpadea LD4 (PD12) con logs
`defmt` RTT. Demuestra que la HAL es portable más allá del F769.

Placa de referencia: **STM32F407G-DISC1** (STM32F407VGT6, HSE 8 MHz, ST-Link
onboard). Documentación: [`docs/boards/stm32f407g-disco.md`](boards/stm32f407g-disco.md).

Tier mínimo futuro (post-G3, sin MPU): STM32F103 Blue Pill — ver
[`docs/boards/stm32f103c8-bluepill.md`](boards/stm32f103c8-bluepill.md).

- [x] `rugus-hal-stm32f4`: GPIO + RCC 168 MHz para F407 Discovery.
- [x] `examples/blink-stm32f407g-disco` verificado en HW.
- [x] `examples/dual-blink-stm32f407g-disco` — scheduler cooperativo, heap SRAM interna.
- [x] `examples/app-sandbox-stm32f407g-disco` — MPU + syscalls en Cortex-M4 (opcional G3).
- [x] Refactor mínimo en `rugus-arch-cortex-m` si M4 expone gaps (ninguno requerido).
- [x] CI matrix: `thumbv7em-none-eabihf` ya cubre Cortex-M4F (mismo target que M7).

✅ **G3 cerrado** (2026-05-25). **Próximo: G4 o F103 downscale** (ver AGENT_LOG).

## G4 — Red + TLS + crypto *(8-10 sem)*

**Entregable:** un ejemplo descarga vía HTTPS contra un servidor LAN.

- [x] `rugus-hal-stm32f7::eth` (ETH MAC + PHY LAN8742) — link + smoltcp + ETH IRQ wake.
- [x] Crate `rugus-net` envolviendo `smoltcp` — static/DHCP helpers + TCP IO adapter.
- [x] Crate `rugus-tls` envolviendo `embedded-tls`.
- [x] Crate `rugus-crypto` — backend software (SHA-256, CSPRNG); CRYP F769 documentado como futuro.
- [x] Ejemplo `https-get-stm32f769-disco`.
- [x] Ejemplo `eth-link-stm32f769-disco` (link + IPv4 estático).

✅ **G4 cerrado** (2026-05-25). **Próximo: G5 o F103 downscale.**

## G5 — Primera arch no-Cortex-M *(12-16 sem)*

**Entregable:** Cortex-A o RISC-V parpadeando.

- [ ] `rugus-arch-cortex-a` (Cortex-A53 / Pi 4 en EL1).
- [ ] MMU básica (identity-map por ahora).
- [ ] Implementaciones de Arch trait completas.
- [ ] `rugus-hal-bcm2711` mínimo.
- [ ] Ejemplo `blink-rpi4`.

(Alternativa: `rugus-arch-riscv` + `rugus-hal-esp32c3`.)

## G6 — Boot verificado + OTA *(6-8 sem por chip)*

**Entregable:** updates remotos seguros con rollback automático.

- [ ] Bootloader Ed25519 reusable (`crates/rugus-bootloader/`).
- [ ] Layout dual-bank parametrizable.
- [ ] HTTP/TLS OTA pull, verify, swap, watchdog.

## G7 — IA embebida opcional *(scope por chip)*

**Entregable:** crate `rugus-ai` con backends por chip.

- [ ] Trait `Inference` arch-agnóstico.
- [ ] Backend TFLite Micro pure-Rust (o equivalente) para Cortex-M7.
- [ ] Backend ONNX simplificado para Cortex-A.
- [ ] Backend trivial PWM/lookup para AVR (sin IA real, demo de fallback).

## G∞ — OS sofisticado

Apps nativas, IPC rico, sistema de paquetes binario, shell on-device,
file system, drivers de dispositivos externos (USB host, SDIO, NVMe en
Cortex-A). Llega cuando llegue. Sin promesas de fecha.

---

## Métricas objetivo (post-G2)

| Métrica | Cortex-M7 @ 216 MHz | Cortex-A53 @ 1.5 GHz |
|---------|---------------------|----------------------|
| Boot a `Arch::init` complete | < 200 ms | < 500 ms |
| Latencia syscall promedio | < 5 µs | < 1 µs |
| Latencia IRQ → handler | < 2 µs | < 500 ns |
| Context switch | < 3 µs | < 1 µs |
