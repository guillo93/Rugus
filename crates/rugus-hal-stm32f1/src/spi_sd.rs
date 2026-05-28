//! SPI1 + SD card mínima — PA4 NSS, PA5 SCK, PA6 MISO, PA7 MOSI.

use crate::pac;

/// Estado del slot SD.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SdStatus {
    /// Sin tarjeta o init fallido.
    Absent,
    /// Tarjeta lista para lectura sector 0.
    Ready,
}

/// Driver SPI1 bloqueante para SD.
pub struct Spi1Sd {
    spi: pac::SPI1,
    status: SdStatus,
}

impl Spi1Sd {
    /// Inicializa SPI1 y intenta init SD.
    pub fn new(rcc: &pac::RCC, spi: pac::SPI1) -> Self {
        enable_spi(rcc, &spi);
        let mut sd = Self {
            spi,
            status: SdStatus::Absent,
        };
        if sd.init_sd() {
            sd.status = SdStatus::Ready;
        }
        sd
    }

    /// Estado actual.
    pub fn status(&self) -> SdStatus {
        self.status
    }

    /// Lee hasta `buf.len()` bytes del sector 0 (offset 0). Retorna bytes leídos.
    pub fn read_boot_sector(&mut self, buf: &mut [u8]) -> usize {
        if self.status != SdStatus::Ready {
            return 0;
        }
        let to_read = buf.len().min(512);
        if self.read_sector(0, &mut buf[..to_read]).is_ok() {
            to_read
        } else {
            0
        }
    }

    fn init_sd(&mut self) -> bool {
        self.cs_high();
        for _ in 0..10 {
            self.transfer(0xFF);
        }
        self.cs_low();
        if self.transfer(0x40) != 0x01 {
            self.cs_high();
            return false;
        }
        for _ in 0..4 {
            self.transfer(0x00);
        }
        self.transfer(0x95);
        let mut timeout = 1000;
        while timeout > 0 {
            if self.transfer(0xFF) == 0x00 {
                break;
            }
            timeout -= 1;
        }
        self.cs_high();
        self.transfer(0xFF);
        timeout > 0
    }

    fn read_sector(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), ()> {
        self.cs_low();
        self.transfer(0x51);
        self.transfer((lba >> 24) as u8);
        self.transfer((lba >> 16) as u8);
        self.transfer((lba >> 8) as u8);
        self.transfer(lba as u8);
        self.transfer(0xFF);
        let mut t = 10000;
        while t > 0 {
            if self.transfer(0xFF) == 0x00 {
                break;
            }
            t -= 1;
        }
        if t == 0 {
            self.cs_high();
            return Err(());
        }
        for b in buf.iter_mut() {
            *b = self.transfer(0xFF);
        }
        self.transfer(0xFF);
        self.transfer(0xFF);
        self.cs_high();
        Ok(())
    }

    fn cs_low(&mut self) {
        // SAFETY: PA4 NSS manual via BSRR.
        unsafe {
            let g = &*pac::GPIOA::ptr();
            g.bsrr.write(|w| w.bits(1 << (4 + 16)));
        }
    }

    fn cs_high(&mut self) {
        unsafe {
            let g = &*pac::GPIOA::ptr();
            g.bsrr.write(|w| w.bits(1 << 4));
        }
    }

    fn transfer(&mut self, byte: u8) -> u8 {
        let mut t = 200_000;
        while !self.spi.sr.read().txe().bit() {
            t -= 1;
            if t == 0 {
                return 0xFF;
            }
        }
        self.spi.dr.write(|w| w.dr().bits(u16::from(byte)));
        t = 200_000;
        while !self.spi.sr.read().rxne().bit() {
            t -= 1;
            if t == 0 {
                return 0xFF;
            }
        }
        self.spi.dr.read().dr().bits() as u8
    }
}

fn enable_spi(rcc: &pac::RCC, spi: &pac::SPI1) {
    rcc.apb2enr.modify(|_, w| {
        w.iopaen().set_bit();
        w.spi1en().set_bit()
    });
    let _ = rcc.apb2enr.read().bits();

    // PA5 SCK, PA6 MISO, PA7 MOSI AF push-pull; PA4 NSS GPIO out high.
    const AF: u32 = 0b1011;
    const OUT: u32 = 0b0011;
    unsafe {
        let g = &*pac::GPIOA::ptr();
        g.crl.modify(|r, w| {
            let mut v = r.bits();
            v = (v & !(0xF << 16)) | (OUT << 16); // PA4 NSS GPIO
            v = (v & !(0xF << 20)) | (AF << 20);
            v = (v & !(0xF << 24)) | (AF << 24);
            v = (v & !(0xF << 28)) | (AF << 28);
            w.bits(v)
        });
        g.bsrr.write(|w| w.bits(1 << 4)); // NSS high
    }

    spi.cr1.write(|w| {
        w.mstr().set_bit();
        w.br().bits(0b111); // /256 slow for bring-up
        w.spe().set_bit()
    });
}
