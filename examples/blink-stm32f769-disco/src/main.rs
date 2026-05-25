//! Rugus blink — STM32F769I-DISCO.
//!
//! G1: arranca con HSE 25 MHz → PLL 216 MHz, activa I/D-cache, configura LD1
//! (PJ13) y la toggle en bucle. Logs `defmt` por SWD/RTT confirman el boot.

#![no_std]
#![no_main]

use rugus_runtime as _; // panic-probe + defmt-rtt
use rugus_runtime::entry;

use rugus_hal::GpioPin;
use rugus_hal_stm32f7::cache;
use rugus_hal_stm32f7::gpio::{DiscoLed, LedPin};
use rugus_hal_stm32f7::pac;
use rugus_hal_stm32f7::rcc;

#[entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("cortex-m peripherals already taken");
    let dp = pac::Peripherals::take().expect("device peripherals already taken");

    let clocks = rcc::init(&dp);
    cache::enable(&mut cp.SCB, &mut cp.CPUID);

    rugus_runtime::enable_cycle_counter(&mut cp);
    defmt::info!(
        "rugus blink @ STM32F769I-DISCO, SYSCLK {} MHz",
        clocks.sysclk_mhz()
    );

    let mut led = LedPin::new(&dp.RCC, DiscoLed::Red);
    defmt::info!("LD1 (PJ13) configured; toggling at ~1 Hz");

    // Busy-wait calibrado a 216 MHz; precisión sustituida por SysTick en G1
    // cuando exista `rugus_core::sched::sleep_ms`.
    const BUSY_TICKS: u32 = 216_000_000;
    loop {
        led.toggle().ok();
        cortex_m::asm::delay(BUSY_TICKS);
    }
}
