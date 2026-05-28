//! USART2 — PA2 TX, PA3 RX @ 115200 (bus de módulos LoRa/BLE).

use crate::pac;
use rugus_hal::SerialPort;

/// Baud rate del bus de módulos.
pub const MODULE_BAUD: u32 = 115_200;

/// Error UART módulos.
pub type UartError = core::convert::Infallible;

/// Handle bloqueante USART2 en PA2/PA3.
pub struct Usart2 {
    usart: pac::USART2,
}

impl Usart2 {
    /// Inicializa USART2: PA2 TX, PA3 RX, 8N1 @ `baud`.
    pub fn new(rcc: &pac::RCC, usart: pac::USART2, pclk1: u32, baud: u32) -> Self {
        rcc.apb2enr.modify(|_, w| w.iopaen().set_bit());
        rcc.apb1enr.modify(|_, w| w.usart2en().set_bit());
        let _ = rcc.apb2enr.read().bits();
        let _ = rcc.apb1enr.read().bits();

        // PA2/PA3 in CRL: pin2 bits 8-11, pin3 bits 12-15.
        const TX: u32 = 0b1011; // AF push-pull
        const RX: u32 = 0b0100; // floating in
                                // SAFETY: solo CRL PA2/PA3.
        unsafe {
            let g = &*pac::GPIOA::ptr();
            g.crl.modify(|r, w| {
                let mut v = r.bits();
                v = (v & !(0xF << 8)) | (TX << 8);
                v = (v & !(0xF << 12)) | (RX << 12);
                w.bits(v)
            });
        }

        configure_usart(&usart, pclk1, baud);
        Self { usart }
    }

    /// Escribe AT probe y retorna true si hay eco RX (módulo presente).
    pub fn probe_module(&mut self) -> bool {
        let _ = self.write(b"AT\r\n");
        cortex_m::asm::delay(800_000);
        let mut buf = [0u8; 4];
        let mut got = 0;
        for _ in 0..1000 {
            if self.usart.sr.read().rxne().bit() {
                buf[got] = self.usart.dr.read().dr().bits() as u8;
                got += 1;
                if got >= 2 {
                    return buf[0] == b'A' || buf[0] == b'O' || buf[1] == b'K';
                }
            }
        }
        false
    }
}

impl SerialPort for Usart2 {
    type Error = UartError;

    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        for &b in buf {
            while !self.usart.sr.read().txe().bit() {}
            self.usart.dr.write(|w| w.dr().bits(u16::from(b)));
        }
        Ok(buf.len())
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        while !self.usart.sr.read().rxne().bit() {}
        buf[0] = self.usart.dr.read().dr().bits() as u8;
        Ok(1)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        while !self.usart.sr.read().tc().bit() {}
        Ok(())
    }
}

fn configure_usart(usart: &pac::USART2, pclk: u32, baud: u32) {
    usart.cr1.write(|w| w.ue().clear_bit());
    let div = (pclk + baud / 2) / baud;
    usart
        .brr
        .write(|w| unsafe { w.bits((div / 16) << 4 | (div % 16)) });
    usart.cr1.write(|w| {
        w.te().set_bit();
        w.re().set_bit();
        w.ue().set_bit()
    });
}
