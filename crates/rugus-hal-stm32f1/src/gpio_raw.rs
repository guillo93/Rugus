//! GPIO genérico por puerto/pin — para syscalls lite del appliance.

use crate::pac;

/// Lee nivel lógico de un pin (IDR). Retorna 0/1.
pub fn read(port: u8, pin: u8) -> Option<u8> {
    if pin > 15 {
        return None;
    }
    let ptr = port_ptr(port)?;
    // SAFETY: ptr válido para puerto GPIO del F103.
    let level = unsafe {
        let g = &*ptr;
        if g.idr.read().bits() & (1 << pin) != 0 {
            1
        } else {
            0
        }
    };
    Some(level)
}

/// Escribe nivel lógico vía BSRR (pin debe estar configurado como salida).
pub fn write(port: u8, pin: u8, high: bool) -> Option<()> {
    if pin > 15 {
        return None;
    }
    let ptr = port_ptr(port)?;
    // SAFETY: BSRR write-only, atómico por bit.
    unsafe {
        let g = &*ptr;
        g.bsrr
            .write(|w| w.bits(if high { 1 << pin } else { 1 << (pin + 16) }));
    }
    Some(())
}

/// Invierte bit en ODR.
pub fn toggle(port: u8, pin: u8) -> Option<()> {
    if pin > 15 {
        return None;
    }
    let ptr = port_ptr(port)?;
    // SAFETY: RMW en ODR single bit.
    unsafe {
        let g = &*ptr;
        g.odr.modify(|r, w| w.bits(r.bits() ^ (1 << pin)));
    }
    Some(())
}

/// Habilita reloj APB2/APB1 y configura pin como salida push-pull 2 MHz.
pub fn configure_output(rcc: &pac::RCC, port: u8, pin: u8) -> Option<()> {
    if pin > 15 {
        return None;
    }
    enable_port_clock(rcc, port)?;
    let ptr = port_ptr(port)?;
    let (reg, shift) = if pin < 8 {
        (0, pin as u32 * 4)
    } else {
        (1, (pin - 8) as u32 * 4)
    };
    const OUT_PP_2MHZ: u32 = 0b10;
    // SAFETY: solo nibble del pin en CRL/CRH.
    unsafe {
        let g = &*ptr;
        if reg == 0 {
            g.crl.modify(|r, w| {
                w.bits((r.bits() & !(0xF << shift)) | (OUT_PP_2MHZ << shift))
            });
        } else {
            g.crh.modify(|r, w| {
                w.bits((r.bits() & !(0xF << shift)) | (OUT_PP_2MHZ << shift))
            });
        }
    }
    Some(())
}

fn port_ptr(port: u8) -> Option<*const pac::gpioa::RegisterBlock> {
    match port {
        b'A' => Some(pac::GPIOA::ptr()),
        b'B' => Some(pac::GPIOB::ptr()),
        b'C' => Some(pac::GPIOC::ptr()),
        b'D' => Some(pac::GPIOD::ptr()),
        _ => None,
    }
}

fn enable_port_clock(rcc: &pac::RCC, port: u8) -> Option<()> {
    match port {
        b'A' => rcc.apb2enr.modify(|_, w| w.iopaen().set_bit()),
        b'B' => rcc.apb2enr.modify(|_, w| w.iopben().set_bit()),
        b'C' => rcc.apb2enr.modify(|_, w| w.iopcen().set_bit()),
        b'D' => rcc.apb2enr.modify(|_, w| w.iopden().set_bit()),
        _ => return None,
    }
    let _ = rcc.apb2enr.read().bits();
    Some(())
}
