<!-- Gracias por contribuir a Rugus. Antes de pedir review, completa esta plantilla. -->

## Resumen

<!-- 1-3 frases. Qué cambia y por qué. -->

## Tipo de cambio

<!-- Marca todos los que apliquen -->

- [ ] `feat`     — nueva capacidad o feature
- [ ] `fix`      — corrección de bug
- [ ] `refactor` — cambio sin alterar comportamiento externo
- [ ] `perf`     — mejora de rendimiento
- [ ] `sec`      — fix de seguridad (coordinar con SECURITY.md antes)
- [ ] `docs`     — solo documentación
- [ ] `chore`    — tareas auxiliares (deps, CI, tooling)
- [ ] `port`     — nueva arquitectura o nuevo chip (lee [`docs/PORTING.md`](../docs/PORTING.md))

## Issue relacionada

<!-- Closes #123  /  Refs #456  /  N/A -->

## Cambios

<!-- Lista en bullets de qué se tocó. Ejemplo:
- `crates/rugus-hal-stm32f7/src/rcc.rs` nuevo: configura PLL a 216 MHz.
- `Cargo.toml` workspace: bump de `cortex-m` a 0.7.8.
- `docs/MEMORY_MAP.md`: añadida nota sobre cache regions.
-->

## Test plan

<!-- Cómo verificaste que funciona. Sé concreto.
- [ ] `cargo build --workspace --target thumbv7em-none-eabihf` pasa
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` pasa
- [ ] `cargo fmt --all -- --check` pasa
- [ ] On-target en STM32F769I-DISCO: <descripción de qué se verificó>
- [ ] QEMU (si aplica): <comando + resultado>
-->

## Checklist (autocontrol)

- [ ] Sigo la política de `unsafe` por capa documentada en `CONTRIBUTING.md`.
- [ ] Si toco `rugus-core` syscall/sched/arch, actualizo el doc correspondiente en el **mismo PR**.
- [ ] Si añado un cambio breaking, lo documento en `CHANGELOG.md` § `[Unreleased]`.
- [ ] Si añado dependencias, son pure-Rust no_std (sin FFI a C).
- [ ] CI verde (`fmt`, `clippy`, `build dev/release`, `cargo doc`).

## Notas para el revisor

<!-- Lo que sea no obvio mirando el diff. Decisiones de diseño, trade-offs, links a manuales del chip. Si no hay nada, borrar esta sección. -->

---

<sub>Al abrir este PR confirmo que mi contribución se publica bajo los términos
de licencia del proyecto (MIT OR Apache-2.0).</sub>
