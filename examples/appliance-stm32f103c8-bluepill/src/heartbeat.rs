//! Heartbeat PC13 consciente de actividad (Blue Pill, activo en bajo).
//!
//! El patrón refleja carga del kernel: pulso lento en idle, parpadeo rápido
//! con UART/CLI/I2C/SD, ráfaga triple tras un comando CLI.

use core::sync::atomic::{AtomicU32, Ordering};

use rugus_hal_stm32f1::gpio_raw;

const MAX_SCORE: u32 = 255;
const PORT: u8 = b'C';
const PIN: u8 = 13;

static ACTIVITY: AtomicU32 = AtomicU32::new(0);

/// Peso por byte UART RX.
pub const UART_RX: u32 = 4;
/// Peso por comando CLI procesado.
pub const CLI_CMD: u32 = 48;
/// Peso por escaneo I2C.
pub const I2C: u32 = 24;
/// Peso por acceso SD.
pub const SD: u32 = 18;

/// Registra actividad del sistema (atómico, no bloquea).
pub fn note(amount: u32) {
    let _ = ACTIVITY.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
        Some(v.saturating_add(amount).min(MAX_SCORE))
    });
}

/// Decae una unidad y devuelve el nivel actual.
pub fn level() -> u32 {
    let v = ACTIVITY.load(Ordering::Relaxed);
    if v > 0 {
        ACTIVITY.store(v - 1, Ordering::Relaxed);
    }
    v
}

/// Enciende LED onboard (pin bajo).
pub fn led_on() {
    let _ = gpio_raw::write(PORT, PIN, false);
}

/// Apaga LED onboard (pin alto).
pub fn led_off() {
    let _ = gpio_raw::write(PORT, PIN, true);
}

/// Calcula si el LED debe estar encendido y el retardo hasta la siguiente iteración.
pub fn step(act: u32, tick: u32) -> (bool, u32) {
    const HZ: u32 = 8_000_000;

    if act >= CLI_CMD {
        // Ráfaga triple tras comando CLI.
        let phase = tick % 12;
        let on = phase == 0 || phase == 2 || phase == 4;
        return (on, HZ / 32);
    }
    if act >= UART_RX * 3 {
        // UART / subsistema activo: parpadeo rápido (~8 Hz).
        let on = tick % 2 == 0;
        return (on, HZ / 16);
    }
    if act >= I2C {
        // I2C / SD: parpadeo medio (~2 Hz).
        let on = tick % 8 < 2;
        return (on, HZ / 16);
    }
    if act > 0 {
        // Actividad residual: pulso ocasional.
        let on = tick % 32 == 0;
        return (on, HZ / 32);
    }

    // Idle: pulso lento ~0.4 Hz (respiración del kernel).
    let on = tick % 640 == 0;
    (on, HZ / 80)
}
