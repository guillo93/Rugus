# Rugus lite appliance — STM32F103 Blue Pill

Referencia del firmware **appliance** para el tier **lite** (F103C8).
Complementa [`RUGUS-KERNEL-VISION.md`](RUGUS-KERNEL-VISION.md) y
[`boards/stm32f103c8-bluepill.md`](boards/stm32f103c8-bluepill.md).

## Fases implementadas

| Fase | Contenido | Comandos CLI |
|------|-----------|--------------|
| 1 | UART `rush` @ 115200 | `cosmos`, `orbit`, `ecosystem`, `pulso`, `spark`, `mute`, `ripple` |
| 2 | I2C1 + mapa GPIO | `scout`, `moor` |
| 3 | SD SPI + RFN/AFR staging | `schema`, `scribe`, `seal`, `hatch` |
| 4 | Scheduler cooperativo | `coil` |
| 5 | USART2 módulos + seguridad | `nest`, `sonar`, `anchor`, `ward` |
| 6 | ML (documentación) | ver [`RUGUS-LITE-ML.md`](RUGUS-LITE-ML.md) |

## Pinout appliance

| Función | Periférico | Pines | Notas |
|---------|------------|-------|-------|
| `rush` (shell) | USART1 | PA9 TX, PA10 RX | Consola principal 115200 8N1 |
| Módulos (LoRa, HM-10) | USART2 | PA2 TX, PA3 RX | Bus serie 115200 (IDENTIFY) |
| Tarjeta SD | SPI1 | PA4 NSS, PA5 SCK, PA6 MISO, PA7 MOSI | Config `.rfn` / apps `.afr` |
| Sensores | I2C1 | PB6 SCL, PB7 SDA | Escaneo `scout` |
| LED onboard | GPIO | PC13 | Heartbeat consciente de actividad + GPIO CLI |
| Debug (dev) | RTT | SWD | `defmt`; no producción |

**BOOT0** = GND para flash/run normal.

## Arquitectura por capas

```
rush (ANSI, parser, léxico, IDENTIFY)
        ↓ syscall::lite::user
rugus-core/syscall/lite (hooks, sin hardware)
        ↓ hooks registrados
services (ejemplo) + rugus-hal-stm32f1
```

El parser y los banners **no** están en `rugus-core`. Ver regla de oro en
[`RUGUS-KERNEL-VISION.md`](RUGUS-KERNEL-VISION.md).

## Protocolo IDENTIFY

`rush` responde al descubrimiento de un host (cliente `rugus-cli` de escritorio)
por **cualquier** transporte serie. Dispara con:

- La línea `IDENTIFY\r\n`, **o**
- El byte de control `ENQ` (`0x05`).

Respuesta: **exactamente una línea**

```text
RUGUS;tier=lite;chip=f103;proto=1;shell=rush;cli=1.0.0\r\n
```

Cableado: respondido tanto en **USART1** (consola) como en **USART2** (bus de
módulos, p. ej. puente BLE HM-10). Es una respuesta barata, sin lógica pesada:
el kernel sigue serio; el host hace el resto. Especificación completa del
handshake y el flujo de auto-detección en [`RUGUS-CLI-HOST.md`](RUGUS-CLI-HOST.md).

## Léxico rush v1

| Comando | Syscall | Descripción |
|---------|---------|-------------|
| `cosmos` | `sys_info` | Versión, placa, personalidad |
| `orbit` | (local) | Ayuda con banner ANSI |
| `ecosystem` | `sys_status` | SD, módulos, tareas, failsafe |
| `moor` | `gpio_bind` | Asociar pin a rol lógico |
| `pulso` | `gpio_read` | Leer GPIO (`pulso C 13`) |
| `spark` | `gpio_write` high | Salida alta |
| `mute` | `gpio_write` low | Salida baja |
| `ripple` | `gpio_toggle` | Toggle |
| `scout` | `bus_scan` | Escanear I2C1 |
| `sonar` | `module_read` | Leer módulo USART2 |
| `schema` | `config_get` | Leer clave RFN staging |
| `scribe` | `config_set` | Escribir clave RFN staging |
| `seal` | `config_commit` | Validar/persistir config |
| `nest` | `module_list` | Listar módulos |
| `hatch` | `app_reload` | Recargar `.afr` |
| `coil` | `task_list` | Tareas del scheduler |
| `anchor` | `sys_failsafe` | Modo fail-safe |
| `ward` | `wdt` | Estado / kick watchdog |
| `IDENTIFY` | (local) | Firma de descubrimiento (host serie/BLE) |

## RFN / AFR (resumen)

- **`.rfn`** — Rugus Field Notation: texto `clave = valor`, parseado en userland
  (`rugus-rfn`). Staging en RAM; `seal` persiste cuando SD está presente.
- **`.afr`** — Application for Rugus: cabecera RFN (`app.name`, `app.version`).
  `hatch demo` carga stub embebido en fase 3.

Ejemplo RFN mínimo:

```rfn
# board.rfn
board = bluepill
personality = lite
led = C13
```

## Build y flash

```bash
cd examples/appliance-stm32f103c8-bluepill
cargo build --release
cargo run --release   # probe-rs via .cargo/config.toml
```

Verify automatizado:

```bash
export PROBE_RS_PROBE=0483:3748:55C3BF6B0648C2875752685117C287
./tools/verify-appliance-stm32f103c8-bluepill.sh
```

Verificación UART opcional (adaptador USB-TTL en PA9/PA10):

```bash
export RUGUS_UART_PORT=/dev/ttyUSB0   # o /dev/ttyACM0 según el adaptador
./tools/verify-appliance-stm32f103c8-bluepill.sh
```

El script envía `cosmos` por serie y comprueba respuesta con banner/personalidad.
Requiere `python3` y `pyserial` (`pip install pyserial`).

## Heartbeat PC13 (actividad del kernel)

PC13 es **activo en bajo**. El LED ya no parpadea a 1 Hz fijo: el patrón refleja
carga del firmware:

| Nivel | Patrón | Disparadores |
|-------|--------|--------------|
| Idle | Pulso lento ~0,4 Hz | Sin UART, CLI ni buses |
| Activo | Parpadeo medio ~2 Hz | I2C (`scout`), SD al boot |
| UART | Parpadeo rápido ~8 Hz | Bytes RX en USART1 |
| CLI | Ráfaga triple | Comando procesado (`cosmos`, etc.) |

Implementación: `examples/appliance-stm32f103c8-bluepill/src/heartbeat.rs` — contador
atómico con decaimiento, tarea cooperativa de baja prioridad, sin bloquear el kernel.

## Consola UART (minicom)

Adaptador USB-TTL al USART1:

| Adaptador | Blue Pill |
|-----------|-----------|
| RX | PA9 (TX) |
| TX | PA10 (RX) |
| GND | GND |

```bash
minicom -D /dev/ttyUSB0 -b 115200
```

Tras reset deberías ver `Rugus lite appliance ready.` Escribe `cosmos` y
Enter para el banner ANSI + info del sistema.

## Crates nuevos

| Crate | Rol |
|-------|-----|
| `rush` | Shell on-device (ANSI + comandos + IDENTIFY) |
| `rugus-rfn` | Parser RFN/AFR userland |
| `rugus-core::syscall::lite` | ABI appliance (hooks) |
| `rugus-hal-stm32f1` | `uart`, `uart2`, `i2c`, `spi_sd`, `gpio_raw`, `wdt` |

## Documentos relacionados

- [`RUGUS-KERNEL-VISION.md`](RUGUS-KERNEL-VISION.md)
- [`RUGUS-LITE-ML.md`](RUGUS-LITE-ML.md) — stub fase 6
- [`SYSCALL_ABI.md`](SYSCALL_ABI.md)
