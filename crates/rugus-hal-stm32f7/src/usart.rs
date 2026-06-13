//! USART para STM32F7 — consolas polled 8N1 sobre acceso MMIO directo.
//!
//! Dos instancias cableadas a los transportes de la F769I-DISCO:
//!
//! - [`Usart2`] — **PA2 TX / PA3 RX**, APB1. Consola por USB-TTL externo. OJO:
//!   en la F769I-DISCO PA2 es además la línea **MDIO** del PHY Ethernet, así que
//!   solo es usable en ejemplos sin red.
//! - [`Usart1`] — **PA9 TX / PA10 RX**, APB2. Es el UART cableado al **Virtual
//!   COM Port del ST-Link** (CN1): la consola sale por `/dev/ttyACM*` sin TTL
//!   externo. Pines dedicados al ST-Link, sin conflicto con el Ethernet.
//!
//! El mapa de registros es el de la IP USART de la familia F7 (CR1/CR2/CR3/BRR
//! al principio, e **ISR/RDR/TDR** en lugar del SR/DR único de F4). Las rutinas
//! de bajo nivel toman la base del periférico como parámetro, de modo que ambas
//! instancias comparten el cuerpo sin duplicarlo.
//!
//! [`Usart2`] incluye un autotest de lazo cerrado por **half-duplex single-wire**
//! (`CR3.HDSEL`): transmitir reinyecta en el receptor, validando el periférico
//! sin cablear pines (regla de validación por RTT).

use crate::gpio::{Mode, OutputType, Pin, PinConfig, Port, Pull, Speed};
use core::ptr::{read_volatile, write_volatile};
use rugus_hal::SerialPort;

/// Base de USART1 (APB2) en la familia STM32F7.
const USART1_BASE: u32 = 0x4001_1000;
/// Base de USART2 (APB1) en la familia STM32F7.
const USART2_BASE: u32 = 0x4000_4400;

/// `RCC->APB1ENR`: bit 17 habilita el reloj de USART2.
const RCC_APB1ENR: u32 = 0x4002_3840;
const USART2EN: u32 = 1 << 17;
/// `RCC->APB2ENR`: bit 4 habilita el reloj de USART1.
const RCC_APB2ENR: u32 = 0x4002_3844;
const USART1EN: u32 = 1 << 4;

// Offsets de registro dentro del bloque USART (F7).
const CR1: u32 = 0x00;
const CR3: u32 = 0x08;
const BRR: u32 = 0x0C;
const ISR: u32 = 0x1C;
const RDR: u32 = 0x24;
const TDR: u32 = 0x28;

// Bits de ISR.
const ISR_RXNE: u32 = 1 << 5;
const ISR_TC: u32 = 1 << 6;
const ISR_TXE: u32 = 1 << 7;

// Bits de CR1 (F7: UE=bit0, RE=bit2, TE=bit3) y CR3.
const CR1_UE: u32 = 1 << 0;
const CR1_RE: u32 = 1 << 2;
const CR1_TE: u32 = 1 << 3;
const CR1_RXNEIE: u32 = 1 << 5;
const CR3_HDSEL: u32 = 1 << 3;

/// AF7 = USART1/2/3 en la familia F7.
const AF_USART: u8 = 7;

/// Baud por defecto de la consola.
pub const CONSOLE_BAUD: u32 = 115_200;

/// Error de USART (infallible en las operaciones bloqueantes actuales).
pub type UartError = core::convert::Infallible;

// --- Cuerpo de bajo nivel parametrizado por base del periférico. ---

#[inline]
unsafe fn read_reg(base: u32, off: u32) -> u32 {
    unsafe { read_volatile((base + off) as *const u32) }
}

#[inline]
unsafe fn write_reg(base: u32, off: u32, val: u32) {
    unsafe { write_volatile((base + off) as *mut u32, val) }
}

/// Escribe un byte por `base` (polling TXE).
fn write_byte_at(base: u32, b: u8) {
    // SAFETY: registros MMIO del USART; espera a TDR vacío antes de escribir.
    unsafe {
        while read_reg(base, ISR) & ISR_TXE == 0 {}
        write_reg(base, TDR, b as u32);
    }
}

/// Saca un byte recibido por `base` si hay (RXNE), sin bloquear.
fn try_read_at(base: u32) -> Option<u8> {
    // SAFETY: leer RDR limpia RXNE.
    unsafe {
        if read_reg(base, ISR) & ISR_RXNE != 0 {
            Some((read_reg(base, RDR) & 0xFF) as u8)
        } else {
            None
        }
    }
}

/// Habilita reloj y programa el USART en `base`. `rcc_enr`/`en_bit` seleccionan
/// el bit de reloj del bus correspondiente (APB1 para USART2, APB2 para USART1).
fn configure_at(base: u32, rcc_enr: u32, en_bit: u32, pclk: u32, baud: u32, loopback: bool) {
    // SAFETY: arranque single-thread; habilita reloj y programa el USART.
    unsafe {
        let v = read_volatile(rcc_enr as *const u32);
        write_volatile(rcc_enr as *mut u32, v | en_bit);
        let _ = read_volatile(rcc_enr as *const u32);

        write_reg(base, CR1, 0); // UE=0 mientras configuramos.
                                 // F7: con OVER8=0 el BRR es directamente el divisor.
        write_reg(base, BRR, (pclk + baud / 2) / baud);
        write_reg(base, CR3, if loopback { CR3_HDSEL } else { 0 });
        write_reg(base, CR1, CR1_UE | CR1_TE | CR1_RE);
    }
}

// ===================== USART2 (PA2/PA3, APB1) =====================

/// Handle polled de USART2 (PA2 TX, PA3 RX). Consola por USB-TTL externo.
pub struct Usart2 {
    _tx: Pin,
    _rx: Option<Pin>,
}

impl Usart2 {
    /// Consola normal: PA2 TX push-pull + PA3 RX, 8N1 @ `baud` con `pclk1` Hz.
    pub fn new(pclk1: u32, baud: u32) -> Self {
        let tx = Pin::new(
            Port::A,
            2,
            PinConfig {
                mode: Mode::Alternate(AF_USART),
                pull: Pull::None,
                speed: Speed::High,
                otype: OutputType::PushPull,
            },
        );
        let rx = Pin::new(
            Port::A,
            3,
            PinConfig {
                mode: Mode::Alternate(AF_USART),
                pull: Pull::Up,
                speed: Speed::High,
                otype: OutputType::PushPull,
            },
        );
        configure_at(USART2_BASE, RCC_APB1ENR, USART2EN, pclk1, baud, false);
        Self {
            _tx: tx,
            _rx: Some(rx),
        }
    }

    /// Autotest single-wire: PA2 en AF open-drain con `CR3.HDSEL`. Transmitir
    /// reinyecta en el receptor sin cablear pines.
    pub fn new_loopback(pclk1: u32, baud: u32) -> Self {
        let tx = Pin::new(
            Port::A,
            2,
            PinConfig {
                mode: Mode::Alternate(AF_USART),
                pull: Pull::Up,
                speed: Speed::High,
                otype: OutputType::OpenDrain,
            },
        );
        configure_at(USART2_BASE, RCC_APB1ENR, USART2EN, pclk1, baud, true);
        Self { _tx: tx, _rx: None }
    }

    /// Escribe un byte (polling TXE).
    pub fn write_byte(&mut self, b: u8) {
        write_byte_at(USART2_BASE, b);
    }

    /// Saca un byte recibido si hay (RXNE), sin bloquear.
    pub fn try_read_byte(&mut self) -> Option<u8> {
        try_read_at(USART2_BASE)
    }

    /// Lee un byte esperando hasta `spins` iteraciones; `None` si no llega.
    pub fn read_byte_timeout(&mut self, spins: u32) -> Option<u8> {
        for _ in 0..spins {
            if let Some(b) = self.try_read_byte() {
                return Some(b);
            }
        }
        None
    }

    /// Habilita la interrupción de recepción (RXNEIE): cada byte recibido pende
    /// la IRQ de USART2. El firmware debe desenmascarar la línea en el NVIC y
    /// drenar los bytes en el handler con [`isr_read_byte`].
    pub fn enable_rx_irq(&mut self) {
        // SAFETY: registro CR1 de USART2; OR no destructivo del bit RXNEIE.
        unsafe {
            write_reg(USART2_BASE, CR1, read_reg(USART2_BASE, CR1) | CR1_RXNEIE);
        }
    }
}

/// Lee un byte recibido desde el handler de IRQ de USART2 (`#[interrupt] fn
/// USART2`): lee `RDR` si `RXNE` está alto (lo que limpia el flag y la pendiente
/// de la IRQ). `None` si la IRQ se disparó por otra causa.
pub fn isr_read_byte() -> Option<u8> {
    try_read_at(USART2_BASE)
}

impl SerialPort for Usart2 {
    type Error = UartError;

    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        for &b in buf {
            self.write_byte(b);
        }
        Ok(buf.len())
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if let Some(b) = self.try_read_byte() {
                buf[0] = b;
                return Ok(1);
            }
        }
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        // SAFETY: espera a transmisión completa (TC).
        unsafe { while read_reg(USART2_BASE, ISR) & ISR_TC == 0 {} }
        Ok(())
    }
}

// ===================== USART1 (PA9/PA10, APB2) =====================

/// Handle polled de USART1 (PA9 TX, PA10 RX). Es el UART cableado al **VCP del
/// ST-Link** en la F769I-DISCO: la consola sale por `/dev/ttyACM*` sin TTL.
pub struct Usart1 {
    _tx: Pin,
    _rx: Pin,
}

impl Usart1 {
    /// Consola por el VCP: PA9 TX push-pull + PA10 RX, 8N1 @ `baud` con `pclk2`.
    pub fn new(pclk2: u32, baud: u32) -> Self {
        let tx = Pin::new(
            Port::A,
            9,
            PinConfig {
                mode: Mode::Alternate(AF_USART),
                pull: Pull::None,
                speed: Speed::High,
                otype: OutputType::PushPull,
            },
        );
        let rx = Pin::new(
            Port::A,
            10,
            PinConfig {
                mode: Mode::Alternate(AF_USART),
                pull: Pull::Up,
                speed: Speed::High,
                otype: OutputType::PushPull,
            },
        );
        configure_at(USART1_BASE, RCC_APB2ENR, USART1EN, pclk2, baud, false);
        Self { _tx: tx, _rx: rx }
    }

    /// Escribe un byte (polling TXE).
    pub fn write_byte(&mut self, b: u8) {
        write_byte_at(USART1_BASE, b);
    }

    /// Saca un byte recibido si hay (RXNE), sin bloquear.
    pub fn try_read_byte(&mut self) -> Option<u8> {
        try_read_at(USART1_BASE)
    }

    /// Habilita la interrupción de recepción (RXNEIE): cada byte recibido pende
    /// la IRQ de USART1. El firmware desenmascara la línea en el NVIC y drena en
    /// el handler con [`isr_read_byte_usart1`].
    pub fn enable_rx_irq(&mut self) {
        // SAFETY: registro CR1 de USART1; OR no destructivo del bit RXNEIE.
        unsafe {
            write_reg(USART1_BASE, CR1, read_reg(USART1_BASE, CR1) | CR1_RXNEIE);
        }
    }
}

/// Lee un byte recibido desde el handler de IRQ de USART1 (`#[interrupt] fn
/// USART1`). `None` si la IRQ se disparó por otra causa.
pub fn isr_read_byte_usart1() -> Option<u8> {
    try_read_at(USART1_BASE)
}

impl SerialPort for Usart1 {
    type Error = UartError;

    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        for &b in buf {
            self.write_byte(b);
        }
        Ok(buf.len())
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if let Some(b) = self.try_read_byte() {
                buf[0] = b;
                return Ok(1);
            }
        }
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        // SAFETY: espera a transmisión completa (TC).
        unsafe { while read_reg(USART1_BASE, ISR) & ISR_TC == 0 {} }
        Ok(())
    }
}
