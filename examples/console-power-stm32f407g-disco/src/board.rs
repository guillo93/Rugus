//! Operaciones de placa del tier full para STM32F407G-DISC1.
//!
//! Aporta las piezas de [`BoardOps`] que dependen del silicio F4 —el GPIO
//! genérico— y delega el resto en los stubs honestos (`Enosys`) de
//! [`rugus_personality_full::BoardOps::unsupported`]. La tabla genérica
//! (`cosmos`/`ecosystem`/`letargo`/`coil`/`scar`) la compone el crate de
//! personalidad full a partir de [`rugus_kernel`]; aquí solo inyectamos lo de
//! placa.
//!
//! GPIO sobre los verbos `rush`:
//! - `pulso` ([`gpio_read`]): reconfigura el pin como entrada y lee su nivel.
//! - `spark`/`mute` ([`gpio_write`]): salida push-pull y fija el nivel.
//! - `ripple` ([`gpio_toggle`]): salida push-pull e invierte el nivel.
//!
//! El puerto se codifica `0=A … 8=I` y el pin `0..=15`, igual que el mapa de
//! `Port` del HAL F4. El operador está autenticado (canal gateado): manipular
//! PA2/PA3 puede tumbar la propia consola UART, es su responsabilidad.

use rugus_core::syscall::lite::GpioLevel;
use rugus_core::Errno;
use rugus_hal::GpioPin;
use rugus_hal_stm32f4::gpio::{Pin, PinConfig, Port, Pull};
use rugus_personality_full::BoardOps;

/// Identidad de la placa para `cosmos`/`ecosystem`.
const BOARD_NAME: &str = "f407-disco";

/// Construye las operaciones de placa del F407: GPIO real + stubs `Enosys` para
/// lo que este ejemplo no cablea (bind/bus/wdt/failsafe/sting).
pub fn ops() -> BoardOps {
    BoardOps {
        gpio_read,
        gpio_write,
        gpio_toggle,
        ..BoardOps::unsupported(BOARD_NAME)
    }
}

/// Traduce el índice de puerto (`0=A … 8=I`) al enum del HAL.
fn port_from(u: u8) -> Option<Port> {
    Some(match u {
        0 => Port::A,
        1 => Port::B,
        2 => Port::C,
        3 => Port::D,
        4 => Port::E,
        5 => Port::F,
        6 => Port::G,
        7 => Port::H,
        8 => Port::I,
        _ => return None,
    })
}

/// `pulso` → reconfigura el pin como entrada (sin pull) y devuelve su nivel
/// (`0`/`1`), o un errno negativo si el puerto/pin no es válido.
fn gpio_read(port: u8, pin: u8) -> i32 {
    let Some(p) = port_from(port) else {
        return Errno::Einval as i32;
    };
    if pin >= 16 {
        return Errno::Einval as i32;
    }
    let g = Pin::new(p, pin, PinConfig::input(Pull::None));
    match g.is_high() {
        Ok(true) => 1,
        Ok(false) => 0,
        Err(_) => Errno::Efault as i32,
    }
}

/// `spark`/`mute` → configura el pin como salida push-pull y fija el nivel.
fn gpio_write(port: u8, pin: u8, level: GpioLevel) -> i32 {
    let Some(p) = port_from(port) else {
        return Errno::Einval as i32;
    };
    if pin >= 16 {
        return Errno::Einval as i32;
    }
    let mut g = Pin::new(p, pin, PinConfig::output());
    let r = match level {
        GpioLevel::High => g.set_high(),
        GpioLevel::Low => g.set_low(),
    };
    match r {
        Ok(()) => 0,
        Err(_) => Errno::Efault as i32,
    }
}

/// `ripple` → configura el pin como salida push-pull e invierte su nivel.
fn gpio_toggle(port: u8, pin: u8) -> i32 {
    let Some(p) = port_from(port) else {
        return Errno::Einval as i32;
    };
    if pin >= 16 {
        return Errno::Einval as i32;
    }
    let mut g = Pin::new(p, pin, PinConfig::output());
    match g.toggle() {
        Ok(()) => 0,
        Err(_) => Errno::Efault as i32,
    }
}
