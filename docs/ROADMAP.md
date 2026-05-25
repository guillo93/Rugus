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

- [ ] `rugus-hal-stm32f7::rcc`: HSE 25 MHz → PLL → SYSCLK 216 MHz, AHB/APB.
- [ ] Activar I/D-Cache del M7 con barriers.
- [ ] `rugus-hal-stm32f7::fmc`: SDRAM 16 MB inicializada y verificada.
- [ ] `rugus-core::heap`: linked-list allocator sobre región configurable.
- [ ] `rugus-core::sched` cooperativo round-robin, max 4 tareas.
- [ ] `rugus-arch-cortex-m::switch`: PendSV ASM en `.itcm`.
- [ ] Ejemplo `dual-blink-stm32f769-disco` con dos tareas.

## G2 — MPU + dominios + syscalls *(8-10 sem)*

**Entregable:** apps en modo usuario; faults reportan dominio + PC.

- [ ] `rugus-arch-cortex-m::mpu`: 8 regiones, política priv/user.
- [ ] `rugus-core::syscall`: SVC handler, dispatch por ID, ABI v0.1.
- [ ] HardFault/MemManage/BusFault/UsageFault con report.
- [ ] Política "app que faulta → kernel mata tarea, no panic global".
- [ ] Ejemplo `app-sandbox-stm32f769-disco` con app userland que faulta
      controladamente.

## G3 — Segundo chip Cortex-M *(4-6 sem)*

**Entregable:** `examples/blink-rp2040-pico` o `examples/blink-stm32f411-bp`
parpadea. Demuestra que la HAL es realmente portable.

- [ ] `rugus-hal-rp2040` o `rugus-hal-stm32f4` (elección según hardware
      disponible).
- [ ] Refactor mínimo en `rugus-arch-cortex-m` si M0+/M4 expone gaps.
- [ ] CI matrix añade nuevo target.

## G4 — Red + TLS + crypto *(8-10 sem)*

**Entregable:** un ejemplo descarga vía HTTPS contra un servidor LAN.

- [ ] `rugus-hal-stm32f7::eth` (ETH MAC + PHY LAN8742).
- [ ] Crate `rugus-net` envolviendo `smoltcp`.
- [ ] Crate `rugus-tls` envolviendo `embedded-tls`.
- [ ] Crate `rugus-crypto` con backend HW por chip cuando disponible (CRYP
      del F769, HMAC del RP2040, etc.).
- [ ] Ejemplo `https-get-stm32f769-disco`.

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
