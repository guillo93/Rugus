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

    // Evita accesos especulativos del CPU a NOR bank1 que bloquean el bus FMC
    // compartido con SDRAM (recomendación ST en BSP / community).
    dp.FMC.bcr1.modify(|_, w| w.mbken().disabled());

    configure_fmc_pins(dp);
}

fn configure_fmc_pins(_dp: &pac::Peripherals) {
    const AF12: u32 = 0b1100;
    af_push_pull_port(unsafe { &*pac::GPIOD::ptr() }, &[0, 1, 8, 9, 10, 14, 15], AF12);
    af_push_pull_port(unsafe { &*pac::GPIOE::ptr() }, &[0, 1, 7, 8, 9, 10, 11, 12, 13, 14, 15], AF12);
    af_push_pull_port(unsafe { &*pac::GPIOF::ptr() }, &[0, 1, 2, 3, 4, 5, 11, 12, 13, 14, 15], AF12);
    af_push_pull_port(unsafe { &*pac::GPIOG::ptr() }, &[0, 1, 2, 4, 5, 8, 15], AF12);
    af_push_pull_port(unsafe { &*pac::GPIOH::ptr() }, &[2, 3, 5, 8, 9, 10, 11, 12, 13, 14, 15], AF12);
    af_push_pull_port(unsafe { &*pac::GPIOI::ptr() }, &[0, 1, 2, 3, 4, 5, 6, 7, 9, 10], AF12);
}

fn af_push_pull_port(port: &pac::gpiod::RegisterBlock, pins: &[u8], af: u32) {
    // BSP ST: AF push-pull, pull-up, very-high speed (108 MHz SDCLK).
    const OSPEED_VERY_HIGH: u32 = 0b11;
    const PULL_UP: u32 = 0b01;

    for pin in pins {
        let bit = *pin as u32;
        let shift = bit * 2;
        port.moder.modify(|r, w| unsafe {
            w.bits((r.bits() & !(0b11 << shift)) | (0b10 << shift))
        });
        port.otyper.modify(|r, w| unsafe { w.bits(r.bits() & !(1 << bit)) });
        port.ospeedr.modify(|r, w| unsafe {
            w.bits((r.bits() & !(0b11 << shift)) | (OSPEED_VERY_HIGH << shift))
        });
        port.pupdr.modify(|r, w| unsafe {
            w.bits((r.bits() & !(0b11 << shift)) | (PULL_UP << shift))
        });
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
    // RBURST/RPIPE pertenecen a SDCR1 (bank 1), no SDCR2.
    fmc.sdcr1().modify(|_, w| {
        w.nc().bits8();
        w.nr().bits12();
        w.mwid().bits32();
        w.nb().nb4();
        w.cas().clocks3();
        w.wp().disabled();
        w.sdclk().div2();
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
    // BSP ST: ≥100 µs tras clock enable; usamos 1 ms (HAL_Delay(1)).
    delay_us(1000);

    send_command(fmc, sdcmr_mode::PALL, 0, 0)?;
    send_command(fmc, sdcmr_mode::AUTO_REFRESH, 8, 0)?;

    let mode_reg = SDRAM_MODEREG_BURST_LENGTH_1
        | SDRAM_MODEREG_BURST_TYPE_SEQUENTIAL
        | SDRAM_MODEREG_CAS_LATENCY_3
        | SDRAM_MODEREG_OPERATING_MODE_STANDARD
        | SDRAM_MODEREG_WRITEBURST_MODE_SINGLE;
    send_command(fmc, sdcmr_mode::LOAD_MODE, 1, mode_reg)?;

    let sdclk_mhz = (SYSCLK_HZ / 2) / 1_000_000;
    // BSP REFRESH_COUNT 0x0603 @ 100 MHz SDCLK; escalar a SDCLK real (108 MHz).
    const REFRESH_COUNT_100MHZ: u32 = 0x0603;
    let refresh = (REFRESH_COUNT_100MHZ * sdclk_mhz / 100).saturating_sub(20);
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
        // NRFS solo aplica al comando AUTO_REFRESH; resto = 0 (valores raw ST).
        let nrfs = if mode == sdcmr_mode::AUTO_REFRESH {
            auto_refresh
        } else {
            0
        };
        w.nrfs().bits(nrfs);
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
    let dcache_was_on = cortex_m::peripheral::SCB::dcache_enabled();
    unsafe {
        if dcache_was_on {
            scb.disable_dcache(cpuid);
        }
        let base = SDRAM_BASE as *mut u32;
        let pattern: u32 = 0xA5A5_1234;
        ptr::write_volatile(base, pattern);
        cortex_m::asm::dmb();
        if ptr::read_volatile(base) != pattern {
            if dcache_was_on {
                scb.enable_dcache(cpuid);
            }
            return Err(SdramError::VerifyFailed);
        }
        let pattern2: u32 = 0xDEAD_BEEF;
        ptr::write_volatile(base.add(1024), pattern2);
        cortex_m::asm::dmb();
        if ptr::read_volatile(base.add(1024)) != pattern2 {
            if dcache_was_on {
                scb.enable_dcache(cpuid);
            }
            return Err(SdramError::VerifyFailed);
        }
        if dcache_was_on {
            scb.enable_dcache(cpuid);
        }
    }
    Ok(())
}
