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
| Módulos (LoRa, HM-10/HM-20 BLE) | USART2 | PA2 TX, PA3 RX | Bus serie 115200 8N1 (IDENTIFY) |
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
| `nest renew` | `module_renew` | Factory reset HM-20 + re-init (destructivo) |
| `hatch` | `app_reload` | Recargar `.afr` |
| `coil` | `task_list` | Tareas del scheduler |
| `anchor` | `sys_failsafe(0)` | Activar fail-safe |
| `anchor off` / `anchor release` | `sys_failsafe(1)` | Desactivar fail-safe |
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

## Módulo BLE HM-10 / HM-20 (USART2)

Adaptador DSD Tech HM-10 o HM-20 (UART transparente BLE):

| Blue Pill | Módulo HM-20 |
|-----------|--------------|
| PA2 (TX)  | RX           |
| PA3 (RX)  | TX           |
| 3.3 V     | VCC          |
| GND       | GND          |
| 3.3 V     | KEY (modo AT; sin KEY el módulo no responde AT) |

El firmware inicializa el módulo al boot con `rugus-hal-stm32f1::hm20`: prueba
**9600 baud** (fábrica DSD Tech) y, si responde, configura `AT+NAME=RUGUS` y
`AT+BAUD4` (115200). Si ya está a 115200, solo renombra. Descriptor de
capacidad: [`examples/eco/hm20-ble.eco`](../examples/eco/hm20-ble.eco).

Comandos útiles en consola USART1:

```text
nest        → slot0: usart2 (hm20-ble)
nest renew  → factory reset AT+RENEW/+RESET + re-init (solo si USART2 presente)
sonar 0     → probe AT (respuesta del módulo; no bloquea ni resetea)
ecosystem   → usart2: hm20-ready | hm20-at-warn | no-at-response | idle
IDENTIFY    → firma RUGUS (también en USART2 para host BLE)
```

### Prueba BLE desde el PC (minicom + teléfono)

1. Flashea el appliance y conecta HM-20 como arriba (**KEY a 3.3 V**).
2. Consola local: `minicom -D /dev/ttyUSB0 -b 115200` → `nest` debe listar `hm20-ble`.
3. En el teléfono, app serial BLE (nRF Connect, Serial Bluetooth Terminal): empareja
   con **RUGUS** (PIN habitual `000000` o `123456` según lote).
4. Envía `IDENTIFY` desde la app BLE; debe responder
   `RUGUS;tier=lite;chip=f103;proto=1;shell=rush;cli=1.0.0`.
5. Opcional: `rugus-cli` en el PC escanea BLE y auto-detecta la firma (ver
   [`RUGUS-CLI-HOST.md`](RUGUS-CLI-HOST.md)).

### Troubleshooting HM-20

| Síntoma | Causa probable | Acción |
|---------|----------------|--------|
| `nest` → `(no modules)`, `ecosystem` → `no-at-response` | KEY flotante o baud distinto | Conectar **KEY a 3.3 V**; cableado PA2↔RX, PA3↔TX; reset |
| `ecosystem` → `hm20-at-warn` | Módulo responde AT pero falló nombre/baud | Revisar alimentación 3.3 V estable; `sonar 0` manual |
| `sonar 0` resetea la placa | Bug corregido: lectura bloqueante sin kick WDT | Reflashear firmware actual |
| BLE no anuncia **RUGUS** | Init falló o KEY sin 3.3 V | Tras fix: `ecosystem` debe mostrar `hm20-ready` |
| BLE anuncia nombre ajeno (p. ej. **sopesa**) y `no-at-response` | Módulo usado en otro proyecto; baud/nombre viejos | Factory reset AT (ver abajo); desempareja BLE del teléfono |

### Factory reset HM-10 / HM-20 (módulo usado en otro proyecto)

Con **KEY a 3.3 V** y sin enlace BLE activo (desconecta el teléfono del módulo):

```text
AT+RENEW      → OK+RENEW   (restaura fábrica: nombre HMSoft, baud 9600, PIN 000000)
AT+RESET      → OK+RESET   (reinicia; tras esto el UART vuelve a 9600)
AT            → OK         (verificación)
```

**Bench (host):** script [`tools/provision-hm20.sh`](../tools/provision-hm20.sh)
— prueba 9600/115200, `AT+RENEW`, `AT+RESET`, verifica `AT+NAME?` / `AT+BAUD?`.
Opcional `--provision` fija `AT+NAME=RUGUS` y `AT+BAUD4` antes de flashear Rugus.
Requiere `python3` + `pyserial` (igual que verify-appliance UART).

**Campo (Rugus consola):** `nest renew` — mismo reset destructivo vía firmware
(sin auto-renew al boot). Desempareja BLE del teléfono antes.

Prueba AT con minicom directo al módulo (USB-TTL ↔ HM-20) si el script no aplica.
Tras reset + reinicio/reflash del appliance, el driver `hm20` renombra a **RUGUS**
y sube a 115200. El nombre BLE antiguo persiste hasta que `AT+NAME` tenga éxito.

La mayoría de HM-10/HM-20 DSD salen de fábrica a **9600 baud**; el firmware
detecta y sube a 115200 automáticamente. Solo si el init falla, reprograma con
terminal AT directo (`AT+BAUD4` → 115200) o revisa KEY/cableado.

## Crates nuevos

| Crate | Rol |
|-------|-----|
| `rush` | Shell on-device (ANSI + comandos + IDENTIFY) |
| `rugus-rfn` | Parser RFN/AFR userland |
| `rugus-core::syscall::lite` | ABI appliance (hooks) |
| `rugus-hal-stm32f1` | `uart`, `uart2`, `hm20`, `i2c`, `spi_sd`, `gpio_raw`, `wdt` |

## Documentos relacionados

- [`RUGUS-KERNEL-VISION.md`](RUGUS-KERNEL-VISION.md)
- [`RUGUS-LITE-ML.md`](RUGUS-LITE-ML.md) — stub fase 6
- [`SYSCALL_ABI.md`](SYSCALL_ABI.md)
