//! HM-10 / HM-20 BLE UART transparent module (DSD Tech) — init vía AT.
//!
//! Cableado típico en Blue Pill: PA2 → RX del módulo, PA3 ← TX del módulo,
//! 3.3 V y GND. El módulo hace de puente serie↔BLE; el firmware habla AT por
//! USART2 antes de pasar a modo transparente.

use crate::uart2::Usart2;
use rugus_hal::SerialPort;

/// Nombre BLE anunciado por defecto en el appliance Rugus.
pub const DEFAULT_NAME: &str = "RUGUS";

/// Baud del bus USART2 (debe coincidir con `AT+BAUD` del módulo).
pub const DEFAULT_BAUD: u32 = 115_200;

/// Configuración mínima para inicializar un HM-10/HM-20.
#[derive(Clone, Copy, Debug)]
pub struct Hm20Config {
    /// Nombre GAP (`AT+NAME=`).
    pub name: &'static str,
    /// Baud objetivo en el bus serie (9600 o 115200).
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

/// Envía `AT\r\n` y espera eco con `OK`/`ERROR` (poll no bloqueante).
pub fn probe(uart: &mut Usart2) -> bool {
    probe_with_kick(uart, || {})
}

fn probe_with_kick(uart: &mut Usart2, kick: fn()) -> bool {
    drain_rx(uart);
    let _ = uart.write(b"AT\r\n");
    wait_ok_with_kick(uart, 120_000, kick)
}

/// Baud de fábrica habitual en HM-10/HM-20 DSD Tech.
const FACTORY_BAUD: u32 = 9600;

/// Factory reset explícito: `AT+RENEW`, `AT+RESET`, luego [`init`].
///
/// `kick` se invoca en cada iteración de espera (p. ej. WDT del appliance).
/// Destructivo: borra nombre/baud/PIN del módulo. Solo para recuperación en campo.
pub fn factory_renew(uart: &mut Usart2, cfg: Hm20Config, kick: fn()) -> InitResult {
    uart.set_baud(FACTORY_BAUD);
    let mut at_ok = probe_with_kick(uart, kick);
    if !at_ok {
        uart.set_baud(DEFAULT_BAUD);
        at_ok = probe_with_kick(uart, kick);
    }
    if !at_ok {
        return InitResult::NoResponse;
    }

    if !send_at_with_kick(uart, b"AT+RENEW\r\n", 250_000, kick) {
        return InitResult::AtError;
    }
    if !send_at_with_kick(uart, b"AT+RESET\r\n", 250_000, kick) {
        return InitResult::AtError;
    }

    kick();
    cortex_m::asm::delay(3_000_000);
    kick();

    uart.set_baud(FACTORY_BAUD);
    init(uart, cfg)
}

/// Configura nombre y baud AT; prueba 9600 (fábrica) y luego 115200.
pub fn init(uart: &mut Usart2, cfg: Hm20Config) -> InitResult {
    uart.set_baud(FACTORY_BAUD);
    if probe(uart) {
        return finish_init(uart, cfg, true);
    }

    uart.set_baud(DEFAULT_BAUD);
    if !probe(uart) {
        return InitResult::NoResponse;
    }

    finish_init(uart, cfg, false)
}

fn finish_init(uart: &mut Usart2, cfg: Hm20Config, upgrade_baud: bool) -> InitResult {
    if !set_name(uart, cfg.name) {
        return InitResult::AtError;
    }

    if upgrade_baud {
        if !set_baud(uart, cfg.baud) {
            return InitResult::AtError;
        }
        uart.set_baud(cfg.baud);
        if !probe(uart) {
            return InitResult::AtError;
        }
    }

    // Notificaciones de enlace BLE (opcional, ignora error).
    let _ = send_at(uart, b"AT+NOTI1\r\n", 80_000);

    InitResult::Ready
}

fn set_name(uart: &mut Usart2, name: &str) -> bool {
    let prefix = b"AT+NAME=";
    let suffix = b"\r\n";
    let name_bytes = name.as_bytes();
    let mut cmd = [0u8; 32];
    let len = prefix.len() + name_bytes.len() + suffix.len();
    if len > cmd.len() {
        return false;
    }
    cmd[..prefix.len()].copy_from_slice(prefix);
    cmd[prefix.len()..prefix.len() + name_bytes.len()].copy_from_slice(name_bytes);
    cmd[prefix.len() + name_bytes.len()..len].copy_from_slice(suffix);
    send_at(uart, &cmd[..len], 100_000)
}

fn set_baud(uart: &mut Usart2, baud: u32) -> bool {
    let code = match baud {
        9600 => b'1',
        115_200 => b'4',
        _ => return false,
    };
    let cmd = [
        b'A', b'T', b'+', b'B', b'A', b'U', b'D', b'=', code, b'\r', b'\n',
    ];
    send_at(uart, &cmd, 100_000)
}

fn send_at(uart: &mut Usart2, cmd: &[u8], delay_cycles: u32) -> bool {
    send_at_with_kick(uart, cmd, delay_cycles, || {})
}

fn send_at_with_kick(uart: &mut Usart2, cmd: &[u8], delay_cycles: u32, kick: fn()) -> bool {
    drain_rx(uart);
    let _ = uart.write(cmd);
    wait_ok_with_kick(uart, delay_cycles, kick)
}

fn wait_ok_with_kick(uart: &mut Usart2, delay_cycles: u32, kick: fn()) -> bool {
    cortex_m::asm::delay(delay_cycles);
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
