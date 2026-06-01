//! USART2 para STM32F4 — PA2 TX / PA3 RX, polled, 8N1.
//!
//! Driver de consola bloqueante por acceso MMIO directo, en la misma línea que
//! [`crate::gpio`]: el bloque USART de la familia F4 expone SR/DR/BRR/CR1-3 en
//! offsets fijos. USART2 cuelga de APB1; en la STM32F407G-DISC1 PA2/PA3 están
//! libres en los headers (la placa no tiene VCP USB-serie en placa).
//!
//! Incluye un autotest de lazo cerrado por **half-duplex single-wire**
//! (`CR3.HDSEL`): el receptor queda atado internamente a la línea TX, así que
//! transmitir y volver a leer valida el periférico sin cablear pines —
//! suficiente para la regla de validación por RTT en ambas placas.

use crate::gpio::{Mode, OutputType, Pin, PinConfig, Port, Pull, Speed};
use core::ptr::{read_volatile, write_volatile};
use rugus_hal::SerialPort;

/// Base de USART2 (APB1) en la familia STM32F4.
const USART2_BASE: u32 = 0x4000_4400;
/// `RCC->APB1ENR`: bit 17 habilita el reloj de USART2.
const RCC_APB1ENR: u32 = 0x4002_3840;
const USART2EN: u32 = 1 << 17;

// Offsets de registro dentro del bloque USART (F4).
const SR: u32 = 0x00;
const DR: u32 = 0x04;
const BRR: u32 = 0x08;
const CR1: u32 = 0x0C;
const CR3: u32 = 0x14;

// Bits de SR.
const SR_TXE: u32 = 1 << 7;
const SR_TC: u32 = 1 << 6;
const SR_RXNE: u32 = 1 << 5;

// Bits de CR1 / CR3.
const CR1_UE: u32 = 1 << 13;
const CR1_TE: u32 = 1 << 3;
const CR1_RE: u32 = 1 << 2;
const CR3_HDSEL: u32 = 1 << 3;

/// AF7 = USART1/2/3 en la familia F4.
const AF_USART: u8 = 7;

/// Baud por defecto de la consola.
pub const CONSOLE_BAUD: u32 = 115_200;

/// Error de USART (infallible en las operaciones bloqueantes actuales).
pub type UartError = core::convert::Infallible;

/// Handle polled de USART2 (PA2 TX, PA3 RX).
pub struct Usart2 {
    // Los pines quedan configurados de por vida; el handle solo necesita la base.
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
        configure(pclk1, baud, false);
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
        configure(pclk1, baud, true);
        Self {
            _tx: tx,
            _rx: None,
        }
    }

    /// Escribe un byte (polling TXE).
    pub fn write_byte(&mut self, b: u8) {
        // SAFETY: registros MMIO de USART2; espera a DR vacío antes de escribir.
        unsafe {
            while read_reg(SR) & SR_TXE == 0 {}
            write_reg(DR, b as u32);
        }
    }

    /// Saca un byte recibido si hay (RXNE), sin bloquear.
    pub fn try_read_byte(&mut self) -> Option<u8> {
        // SAFETY: leer DR limpia RXNE.
        unsafe {
            if read_reg(SR) & SR_RXNE != 0 {
                Some((read_reg(DR) & 0xFF) as u8)
            } else {
                None
            }
        }
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
        unsafe {
            while read_reg(SR) & SR_TC == 0 {}
        }
        Ok(())
    }
}

#[inline]
unsafe fn read_reg(off: u32) -> u32 {
    unsafe { read_volatile((USART2_BASE + off) as *const u32) }
}

#[inline]
unsafe fn write_reg(off: u32, val: u32) {
    unsafe { write_volatile((USART2_BASE + off) as *mut u32, val) }
}

fn configure(pclk1: u32, baud: u32, loopback: bool) {
    // SAFETY: arranque single-thread; habilita reloj y programa USART2.
    unsafe {
        let v = read_volatile(RCC_APB1ENR as *const u32);
        write_volatile(RCC_APB1ENR as *mut u32, v | USART2EN);
        let _ = read_volatile(RCC_APB1ENR as *const u32);

        write_reg(CR1, 0); // UE=0 mientras configuramos.
        let div = (pclk1 + baud / 2) / baud;
        write_reg(BRR, ((div / 16) << 4) | (div % 16));
        write_reg(CR3, if loopback { CR3_HDSEL } else { 0 });
        write_reg(CR1, CR1_UE | CR1_TE | CR1_RE);
    }
}
