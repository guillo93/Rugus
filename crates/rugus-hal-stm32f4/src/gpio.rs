//! GPIO para STM32F4.
//!
//! API genérica de pin (cualquier puerto/pin, modos input/output/AF/analog,
//! pull, speed, open-drain) por acceso MMIO directo: el bloque GPIO de la
//! familia F4 tiene layout idéntico en todos los puertos (base `0x4002_0000`,
//! stride `0x400`). Los LEDs de la STM32F407G-DISC1 (UM1472 §6.4) son un caso
//! particular construido sobre [`Pin`].

use crate::pac;
use core::ptr::{read_volatile, write_volatile};
use rugus_hal::GpioPin;

/// Base del primer puerto GPIO (GPIOA) en la familia STM32F4.
const GPIO_BASE: u32 = 0x4002_0000;
/// Separación entre bloques de puerto consecutivos.
const GPIO_STRIDE: u32 = 0x400;
/// `RCC->AHB1ENR`: bit N habilita el reloj del puerto N (GPIOA=0 … GPIOK=10).
const RCC_AHB1ENR: u32 = 0x4002_3830;

// Offsets de registro dentro de un bloque de puerto GPIO.
const MODER: u32 = 0x00;
const OTYPER: u32 = 0x04;
const OSPEEDR: u32 = 0x08;
const PUPDR: u32 = 0x0C;
const IDR: u32 = 0x10;
const ODR: u32 = 0x14;
const BSRR: u32 = 0x18;
const AFRL: u32 = 0x20;
const AFRH: u32 = 0x24;

/// Puerto GPIO. El índice (A=0 … K=10) selecciona base y bit de reloj.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Port {
    /// GPIOA.
    A,
    /// GPIOB.
    B,
    /// GPIOC.
    C,
    /// GPIOD.
    D,
    /// GPIOE.
    E,
    /// GPIOF.
    F,
    /// GPIOG.
    G,
    /// GPIOH.
    H,
    /// GPIOI.
    I,
}

impl Port {
    #[inline]
    fn index(self) -> u32 {
        self as u32
    }

    #[inline]
    fn base(self) -> u32 {
        GPIO_BASE + GPIO_STRIDE * self.index()
    }
}

/// Modo de un pin (campo MODER de 2 bits; función alternativa lleva su AF 0–15).
#[derive(Clone, Copy, Debug)]
pub enum Mode {
    /// Entrada digital.
    Input,
    /// Salida digital.
    Output,
    /// Función alternativa con número AF (0–15).
    Alternate(u8),
    /// Analógico (ADC/DAC).
    Analog,
}

/// Resistencia de pull interna (campo PUPDR).
#[derive(Clone, Copy, Debug)]
pub enum Pull {
    /// Sin pull.
    None,
    /// Pull-up.
    Up,
    /// Pull-down.
    Down,
}

/// Velocidad de slew del driver de salida (campo OSPEEDR).
#[derive(Clone, Copy, Debug)]
pub enum Speed {
    /// Baja.
    Low,
    /// Media.
    Medium,
    /// Alta.
    High,
    /// Muy alta.
    VeryHigh,
}

/// Tipo de driver de salida (campo OTYPER).
#[derive(Clone, Copy, Debug)]
pub enum OutputType {
    /// Push-pull.
    PushPull,
    /// Open-drain.
    OpenDrain,
}

/// Configuración completa de un pin.
#[derive(Clone, Copy, Debug)]
pub struct PinConfig {
    /// Modo (input/output/AF/analog).
    pub mode: Mode,
    /// Pull interno.
    pub pull: Pull,
    /// Velocidad de salida.
    pub speed: Speed,
    /// Tipo de salida.
    pub otype: OutputType,
}

impl PinConfig {
    /// Salida push-pull, sin pull, velocidad baja (típico para LED).
    pub const fn output() -> Self {
        Self {
            mode: Mode::Output,
            pull: Pull::None,
            speed: Speed::Low,
            otype: OutputType::PushPull,
        }
    }

    /// Entrada con el pull indicado (típico para botón).
    pub const fn input(pull: Pull) -> Self {
        Self {
            mode: Mode::Input,
            pull,
            speed: Speed::Low,
            otype: OutputType::PushPull,
        }
    }
}

/// Pin GPIO genérico configurado en construcción.
pub struct Pin {
    port: Port,
    pin: u8,
}

impl Pin {
    /// Habilita el reloj del puerto y aplica `cfg`. `pin` ∈ 0..=15.
    pub fn new(port: Port, pin: u8, cfg: PinConfig) -> Self {
        debug_assert!(pin < 16);
        enable_port_clock(port);
        configure(port, pin, cfg);
        Self { port, pin }
    }
}

impl GpioPin for Pin {
    type Error = core::convert::Infallible;

    fn set_high(&mut self) -> Result<(), Self::Error> {
        // SAFETY: BSRR es write-only y atómico por bit.
        unsafe { write_reg(self.port, BSRR, 1 << self.pin) };
        Ok(())
    }

    fn set_low(&mut self) -> Result<(), Self::Error> {
        // SAFETY: BSRR es write-only y atómico por bit (mitad alta = reset).
        unsafe { write_reg(self.port, BSRR, 1 << (self.pin + 16)) };
        Ok(())
    }

    fn toggle(&mut self) -> Result<(), Self::Error> {
        // SAFETY: lee ODR y emite el set/reset correspondiente por BSRR (sin
        // ventana RMW sobre ODR). Cooperativo, single-thread.
        unsafe {
            let on = read_reg(self.port, ODR) & (1 << self.pin) != 0;
            let bit = if on {
                1 << (self.pin + 16)
            } else {
                1 << self.pin
            };
            write_reg(self.port, BSRR, bit);
        }
        Ok(())
    }

    fn is_high(&self) -> Result<bool, Self::Error> {
        // SAFETY: lectura atómica de IDR.
        let level = unsafe { read_reg(self.port, IDR) & (1 << self.pin) != 0 };
        Ok(level)
    }
}

#[inline]
unsafe fn read_reg(port: Port, off: u32) -> u32 {
    unsafe { read_volatile((port.base() + off) as *const u32) }
}

#[inline]
unsafe fn write_reg(port: Port, off: u32, val: u32) {
    unsafe { write_volatile((port.base() + off) as *mut u32, val) }
}

#[inline]
unsafe fn modify_reg(port: Port, off: u32, clear: u32, set: u32) {
    unsafe {
        let v = read_reg(port, off);
        write_reg(port, off, (v & !clear) | set);
    }
}

fn enable_port_clock(port: Port) {
    // SAFETY: RMW sobre AHB1ENR; cada placa habilita relojes en arranque
    // single-thread, antes de lanzar tareas.
    unsafe {
        let v = read_volatile(RCC_AHB1ENR as *const u32);
        write_volatile(RCC_AHB1ENR as *mut u32, v | (1 << port.index()));
        let _ = read_volatile(RCC_AHB1ENR as *const u32); // barrera tras enable
    }
}

fn configure(port: Port, pin: u8, cfg: PinConfig) {
    let p = pin as u32;
    let two = p * 2;
    let (mode_bits, af): (u32, Option<u8>) = match cfg.mode {
        Mode::Input => (0b00, None),
        Mode::Output => (0b01, None),
        Mode::Alternate(af) => (0b10, Some(af)),
        Mode::Analog => (0b11, None),
    };
    let pupd = match cfg.pull {
        Pull::None => 0b00,
        Pull::Up => 0b01,
        Pull::Down => 0b10,
    };
    let ospeed = match cfg.speed {
        Speed::Low => 0b00,
        Speed::Medium => 0b01,
        Speed::High => 0b10,
        Speed::VeryHigh => 0b11,
    };
    // SAFETY: solo tocamos los campos del pin que poseemos en este puerto.
    unsafe {
        if let Some(af) = af {
            let af = (af as u32) & 0xF;
            if p < 8 {
                modify_reg(port, AFRL, 0xF << (p * 4), af << (p * 4));
            } else {
                let s = (p - 8) * 4;
                modify_reg(port, AFRH, 0xF << s, af << s);
            }
        }
        match cfg.otype {
            OutputType::PushPull => modify_reg(port, OTYPER, 1 << p, 0),
            OutputType::OpenDrain => modify_reg(port, OTYPER, 1 << p, 1 << p),
        }
        modify_reg(port, OSPEEDR, 0b11 << two, ospeed << two);
        modify_reg(port, PUPDR, 0b11 << two, pupd << two);
        modify_reg(port, MODER, 0b11 << two, mode_bits << two);
    }
}

/// User LEDs en STM32F407G-DISC1 (todos en GPIOD, activos en alto).
#[derive(Clone, Copy, Debug)]
pub enum DiscoLed {
    /// LD4, verde, PD12.
    Green,
    /// LD3, naranja, PD13.
    Orange,
    /// LD5, rojo, PD14.
    Red,
    /// LD6, azul, PD15.
    Blue,
}

impl DiscoLed {
    #[inline]
    fn pin(self) -> u8 {
        match self {
            DiscoLed::Green => 12,
            DiscoLed::Orange => 13,
            DiscoLed::Red => 14,
            DiscoLed::Blue => 15,
        }
    }
}

/// Handle de un LED, salida push-pull sobre [`Pin`].
pub struct LedPin {
    pin: Pin,
}

impl LedPin {
    /// Crea el handle y configura el pin como salida push-pull.
    ///
    /// `_rcc` se conserva por compatibilidad de API; el reloj del puerto lo
    /// habilita [`Pin::new`] vía AHB1ENR.
    pub fn new(_rcc: &pac::RCC, led: DiscoLed) -> Self {
        Self {
            pin: Pin::new(Port::D, led.pin(), PinConfig::output()),
        }
    }
}

impl GpioPin for LedPin {
    type Error = core::convert::Infallible;

    fn set_high(&mut self) -> Result<(), Self::Error> {
        self.pin.set_high()
    }
    fn set_low(&mut self) -> Result<(), Self::Error> {
        self.pin.set_low()
    }
    fn toggle(&mut self) -> Result<(), Self::Error> {
        self.pin.toggle()
    }
    fn is_high(&self) -> Result<bool, Self::Error> {
        self.pin.is_high()
    }
}
