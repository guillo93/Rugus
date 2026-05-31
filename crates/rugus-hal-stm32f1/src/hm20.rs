//! HM-10 / HM-20 BLE UART transparent module (DSD Tech) — init vía AT.
//!
//! Cableado típico en Blue Pill: PA2 → RX del módulo, PA3 ← TX del módulo,
//! 3.3 V y GND. El módulo hace de puente serie↔BLE; el firmware habla AT por
//! USART2 antes de pasar a modo transparente.

use crate::uart2::Usart2;
use rugus_hal::SerialPort;

/// Nombre BLE anunciado por defecto en el appliance Rugus.
pub const DEFAULT_NAME: &str = "RUGUS";

/// Baud de fábrica del HM-20 DSD Tech. El firmware ADOPTA el baud actual del
/// módulo durante el sondeo en vez de forzarlo: 9600 desde el HSI de 8 MHz del
/// F103 tiene <0.1 % de error (vs +0.64 % a 115200 sobre un HSI sin calibrar),
/// y elimina el modo de fallo "MCU y módulo en baudios distintos" que deja el
/// BLE sin enlace. La provisión a otra velocidad se hace en banco con
/// `tools/provision-hm20.sh`; aquí solo nos sincronizamos a lo que el módulo ya
/// usa.
pub const DEFAULT_BAUD: u32 = 9600;

/// Configuración mínima para inicializar un HM-10/HM-20.
#[derive(Clone, Copy, Debug)]
pub struct Hm20Config {
    /// Nombre GAP (`AT+NAME=`).
    pub name: &'static str,
    /// Baud objetivo en el bus serie (9600, 57600 o 115200).
    pub baud: u32,
}

impl Default for Hm20Config {
    fn default() -> Self {
        Self {
            name: DEFAULT_NAME,
            baud: DEFAULT_BAUD,
        }
    }
}

/// Resultado de la secuencia de init AT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitResult {
    /// Módulo respondió OK a probe y (si aplica) nombre configurado.
    Ready,
    /// Sin respuesta AT (módulo ausente o baud distinto).
    NoResponse,
    /// Respondió pero la secuencia AT falló.
    AtError,
}

/// Envía `AT` (sin `\r\n`, según datasheet HM-20) y espera `OK` (poll no
/// bloqueante).
pub fn probe(uart: &mut Usart2) -> bool {
    probe_with_kick(uart, || {})
}

fn probe_with_kick(uart: &mut Usart2, kick: fn()) -> bool {
    drain_rx(uart);
    let _ = uart.write(b"AT");
    let _ = uart.flush();
    wait_ok_with_kick(uart, kick)
}

/// Baud de fábrica habitual en HM-10/HM-20 DSD Tech.
const FACTORY_BAUD: u32 = 9600;

/// Bauds probados en init (orden: fábrica → intermedio → objetivo Rugus).
const PROBE_BAUDS: [u32; 3] = [9600, 57_600, 115_200];

fn probe_bauds(uart: &mut Usart2, kick: fn()) -> Option<u32> {
    for &baud in &PROBE_BAUDS {
        uart.set_baud(baud);
        if probe_with_kick(uart, kick) {
            return Some(baud);
        }
    }
    None
}

/// Factory reset explícito: `AT+RENEW`, `AT+RESET`, luego [`init`].
///
/// `kick` se invoca en cada iteración de espera (p. ej. WDT del appliance).
/// Destructivo: borra nombre/baud/PIN del módulo. Solo para recuperación en campo.
pub fn factory_renew(uart: &mut Usart2, cfg: Hm20Config, kick: fn()) -> InitResult {
    if probe_bauds(uart, kick).is_none() {
        return InitResult::NoResponse;
    }

    if !send_at_with_kick(uart, b"AT+RENEW", kick) {
        return InitResult::AtError;
    }
    if !send_at_with_kick(uart, b"AT+RESET", kick) {
        return InitResult::AtError;
    }

    kick();
    cortex_m::asm::delay(3_000_000);
    kick();

    uart.set_baud(FACTORY_BAUD);
    init(uart, cfg)
}

/// Inicializa el módulo adoptando su baud actual (prueba 9600, 57600 y 115200).
///
/// No fuerza un cambio de baud: tras el sondeo el USART2 queda sincronizado a la
/// velocidad real del módulo, que es la que usará el puente BLE transparente.
/// Esto elimina la condición de carrera/desajuste de baudios que dejaba el
/// enlace mudo. Nombre y `AT+NOTI1` son best-effort: si el `AT` respondió, el
/// enlace de datos ya sirve y devolvemos `Ready` aunque el setter cosmético
/// falle por dialecto de firmware.
pub fn init(uart: &mut Usart2, cfg: Hm20Config) -> InitResult {
    init_with_kick(uart, cfg, || {})
}

/// Igual que [`init`] pero invoca `kick` en cada espera (alimenta el watchdog
/// durante el sondeo de arranque, que puede tardar ~1 s si el módulo está a un
/// baud distinto al primero probado).
pub fn init_with_kick(uart: &mut Usart2, cfg: Hm20Config, kick: fn()) -> InitResult {
    if probe_bauds(uart, kick).is_none() {
        // Dejar el bus en el baud de fábrica para que `sonar`/`nest renew`
        // manuales hablen a 9600 en vez de quedar en el último baud probado.
        uart.set_baud(FACTORY_BAUD);
        return InitResult::NoResponse;
    }

    // Best-effort: el enlace transparente funciona aunque estos fallen.
    let _ = set_name(uart, cfg.name);
    let _ = send_at_with_kick(uart, b"AT+NOTI1", kick);

    InitResult::Ready
}

/// Provisión persistente del nombre BLE.
///
/// Este módulo (se anuncia `HMSoft` de fábrica, dialecto HM-10/HMSoft V5+) usa
/// `AT+NAME<n>` SIN `=` → responde `OK+Set:<n>`. El `=` NO se separa: `AT+NAME=X`
/// fija el nombre literal `=X` (verificado en HW), así que NUNCA usar `=`. El
/// cambio sólo se anuncia tras `AT+RESET`; el `AT+NAME?` previo al reset sigue
/// devolviendo el nombre viejo. Por eso: set → reset → esperar reinicio (~7,5 s
/// de margen) → re-sincronizar baud → leer de vuelta y confirmar que coincide.
/// Devuelve `true` sólo si el readback contiene el nombre solicitado. Pensado
/// para provisión puntual desde consola (`scribe ble.name <n>`), no en cada
/// arranque: persiste en la NVRAM del módulo.
///
/// `kick` se invoca en cada espera para alimentar el watchdog (la secuencia
/// corre síncrona en la tarea CLI, que no cede el CPU mientras tanto).
pub fn provision_name(uart: &mut Usart2, name: &str, kick: fn()) -> bool {
    let name_b = name.as_bytes();
    if name_b.is_empty() || name_b.len() > 18 {
        return false;
    }
    let mut buf = [0u8; 32];
    let n = build_name_cmd(&mut buf, b"AT+NAME", name_b);
    if !send_at_with_kick(uart, &buf[..n], kick) {
        return false;
    }
    // Aplicar: reset y esperar a que el módulo reinicie antes de re-sincronizar.
    let _ = send_at_with_kick(uart, b"AT+RESET", kick);
    for _ in 0..20 {
        kick();
        cortex_m::asm::delay(3_000_000);
    }
    let _ = probe_bauds(uart, kick);
    // Confirmar leyendo de vuelta el nombre real.
    let mut rb = [0u8; 48];
    let r = send_at_capture(uart, b"AT+NAME?", &mut rb, kick);
    contains(&rb[..r], name_b)
}

/// Lee el nombre BLE actual del módulo (`AT+NAME?`) y captura la respuesta cruda
/// en `out`. La respuesta varía por dialecto (`OK+NAME:<n>` u `OK+Get:<n>`).
/// Devuelve los bytes capturados.
pub fn query_name(uart: &mut Usart2, out: &mut [u8], kick: fn()) -> usize {
    send_at_capture(uart, b"AT+NAME?", out, kick)
}

/// `true` si `needle` aparece como subsecuencia contigua dentro de `hay`.
fn contains(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > hay.len() {
        return false;
    }
    hay.windows(needle.len()).any(|w| w == needle)
}

/// Concatena `prefix` + `name` en `buf`; devuelve la longitud. Trunca si no cabe.
fn build_name_cmd(buf: &mut [u8], prefix: &[u8], name: &[u8]) -> usize {
    let len = (prefix.len() + name.len()).min(buf.len());
    let p = prefix.len().min(buf.len());
    buf[..p].copy_from_slice(&prefix[..p]);
    let n = len - p;
    buf[p..len].copy_from_slice(&name[..n]);
    len
}

/// Envía `cmd` y captura la respuesta cruda en `out` (ventana ~250 ms). No
/// interpreta OK/ERROR; útil para diagnóstico/lectura de campos.
fn send_at_capture(uart: &mut Usart2, cmd: &[u8], out: &mut [u8], kick: fn()) -> usize {
    drain_rx(uart);
    let _ = uart.write(cmd);
    let _ = uart.flush();
    let mut pos = 0usize;
    for _ in 0..4_000 {
        kick();
        if let Some(b) = uart.try_read_byte() {
            if pos < out.len() {
                out[pos] = b;
                pos += 1;
            }
        } else {
            cortex_m::asm::delay(500);
        }
    }
    pos
}

fn set_name(uart: &mut Usart2, name: &str) -> bool {
    // Datasheet HM-20: `AT+NAMEname` (sin `=`, sin `\r\n`) → `OK+Set:name`.
    // Nombre máx. 18 chars; el módulo lo trunca si excede.
    let prefix = b"AT+NAME";
    let name_bytes = name.as_bytes();
    let mut cmd = [0u8; 32];
    let len = prefix.len() + name_bytes.len();
    if len > cmd.len() {
        return false;
    }
    cmd[..prefix.len()].copy_from_slice(prefix);
    cmd[prefix.len()..len].copy_from_slice(name_bytes);
    send_at(uart, &cmd[..len])
}

fn send_at(uart: &mut Usart2, cmd: &[u8]) -> bool {
    send_at_with_kick(uart, cmd, || {})
}

fn send_at_with_kick(uart: &mut Usart2, cmd: &[u8], kick: fn()) -> bool {
    drain_rx(uart);
    let _ = uart.write(cmd);
    let _ = uart.flush();
    wait_ok_with_kick(uart, kick)
}

fn wait_ok_with_kick(uart: &mut Usart2, kick: fn()) -> bool {
    // Sin retardo de bloqueo previo: sondear desde ya. El registro RX del F103
    // es de 1 byte; un `delay` aquí desborda la respuesta `OK\r\n` antes de
    // leerla. La ventana total del bucle (~250 ms) cubre de sobra la latencia
    // AT del HM-20.
    let mut buf = [0u8; 32];
    let mut pos = 0usize;
    for _ in 0..4_000 {
        kick();
        if let Some(b) = uart.try_read_byte() {
            if pos < buf.len() {
                buf[pos] = b;
                pos += 1;
            }
            if window_has_ok(&buf[..pos]) {
                return true;
            }
            if window_has_error(&buf[..pos]) {
                return false;
            }
        } else {
            cortex_m::asm::delay(500);
        }
    }
    window_has_ok(&buf[..pos])
}

fn window_has_ok(buf: &[u8]) -> bool {
    buf.windows(2).any(|w| w == b"OK")
}

fn window_has_error(buf: &[u8]) -> bool {
    buf.windows(5).any(|w| w == b"ERROR")
}

fn drain_rx(uart: &mut Usart2) {
    while uart.try_read_byte().is_some() {}
}
