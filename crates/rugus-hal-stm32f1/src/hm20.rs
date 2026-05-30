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
    drain_rx(uart);
    let _ = uart.write(b"AT\r\n");
    wait_ok(uart, 120_000)
}

/// Configura nombre y baud AT; tolera módulos ya en 115200.
pub fn init(uart: &mut Usart2, cfg: Hm20Config) -> InitResult {
    if !probe(uart) {
        return InitResult::NoResponse;
    }

    if !set_name(uart, cfg.name) {
        return InitResult::AtError;
    }

    if cfg.baud != DEFAULT_BAUD && !set_baud(uart, cfg.baud) {
        return InitResult::AtError;
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
    drain_rx(uart);
    let _ = uart.write(cmd);
    wait_ok(uart, delay_cycles)
}

fn wait_ok(uart: &mut Usart2, delay_cycles: u32) -> bool {
    cortex_m::asm::delay(delay_cycles);
    let mut buf = [0u8; 32];
    let mut pos = 0usize;
    for _ in 0..4_000 {
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
