//! Rugus F5.C.1 — bringup de la flash QSPI NOR en STM32F769I-DISCO.
//!
//! Valida el driver [`rugus_hal_stm32f7::qspi`] contra la Macronix MX25L51245G
//! (512 Mbit) de la placa (UM2033 §5.14):
//!
//! 1. Inicializa relojes + I/D-cache.
//! 2. Crea el driver QSPI (configura pines, controlador, 4-byte mode) y verifica
//!    el JEDEC ID (espera `C2 20 1A` = Macronix / tipo / 512 Mbit).
//! 3. Ejercicio `BlockDevice`: borra un subsector, comprueba que quedó a `0xFF`,
//!    programa un patrón y lo relee verificando.
//!
//! Todo el log sale por SWD/RTT desde una tarea privilegiada (no hay scheduler
//! aquí; es un probe de bringup puro). El LED rojo (LD1/PJ13) indica el
//! resultado: parpadeo rápido = OK, fijo encendido = fallo.

#![no_std]
#![no_main]

use rugus_runtime as _; // panic-probe + defmt-rtt
use rugus_runtime::entry;

use rugus_hal::BlockDevice;
use rugus_hal::GpioPin;
use rugus_hal_stm32f7::cache;
use rugus_hal_stm32f7::gpio::{DiscoLed, LedPin};
use rugus_hal_stm32f7::pac;
use rugus_hal_stm32f7::qspi::Qspi;
use rugus_hal_stm32f7::rcc;

/// Dirección de prueba (un subsector "alto" para no pisar nada relevante).
const TEST_ADDR: u32 = 0x0010_0000; // 1 MiB
const PATTERN: &[u8] = b"RUGUS-F5C QSPI MX25L51245G ok #0123456789";

#[entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("cortex-m peripherals already taken");
    let dp = pac::Peripherals::take().expect("device peripherals already taken");

    let clocks = rcc::init(&dp);
    cache::enable(&mut cp.SCB, &mut cp.CPUID);
    rugus_runtime::enable_cycle_counter(&mut cp);

    defmt::info!(
        "rugus qspi-probe @ STM32F769I-DISCO, SYSCLK {} MHz",
        clocks.sysclk_mhz()
    );

    let mut led = LedPin::new(&dp.RCC, DiscoLed::Red);

    match run(dp.QUADSPI, &dp.RCC) {
        Ok(()) => {
            defmt::info!("QSPI PROBE OK — JEDEC + erase/program/read verificados");
            // Parpadeo rápido infinito = éxito.
            loop {
                led.toggle().ok();
                cortex_m::asm::delay(20_000_000);
            }
        }
        Err(e) => {
            defmt::error!("QSPI PROBE FALLO: {}", e);
            // LED fijo encendido = fallo.
            led.set_low().ok();
            loop {
                cortex_m::asm::wfi();
            }
        }
    }
}

/// Resultado legible para defmt.
#[derive(defmt::Format)]
enum ProbeError {
    Init,
    EraseVerify,
    ReadBack,
}

fn run(qspi: pac::QUADSPI, rcc: &pac::RCC) -> Result<(), ProbeError> {
    let mut flash = match Qspi::new(qspi, rcc) {
        Ok(f) => f,
        Err(e) => {
            defmt::error!("Qspi::new error: {}", defmt::Debug2Format(&e));
            return Err(ProbeError::Init);
        }
    };

    let mut id = [0u8; 3];
    flash.read_jedec(&mut id).map_err(|_| ProbeError::Init)?;
    defmt::info!("JEDEC ID = {:02x} {:02x} {:02x}", id[0], id[1], id[2]);
    defmt::info!(
        "capacity = {} MiB, page = {} B, erase = {} B",
        flash.capacity() / (1024 * 1024),
        flash.prog_size(),
        flash.erase_size()
    );

    // 1) Borra el subsector y verifica 0xFF.
    flash
        .erase_sector(TEST_ADDR)
        .map_err(|_| ProbeError::EraseVerify)?;
    let mut buf = [0u8; 64];
    flash
        .read(TEST_ADDR, &mut buf)
        .map_err(|_| ProbeError::EraseVerify)?;
    if buf.iter().any(|&b| b != 0xFF) {
        defmt::error!("post-erase no es 0xFF: {:02x}", buf);
        return Err(ProbeError::EraseVerify);
    }
    defmt::info!("erase OK (subsector @ {:#010x} a 0xFF)", TEST_ADDR);

    // 2) Programa el patrón y reléelo.
    flash
        .program(TEST_ADDR, PATTERN)
        .map_err(|_| ProbeError::ReadBack)?;
    let mut rb = [0u8; 64];
    flash
        .read(TEST_ADDR, &mut rb[..PATTERN.len()])
        .map_err(|_| ProbeError::ReadBack)?;
    if &rb[..PATTERN.len()] != PATTERN {
        defmt::error!("read-back no coincide: {:02x}", rb[..PATTERN.len()]);
        return Err(ProbeError::ReadBack);
    }
    defmt::info!("program+read OK ({} bytes verificados)", PATTERN.len());
    Ok(())
}
