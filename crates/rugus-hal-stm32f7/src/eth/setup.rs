//! RMII pin mux and ETH peripheral clock/reset for STM32F769I-DISCO.

use crate::pac;

const AF11: u32 = 0b1011;
const OSPEED_VERY_HIGH: u32 = 0b11;

/// PHY address of the onboard LAN8742A (UM2033).
pub const LAN8742_PHY_ADDR: u8 = 0;

/// Enable SYSCFG + ETH MAC/TX/RX clocks, select RMII, pulse reset.
pub fn enable_peripheral() {
    cortex_m::interrupt::free(|_| unsafe {
        let rcc = &*pac::RCC::ptr();
        let syscfg = &*pac::SYSCFG::ptr();

        rcc.apb2enr.modify(|_, w| w.syscfgen().set_bit());

        if rcc.ahb1enr.read().ethmacen().bit_is_set() {
            rcc.ahb1enr.modify(|_, w| w.ethmacen().clear_bit());
        }

        syscfg.pmc.modify(|_, w| w.mii_rmii_sel().set_bit());

        rcc.ahb1enr.modify(|_, w| {
            w.ethmacen().set_bit();
            w.ethmactxen().set_bit();
            w.ethmacrxen().set_bit()
        });

        rcc.ahb1rstr.modify(|_, w| w.ethmacrst().set_bit());
        rcc.ahb1rstr.modify(|_, w| w.ethmacrst().clear_bit());
    });
}

/// Configure RMII + MDIO/MDC pins on the F769I-DISCO (PB11/PB12 TX, not Nucleo PG11).
pub fn configure_disco_pins(dp: &pac::Peripherals) {
    let rcc = &dp.RCC;
    rcc.ahb1enr.modify(|_, w| {
        w.gpioaen().set_bit();
        w.gpioben().set_bit();
        w.gpiocen().set_bit()
    });
    let _ = rcc.ahb1enr.read().bits();

    // RMII inputs: REF_CLK PA1, CRS_DV PA7, RXD0 PC4, RXD1 PC5
    unsafe {
        input_floating(pac::GPIOA::ptr() as *const GpioBlock, &[1, 7]);
        input_floating(pac::GPIOC::ptr() as *const GpioBlock, &[4, 5]);
        af_very_high(pac::GPIOB::ptr() as *const GpioBlock, &[11, 12, 13], AF11);
        af_very_high(pac::GPIOA::ptr() as *const GpioBlock, &[2], AF11);
        af_very_high(pac::GPIOC::ptr() as *const GpioBlock, &[1], AF11);
    }
}

/// All GPIO ports share the same register layout on STM32F7.
type GpioBlock = pac::gpioa::RegisterBlock;

unsafe fn input_floating(port: *const GpioBlock, pins: &[u8]) {
    // SAFETY: port is a valid GPIO register block from PAC::ptr().
    let gpio = unsafe { &*port };
    for pin in pins {
        let bit = *pin as u32;
        let shift = bit * 2;
        gpio.moder
            .modify(|r, w| unsafe { w.bits(r.bits() & !(0b11 << shift)) });
        gpio.pupdr
            .modify(|r, w| unsafe { w.bits(r.bits() & !(0b11 << shift)) });
    }
}

unsafe fn af_very_high(port: *const GpioBlock, pins: &[u8], af: u32) {
    // SAFETY: port is a valid GPIO register block from PAC::ptr().
    let gpio = unsafe { &*port };
    for pin in pins {
        let bit = *pin as u32;
        let shift = bit * 2;
        gpio.moder
            .modify(|r, w| unsafe { w.bits((r.bits() & !(0b11 << shift)) | (0b10 << shift)) });
        gpio.otyper
            .modify(|r, w| unsafe { w.bits(r.bits() & !(1 << bit)) });
        gpio.ospeedr.modify(|r, w| unsafe {
            w.bits((r.bits() & !(0b11 << shift)) | (OSPEED_VERY_HIGH << shift))
        });
        gpio.pupdr
            .modify(|r, w| unsafe { w.bits(r.bits() & !(0b11 << shift)) });
        let afr_shift = (bit % 8) * 4;
        if bit < 8 {
            gpio.afrl.modify(|r, w| unsafe {
                w.bits((r.bits() & !(0xF << afr_shift)) | (af << afr_shift))
            });
        } else {
            gpio.afrh.modify(|r, w| unsafe {
                w.bits((r.bits() & !(0xF << afr_shift)) | (af << afr_shift))
            });
        }
    }
}

/// Marker: MDIO configured in [`configure_disco_pins`].
pub struct Mdio;

/// Marker: MDC configured in [`configure_disco_pins`].
pub struct Mdc;

/// Ethernet peripheral parts from PAC.
pub struct PartsIn {
    /// MAC registers.
    pub mac: pac::ETHERNET_MAC,
    /// MMC registers.
    pub mmc: pac::ETHERNET_MMC,
    /// DMA registers.
    pub dma: pac::ETHERNET_DMA,
}

impl PartsIn {
    /// Bundle ETH blocks from [`pac::Peripherals`].
    pub const fn new(
        mac: pac::ETHERNET_MAC,
        mmc: pac::ETHERNET_MMC,
        dma: pac::ETHERNET_DMA,
    ) -> Self {
        Self { mac, mmc, dma }
    }
}
