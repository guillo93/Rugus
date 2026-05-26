# Project Context — Rugus

## Qué es

**Rugus** es un kernel / OS en **Rust puro `no_std`**, **multi-arquitectura
por diseño**, que crece poco a poco hasta convertirse en un sistema
operativo sofisticado. Empezando como `rugus-kernel` (mínimo viable
embebido), evolucionando a `rugus-os` (sistema completo con apps nativas,
red, IA opcional).

## Cómo nació

Originalmente concebido como kernel propio para la **STM32F769I-DISCO** del
proyecto smart-home del owner. En sesión del 2026-05-24 se decidió
**separarlo** del firmware del panel y darle vida propia como kernel
genérico. Razones del split:

- El kernel no debería estar acoplado a un chip.
- Vendrán otros consumidores (sensores IoT, otros paneles, terceros).
- El kernel y el panel tienen cadencias de release distintas.
- Posibilidad de publicar el kernel en crates.io como ecosistema propio.

Repo del primer consumidor (panel smart-home):
[`guillo93/Panel-smartH`](https://github.com/guillo93/Panel-smartH).

## Decisiones bloqueadas (no re-abrir)

| Decisión | Detalle |
|---|---|
| **Pure Rust `no_std`, cero FFI a C** | Sin LVGL, sin mbedtls, sin lwIP, sin FreeRTOS C-API. Stack equivalente pure-Rust o se escribe propio. |
| **Multi-arch por trait `Arch`** | `rugus-core` no depende de PAC ni de cortex-m; toda CPU pasa por el trait. |
| **HAL por traits** | `rugus-hal` separado del kernel. Permite ecosistema independiente. |
| **No promesa sin prueba** | Una arch o chip se documenta como soportado solo cuando hay ejemplo en HW que parpadea. |
| **Seguridad como pilar** | MPU/MMU/PMP obligatorios donde el chip los tenga; apps `#![forbid(unsafe_code)]`. |
| **Bootloader propio Ed25519 + OTA dual-bank** | Para chips con flash suficiente, post hito G6. |

## Arquitecturas y chips planificados

Estado en `README.md`, `docs/ROADMAP.md` y [`docs/boards/`](../boards/README.md).
Resumen:

| Arch | Chip ejemplar | Hito |
|------|---------------|------|
| Cortex-M7 (ARMv7E-M) | STM32F769NIH6 | G0 (actual) |
| Cortex-M4 (ARMv7E-M) | STM32F407VGT6 / Discovery | G3 (activo) |
| Cortex-M3 (ARMv7-M) | STM32F103C8T6 / Blue Pill | post-G3 (“Rugus lite”) |
| Cortex-M0+ (ARMv6-M) | RP2040 | G3 |
| Cortex-M33 (ARMv8-M Main) | nRF5340 / STM32L5 | G4 |
| AVR 8-bit | ATmega328P | exploratorio (sin alloc, sin MPU) |
| RISC-V RV32IMAC | ESP32-C3 | G5 |
| Cortex-A53 (ARMv8-A) | Raspberry Pi 4 | G5 |

## Convenciones de naming

- Repo: `Rugus` (PascalCase, GitHub).
- Crates: `rugus-<area>`, `rugus-arch-<isa>`, `rugus-hal-<chip-family>`.
- Ejemplos: `<demo>-<board>`, donde `<board>` identifica la placa concreta
  (e.g. `blink-stm32f769-disco`, no `blink-stm32f7`).
- Documentación: hitos numerados como `G0`, `G1`, `G2`… (G = Génesis +
  número). Evita confusión con "Phase" que ya se usa en consumidores.

## Owner

- Luis Guillermo Hernandez ([@guillo93](https://github.com/guillo93)).
- Email: `luiguihez93@gmail.com`.
- Trabaja en Windows 11 (PowerShell 7 disponible).
- Repo local: `C:\Users\luigu\OneDrive\Documents\Rugus\`.
- Proyectos relacionados activos:
  - `Panel-smartH` (consumidor #1, smart-home panel).
  - `Diagnos` / `Celador` (servidor vigilancia IA, Python — no relacionado
    técnicamente con Rugus salvo que el panel se conecta a él).
