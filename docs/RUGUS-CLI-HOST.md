# `rugus-cli` (host) — cliente de escritorio Rugus

Cliente de **PC** para descubrir y manejar dispositivos Rugus por **serie** y
**BLE**. Es la "cima bonita" de la experiencia: tablas, colores y paneles viven
aquí (TUI), no en el kernel. El dispositivo solo expone la shell `rush` y una
respuesta `IDENTIFY` barata.

> Stack A: núcleo Rust (`rugus-proto`) + frontend nativo (`ratatui`). Pura Rust.

## Arquitectura

```
rugus-cli (binario host, std)
  ├─ transporte serie  (serialport, hilos)
  ├─ transporte BLE    (btleplug, tokio en hilo dedicado)
  └─ TUI               (ratatui + crossterm)
        ↓ usa
rugus-proto (lib host, std)
  ├─ identify   — handshake IDENTIFY, parseo/validación de la firma
  ├─ frame      — ensamblado de líneas desde bytes (serie/BLE)
  ├─ command    — modelo del léxico `rush` + serialización al cable
  └─ render     — secuencias ANSI SGR → spans estilados
```

`rugus-proto` es agnóstico del transporte y de la UI; se reutilizará desde un
frontend Android (vía UniFFI) sin tocar la lógica de protocolo.

Estos crates son **host-side** (triple del PC) y están **excluidos** del
workspace embebido (`exclude` en el `Cargo.toml` raíz), de modo que
`cargo build --workspace --target thumbv7m-*` (CI embebido) no los compila. El
job `host` del CI los construye/testea sobre el triple nativo.

## Protocolo IDENTIFY (spec)

El host descubre dispositivos enviando un disparador y validando la respuesta.

**Disparador (host → dispositivo):**

- La línea de texto `IDENTIFY\r\n`, **o**
- El byte de control `ENQ` (`0x05`).

**Respuesta (dispositivo → host):** exactamente **una línea** terminada en `\r\n`:

```text
RUGUS;tier=<tier>;chip=<chip>;proto=<n>;shell=<shell>;cli=<ver>
```

Ejemplo del appliance F103 (`rush`):

```text
RUGUS;tier=lite;chip=f103;proto=1;shell=rush;cli=1.0.0
```

**Campos:**

| Campo   | Significado                         | Ejemplo |
|---------|-------------------------------------|---------|
| `tier`  | Nivel del dispositivo               | `lite`  |
| `chip`  | Familia de chip                     | `f103`  |
| `proto` | Versión del protocolo IDENTIFY      | `1`     |
| `shell` | Shell on-device                     | `rush`  |
| `cli`   | Versión del léxico                  | `1.0.0` |

**Reglas de parseo (`rugus_proto::parse_signature`):**

- Debe empezar por el prefijo `RUGUS;`. Cualquier otra cosa se **rechaza**
  (`SignatureError::NotRugus`) — así se ignoran otros dispositivos serie/BLE.
- Tolera `\r\n` final y espacios alrededor de `;` y `=`.
- Campos desconocidos se preservan en `extra` (forward-compat).
- Faltar `tier`/`chip`/`proto`/`shell`/`cli` → error; `proto` no numérico → error.

El firmware implementa el lado dispositivo en el crate `rush`
(`rush::identify`), respondiendo tanto en USART1 (consola) como en USART2
(bus de módulos / puente BLE). Ver [`RUGUS-LITE-APPLIANCE.md`](RUGUS-LITE-APPLIANCE.md).

## Flujo de auto-detección

1. **Enumerar serie:** lista los puertos del sistema (`serialport`).
2. **Escanear BLE:** descubre periféricos (`btleplug`), busca una característica
   *notify* + una *write* (perfiles HM-10 `FFE0/FFE1` y Nordic UART).
3. **Sondear:** a cada candidato le envía `IDENTIFY` y lee la respuesta con un
   timeout corto.
4. **Filtrar:** solo se listan los que devuelven una **firma `RUGUS;` válida**.
5. **Conectar:**
   - 0 dispositivos → mensaje de ayuda y salida.
   - 1 dispositivo → conexión automática.
   - varios → **menú** de selección.
6. **Sesión TUI:** consola con scrollback estilado (ANSI → colores), panel con
   el léxico `rush`, y línea de entrada. Los comandos (`cosmos`, `orbit`,
   `ecosystem`, …) se transmiten tal cual al dispositivo (passthrough).

## Uso en PC

```bash
# Compilar (host). En Linux requiere libudev y dbus de desarrollo:
#   Debian/Ubuntu: sudo apt install libudev-dev libdbus-1-dev pkg-config
#   Fedora:        sudo dnf install systemd-devel dbus-devel pkgconf-pkg-config
cd crates/rugus-cli
cargo build --release

# Auto-detección (serie + BLE) y conexión:
cargo run --release

# Opciones:
cargo run --release -- --list          # detectar y listar, luego salir
cargo run --release -- --no-ble         # solo serie
cargo run --release -- --no-serial      # solo BLE
cargo run --release -- --serial /dev/ttyUSB0   # conexión directa a un puerto
```

En la TUI: escribe un comando y pulsa **Enter** para enviarlo; **Esc** o
**Ctrl-C** para salir. El panel derecho lista el léxico `rush` como referencia.

**Permisos serie (Linux):** añade tu usuario al grupo `dialout` (o `uucp`) para
acceder a `/dev/ttyUSB*` sin `sudo`.

**Cableado del adaptador USB-TTL al USART1 del appliance:** RX↔PA9 (TX),
TX↔PA10 (RX), GND↔GND, 115200 8N1.

## Frontend Android (futuro)

El plan es **BLE-first** en Android (HM-10 / Nordic UART como puente):

- Reutilizar `rugus-proto` desde Kotlin/Swift vía **UniFFI** (bindings generados
  sobre la misma lógica de IDENTIFY/frame/command/render). Sin reimplementar el
  protocolo.
- El transporte BLE nativo de la plataforma (Android BLE / CoreBluetooth)
  alimenta el mismo `LineAssembler` y `parse_signature`.
- La UI puede ser nativa (Compose) consumiendo el modelo de comandos y el render
  de spans de `rugus-proto`.

## Documentos relacionados

- [`RUGUS-LITE-APPLIANCE.md`](RUGUS-LITE-APPLIANCE.md) — firmware lite + IDENTIFY.
- [`RUGUS-KERNEL-VISION.md`](RUGUS-KERNEL-VISION.md) — capas y regla de oro.
