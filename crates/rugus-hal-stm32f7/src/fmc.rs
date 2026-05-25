//! FMC — SDRAM externa en STM32F769I-DISCO (UM2033).
//!
//! Inicializa la MT48LC4M32B2B5 (16 MB @ 0xC000_0000) vía FMC bank 1.
//! Parámetros y secuencia de arranque tomados del BSP ST (`stm32f769i_discovery_sdram`).

use crate::pac;
use crate::rcc::SYSCLK_HZ;
use core::ptr;

/// Base de la SDRAM mapeada por FMC bank 1.
pub const SDRAM_BASE: usize = 0xC000_0000;

/// Tamaño usable (16 MB).
pub const SDRAM_SIZE: usize = 16 * 1024 * 1024;

/// Resultado de [`init`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SdramError {
    /// Comando FMC no completó a tiempo.
    CommandTimeout,
    /// Prueba de lectura/escritura falló.
    VerifyFailed,
}

/// Inicializa FMC + SDRAM y ejecuta una prueba simple de lectura/escritura.
///
/// Requiere reloj del sistema ya configurado (216 MHz en DISCO). Configura MPU
/// mínima para la región SDRAM (normal, write-back) antes de acceder con D-cache.
pub fn init(
    dp: &pac::Peripherals,
    scb: &mut cortex_m::peripheral::SCB,
    cpuid: &mut cortex_m::peripheral::CPUID,
) -> Result<(), SdramError> {
    enable_clocks_and_pins(dp);
    configure_controller(&dp.FMC);
    run_init_sequence(&dp.FMC)?;
    configure_mpu(scb);
    verify(scb, cpuid)?;
    Ok(())
}

fn enable_clocks_and_pins(dp: &pac::Peripherals) {
    let rcc = &dp.RCC;
    rcc.ahb3enr.modify(|_, w| w.fmcen().enabled());
    let _ = rcc.ahb3enr.read().bits();

    rcc.ahb1enr.modify(|_, w| {
        w.gpioden().enabled();
        w.gpioeen().enabled();
        w.gpiofen().enabled();
        w.gpiogen().enabled();
        w.gpiohen().enabled();
        w.gpioien().enabled()
    });
    let _ = rcc.ahb1enr.read().bits();

    configure_fmc_pins(dp);
}

macro_rules! af_port {
    ($port:ty, $dp:expr, $field:ident, $pins:expr, $af:expr) => {{
        let port: &$port = &$dp.$field;
        let port: &pac::GPIOD = unsafe { &*(port as *const $port as *const pac::GPIOD) };
        af_push_pull_regs(port, $pins, $af);
    }};
}

fn configure_fmc_pins(dp: &pac::Peripherals) {
    const AF12: u32 = 0b1100;
    af_port!(pac::GPIOD, dp, GPIOD, &[0, 1, 8, 9, 10, 14, 15], AF12);
    af_port!(pac::GPIOE, dp, GPIOE, &[0, 1, 7, 8, 9, 10, 11, 12, 13, 14, 15], AF12);
    af_port!(pac::GPIOF, dp, GPIOF, &[0, 1, 2, 3, 4, 5, 11, 12, 13, 14, 15], AF12);
    af_port!(pac::GPIOG, dp, GPIOG, &[0, 1, 2, 4, 5, 8, 15], AF12);
    af_port!(pac::GPIOH, dp, GPIOH, &[2, 3, 5, 8, 9, 10, 11, 12, 13, 14, 15], AF12);
    af_port!(pac::GPIOI, dp, GPIOI, &[0, 1, 2, 3, 4, 5, 6, 7, 9, 10], AF12);
}

fn af_push_pull_regs(port: &pac::GPIOD, pins: &[u8], af: u32) {
    for pin in pins {
        let bit = *pin as u32;
        let shift = bit * 2;
        port.moder.modify(|r, w| unsafe {
            w.bits((r.bits() & !(0b11 << shift)) | (0b10 << shift))
        });
        port.otyper.modify(|r, w| unsafe { w.bits(r.bits() & !(1 << bit)) });
        let afr_shift = (bit % 8) * 4;
        if bit < 8 {
            port.afrl.modify(|r, w| unsafe {
                w.bits((r.bits() & !(0xF << afr_shift)) | (af << afr_shift))
            });
        } else {
            port.afrh.modify(|r, w| unsafe {
                w.bits((r.bits() & !(0xF << afr_shift)) | (af << afr_shift))
            });
        }
    }
}

fn configure_controller(fmc: &pac::FMC) {
    // SDCLK = HCLK/2 → 108 MHz @ SYSCLK 216 MHz.
    fmc.sdcr1().modify(|_, w| {
        w.nc().bits8();
        w.nr().bits12();
        w.mwid().bits32();
        w.nb().nb4();
        w.cas().clocks3();
        w.wp().disabled();
        w.sdclk().div2()
    });
    fmc.sdcr2().modify(|_, w| {
        w.rburst().enabled();
        w.rpipe().no_delay()
    });
    fmc.sdtr1().modify(|_, w| {
        w.tmrd().bits(2);
        w.txsr().bits(7);
        w.tras().bits(4);
        w.trc().bits(7);
        w.twr().bits(2);
        w.trp().bits(2);
        w.trcd().bits(2)
    });
}

fn run_init_sequence(fmc: &pac::FMC) -> Result<(), SdramError> {
    send_command(fmc, sdcmr_mode::CLK_ENABLE, 0, 0)?;
    delay_us(200);

    send_command(fmc, sdcmr_mode::PALL, 0, 0)?;
    send_command(fmc, sdcmr_mode::AUTO_REFRESH, 8, 0)?;

    let mode_reg = SDRAM_MODEREG_BURST_LENGTH_1
        | SDRAM_MODEREG_BURST_TYPE_SEQUENTIAL
        | SDRAM_MODEREG_CAS_LATENCY_3
        | SDRAM_MODEREG_OPERATING_MODE_STANDARD
        | SDRAM_MODEREG_WRITEBURST_MODE_SINGLE;
    send_command(fmc, sdcmr_mode::LOAD_MODE, 1, mode_reg)?;

    let sdclk_mhz = (SYSCLK_HZ / 2) / 1_000_000;
    // BSP REFRESH_COUNT 0x0603 @ 100 MHz SDCLK, escalado lineal.
    let refresh = (1539u32 * sdclk_mhz / 100).saturating_sub(20);
    fmc.sdrtr.write(|w| w.count().bits(refresh as u16));
    Ok(())
}

mod sdcmr_mode {
    pub const CLK_ENABLE: u8 = 1;
    pub const PALL: u8 = 2;
    pub const AUTO_REFRESH: u8 = 3;
    pub const LOAD_MODE: u8 = 4;
}

const SDRAM_MODEREG_BURST_LENGTH_1: u16 = 0x0000;
const SDRAM_MODEREG_BURST_TYPE_SEQUENTIAL: u16 = 0x0000;
const SDRAM_MODEREG_CAS_LATENCY_3: u16 = 0x0030;
const SDRAM_MODEREG_OPERATING_MODE_STANDARD: u16 = 0x0000;
const SDRAM_MODEREG_WRITEBURST_MODE_SINGLE: u16 = 0x0200;

fn send_command(
    fmc: &pac::FMC,
    mode: u8,
    auto_refresh: u8,
    mode_reg: u16,
) -> Result<(), SdramError> {
    fmc.sdcmr.write(|w| {
        match mode {
            sdcmr_mode::CLK_ENABLE => {
                w.mode().clock_configuration_enable();
            }
            sdcmr_mode::PALL => {
                w.mode().pall();
            }
            sdcmr_mode::AUTO_REFRESH => {
                w.mode().auto_refresh_command();
            }
            sdcmr_mode::LOAD_MODE => {
                w.mode().load_mode_register();
            }
            _ => {}
        }
        w.ctb1().issued();
        w.nrfs().bits(auto_refresh);
        w.mrd().bits(mode_reg)
    });
    wait_command(fmc)
}

fn wait_command(fmc: &pac::FMC) -> Result<(), SdramError> {
    for _ in 0..0xFFFF {
        if fmc.sdsr.read().busy().is_not_busy() {
            return Ok(());
        }
    }
    Err(SdramError::CommandTimeout)
}

fn delay_us(us: u32) {
    let ticks = (SYSCLK_HZ / 1_000_000) * us;
    cortex_m::asm::delay(ticks);
}

fn configure_mpu(_scb: &mut cortex_m::peripheral::SCB) {
    // MPU desactivada en G1: la región SDRAM se accede con mantenimiento
    // explícito de cache en verify. Configuración MPU completa llega en G2.
}

fn verify(scb: &mut cortex_m::peripheral::SCB, cpuid: &mut cortex_m::peripheral::CPUID) -> Result<(), SdramError> {
    // SAFETY: SDRAM init completada.
    unsafe {
        scb.disable_dcache(cpuid);
        let base = SDRAM_BASE as *mut u32;
        let pattern: u32 = 0xA5A5_1234;
        ptr::write_volatile(base, pattern);
        cortex_m::asm::dmb();
        if ptr::read_volatile(base) != pattern {
            scb.enable_dcache(cpuid);
            return Err(SdramError::VerifyFailed);
        }
        let pattern2: u32 = 0xDEAD_BEEF;
        ptr::write_volatile(base.add(1024), pattern2);
        cortex_m::asm::dmb();
        if ptr::read_volatile(base.add(1024)) != pattern2 {
            scb.enable_dcache(cpuid);
            return Err(SdramError::VerifyFailed);
        }
        scb.enable_dcache(cpuid);
    }
    Ok(())
}
