//! I2C1 master polling — PB6 SCL, PB7 SDA @ 100 kHz (HSI 8 MHz).

use crate::pac;

/// Bus I2C1 del appliance (sensores).
pub struct I2c1 {
    i2c: pac::I2C1,
}

impl I2c1 {
    /// Inicializa I2C1 en modo master 100 kHz.
    pub fn new(rcc: &pac::RCC, i2c: pac::I2C1) -> Self {
        rcc.apb2enr.modify(|_, w| w.iopben().set_bit());
        rcc.apb1enr.modify(|_, w| w.i2c1en().set_bit());
        let _ = rcc.apb2enr.read().bits();
        let _ = rcc.apb1enr.read().bits();

        // PB6/PB7: AF open-drain. CRL bits 24-31 for pin 6,7.
        // CNF=10 AF OD, MODE=11 50MHz → 0b1011
        const AF_OD: u32 = 0b1011;
        // SAFETY: solo CRL PB6/PB7.
        unsafe {
            let g = &*pac::GPIOB::ptr();
            g.crl.modify(|r, w| {
                let mut v = r.bits();
                v = (v & !(0xF << 24)) | (AF_OD << 24);
                v = (v & !(0xF << 28)) | (AF_OD << 28);
                w.bits(v)
            });
        }

        i2c.cr1.write(|w| w.swrst().set_bit());
        i2c.cr1.write(|w| w.swrst().clear_bit());
        // CCR for 100kHz @ 8MHz PCLK1: 8000000/(2*100000)=40 → 0x28
        i2c.cr2.write(|w| unsafe { w.freq().bits(8) });
        i2c.ccr.write(|w| unsafe { w.ccr().bits(40) });
        i2c.trise.write(|w| w.trise().bits(9));
        i2c.cr1.write(|w| w.pe().set_bit());

        Self { i2c }
    }

    /// Escaneo 7-bit: retorna direcciones ACK en `found` (max `found.len()`).
    pub fn scan(&mut self, found: &mut [u8]) -> usize {
        let mut n = 0;
        for addr in 0x08u8..=0x77 {
            if self.probe(addr) && n < found.len() {
                found[n] = addr;
                n += 1;
            }
        }
        n
    }

    fn probe(&mut self, addr: u8) -> bool {
        self.start();
        let ok = self.write_addr(addr << 1).is_ok();
        self.stop();
        ok
    }

    fn start(&mut self) {
        self.i2c.cr1.modify(|_, w| w.start().set_bit());
        while !self.i2c.sr1.read().sb().bit() {}
    }

    fn stop(&mut self) {
        self.i2c.cr1.modify(|_, w| w.stop().set_bit());
    }

    fn write_addr(&mut self, byte: u8) -> Result<(), ()> {
        self.i2c.dr.write(|w| w.dr().bits(byte));
        self.wait_addr();
        if self.i2c.sr1.read().af().bit() {
            self.i2c.sr1.modify(|_, w| w.af().clear_bit());
            return Err(());
        }
        Ok(())
    }

    fn wait_addr(&mut self) {
        while !self.i2c.sr1.read().addr().bit() {}
        let _ = self.i2c.sr1.read().bits();
        let _ = self.i2c.sr2.read().bits();
    }
}
