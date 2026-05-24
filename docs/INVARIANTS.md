# Invariants

Lista de invariantes del sistema que **siempre** se mantienen, con su
mecanismo de verificación. Cada PR que pueda violar uno debe argumentar
en su descripción que el invariante sigue válido tras el cambio.

Documento vivo. Crece con cada hito. Hoy refleja el estado planeado
post-G2.

---

## I-1 — `rugus-core` no asigna en heap dinámico

**Forma:** ninguna ruta de `rugus-core` invoca `alloc::*`.

**Verificación:** `rugus-core/Cargo.toml` no declara `alloc` ni
`extern crate alloc`. CI grep que rechaza `Box::`, `Vec::with_capacity`,
`String::from` en `crates/rugus-core/src/**`.

**Por qué:** previsibilidad de memoria. Tareas, mensajes IPC y buffers de
IRQ viven en pools `heapless` reservados estáticamente. OOM en una app no
mata el kernel.

---

## I-2 — `rugus-hal` (traits) tiene `#![forbid(unsafe_code)]`

**Forma:** el crate de traits es 100 % seguro. Los impls (`rugus-hal-*`)
pueden tener `unsafe`.

**Verificación:** atributo en `crates/rugus-hal/src/lib.rs`. CI lo verifica
con `cargo geiger` o grep.

---

## I-3 — Context switch es determinista en latencia

**Forma:** `Arch::switch_context` ejecuta en < 3 µs (Cortex-M7 @ 216 MHz)
o < 1 µs (Cortex-A53 @ 1.5 GHz) en el peor caso.

**Verificación:** test on-target en `tests/latency-<board>.rs` que mide
ciclos con DWT (Cortex-M) o PMU (Cortex-A).

---

## I-4 — Punteros user-space siempre validados antes de uso

**Forma:** todo syscall handler que reciba un puntero usuario llama a
`arch.validate_user_ptr(ptr, len, perms)` antes de acceder. Nunca
`unsafe { *user_ptr }` en `core::syscall`.

**Verificación:** review obligatoria de PRs que toquen
`crates/rugus-core/src/syscall.rs`. Marca explícita en cada arm del match.

---

## I-5 — Secretos no salen del dominio kernel

**Forma:** el trait `SecretStore` nunca expone `get_key()`. Apps piden
operaciones (`sign`, `rng_fill`); las claves nunca atraviesan la frontera.

**Verificación:** trait diseñado sin método de extracción. PR que añada
`get_key` o equivalente rechazado por contradecir este invariante.

---

## I-6 — Boot solo continúa con firma válida (post G6)

**Forma:** bootloader verifica firma Ed25519 + SHA-256 del slot activo
antes de saltar. Ambos slots inválidos → modo recovery (espera OTA).

**Verificación:** test on-target con slot corrupto. Clave pública en
`.rodata` del bootloader, no actualizable por OTA.

---

## I-7 — OTA deja siempre el sistema arrancable (post G6)

**Forma:** tras cualquier paso de OTA, si la placa se reseteara, el
bootloader encuentra al menos un slot válido para arrancar.

**Verificación:** test on-target que simula corte de energía en cada
punto del flujo.

---

*Invariantes adicionales se documentarán cuando aparezcan. Eliminar uno
requiere justificación explícita y entrada en `AGENT_LOG.md`.*
