# Porting Guide

Cómo añadir una nueva arquitectura (ISA) o un nuevo chip a Rugus.

## Caso A — Nueva arquitectura

Implica escribir un nuevo backend del trait `Arch` definido en
`rugus-core::arch::Arch`.

### Pasos

1. **Verifica que la arch tiene lo mínimo.** Rugus asume:
   - ISA con stack pointer e instrucciones de salto indirecto.
   - Algún mecanismo de IRQ y de cambio de contexto (cooperativo basta).
   - Opcionalmente: MPU/MMU/PMP para aislamiento. Sin esto se puede correr
     pero los dominios userland son "best-effort" (apps confían en el kernel
     por ausencia de hardware enforcement).

2. **Crea el crate** `crates/rugus-arch-<isa>/`:
   ```
   crates/rugus-arch-<isa>/
   ├── Cargo.toml
   └── src/
       ├── lib.rs         export pub struct Arch + impl rugus_core::Arch
       ├── context.rs     struct Context (regs, sp), switch ASM
       ├── irq.rs         enter/exit critical
       ├── mpu.rs         (opt) si la arch tiene MPU/MMU/PMP
       └── time.rs        SysTick o equivalente
   ```

3. **Implementa el trait completo.** Si una función no aplica (`HAS_MPU =
   false` y se llama a configurar región), devolver `Err(ArchError::Unsupported)`.

4. **Añade el target** a `rust-toolchain.toml` y a la matrix de
   `.github/workflows/ci.yml`.

5. **Demuestra con un ejemplo.** Crea `examples/blink-<board>/` con la
   placa más barata/típica de la nueva arch. **Sin ejemplo en HW, no se
   anuncia soporte en el README.**

6. **Documenta peculiaridades** en `docs/arch-<isa>.md` (puede ser archivo
   nuevo): convenciones de calling, gotchas conocidos, herramientas extra
   requeridas (e.g. `avr-gcc` para AVR linking, OpenSBI para RISC-V).

7. **Actualiza** `AGENT_LOG.md` con la entrada de portado y `README.md` con
   la nueva fila en la tabla de arquitecturas.

## Caso B — Nuevo chip (misma arch existente)

Implica implementar los traits de `rugus-hal` para un chip nuevo cuya
arch ya está soportada.

### Pasos

1. **Crea el crate** `crates/rugus-hal-<chip-family>/`:
   ```
   crates/rugus-hal-<chip-family>/
   ├── Cargo.toml          dep: rugus-hal, PAC svd2rust del chip
   └── src/
       ├── lib.rs
       ├── gpio.rs         impl rugus_hal::GpioPin
       ├── serial.rs       impl rugus_hal::SerialPort
       ├── rcc.rs          clocks
       └── ...             (drivers que el chip ofrece)
   ```

2. **Implementa primero el mínimo** que justifique el ejemplo: GPIO + clocks
   suelen bastar para un blink. Los demás drivers se añaden cuando un
   ejemplo o un consumidor los pida.

3. **Define `memory.x`** en cada ejemplo que use el chip. NO ponerlo en el
   crate HAL: cada placa puede tener configuración FMC/SDRAM/QSPI distinta.

4. **Feature flags por chip de la familia.** Si la familia tiene variantes
   (STM32F767/769/779), un Cargo `feature` por variante; el consumidor
   activa la suya.

5. **Demuestra con `examples/<demo>-<board>/`** antes de mergear.

## Caso C — Nuevo driver para un chip ya soportado

Implica añadir una capacidad nueva (e.g. driver Ethernet en
`rugus-hal-stm32f7`).

1. Si la capacidad **encaja en un trait existente** de `rugus-hal`, impleméntalo.
2. Si **no encaja** (capacidad nueva tipo "DMA2D" o "JPEG HW"), evalúa:
   - ¿Está disponible en otros chips del ecosistema? → propón añadir trait
     a `rugus-hal` en un PR aparte primero.
   - ¿Es exclusiva de este chip? → expón el driver como API pública del
     `rugus-hal-<chip>` sin meterlo en el trait genérico. Documenta que es
     "chip-specific extension".

## Lista de comprobación pre-merge

Para cualquier portado:

- [ ] El ejemplo correspondiente parpadea (o equivalente verificable) en HW.
- [ ] CI pasa con el nuevo target en matrix.
- [ ] `docs/ROADMAP.md` marca el hito apropiado.
- [ ] `README.md` actualizado con la fila de la tabla de arquitecturas/chips.
- [ ] `AGENT_LOG.md` con entrada de la sesión.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` pasa.
- [ ] Si el chip nuevo tiene HW interesante (crypto, JPEG, neural accel),
      mencionado en `docs/HAL_TRAITS.md` como capacidad opcional.
