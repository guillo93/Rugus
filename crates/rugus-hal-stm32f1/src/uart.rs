//! USART1 — PA9 TX, PA10 RX @ 115200 (rugus-cli console).

use crate::pac;
use rugus_hal::SerialPort;

/// Baud rate por defecto de la consola rugus-cli.
pub const CLI_BAUD: u32 = 115_200;

/// Error de UART (infallible en operaciones bloqueantes actuales).
pub type UartError = core::convert::Infallible;

/// Handle bloqueante para USART1 en PA9/PA10.
pub struct Usart1 {
    usart: pac::USART1,
}

impl Usart1 {
    /// Inicializa USART1: PA9 TX, PA10 RX, 8N1 @ `baud` con `pclk2` Hz.
    pub fn new(rcc: &pac::RCC, usart: pac::USART1, pclk2: u32, baud: u32) -> Self {
        enable_clocks(rcc);
        configure_pins();
        configure_usart(&usart, pclk2, baud);
        Self { usart }
    }

    /// Lee byte si RXNE; no bloquea.
    pub fn try_read_byte(&mut self) -> Option<u8> {
        if self.usart.sr.read().rxne().bit() {
            Some(self.read_byte())
        } else {
            None
        }
    }

    /// Escribe un byte (polling TXE).
    pub fn write_byte(&mut self, b: u8) {
        while !self.usart.sr.read().txe().bit() {}
        // SAFETY: TXE indica DR vacío.
        self.usart.dr.write(|w| w.dr().bits(u16::from(b)));
    }

    /// Lee un byte (polling RXNE). Bloquea hasta recibir.
    pub fn read_byte(&mut self) -> u8 {
        while !self.usart.sr.read().rxne().bit() {}
        self.usart.dr.read().dr().bits() as u8
    }
}

impl SerialPort for Usart1 {
    type Error = UartError;

    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        for (i, &b) in buf.iter().enumerate() {
            self.write_byte(b);
            if i + 1 == buf.len() {
                break;
            }
        }
        Ok(buf.len())
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        buf[0] = self.read_byte();
        Ok(1)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        while !self.usart.sr.read().tc().bit() {}
        Ok(())
    }
}

fn enable_clocks(rcc: &pac::RCC) {
    rcc.apb2enr.modify(|_, w| {
        w.iopaen().set_bit();
        w.usart1en().set_bit()
    });
    let _ = rcc.apb2enr.read().bits();
}

fn configure_pins() {
    // PA9: AF push-pull TX, PA10: floating input RX.
    // CRH: PA8-PA15. PA9 nibble bits 4-7, PA10 bits 8-11.
    // TX: CNF=10 (AF push-pull), MODE=11 (50 MHz) → 0b1011
    // RX: CNF=01 (floating in), MODE=00 → 0b0100
    const TX_NIBBLE: u32 = 0b1011;
    const RX_NIBBLE: u32 = 0b0100;
    // SAFETY: solo GPIOA CRH para PA9/PA10.
    unsafe {
        let g = &*pac::GPIOA::ptr();
        g.crh.modify(|r, w| {
            let mut v = r.bits();
            v = (v & !(0xF << 4)) | (TX_NIBBLE << 4);
            v = (v & !(0xF << 8)) | (RX_NIBBLE << 8);
            w.bits(v)
        });
    }
}

fn configure_usart(usart: &pac::USART1, pclk2: u32, baud: u32) {
    usart.cr1.write(|w| w.ue().clear_bit());
    let div = (pclk2 + baud / 2) / baud;
    let mantissa = div / 16;
    let fraction = div % 16;
    usart
        .brr
        .write(|w| unsafe { w.bits(mantissa << 4 | fraction) });
    usart.cr1.write(|w| {
        w.te().set_bit();
        w.re().set_bit();
        w.ue().set_bit()
    });
}
