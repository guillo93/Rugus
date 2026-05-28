//! Rugus blink — STM32F103C8 Blue Pill.
//!
//! Rugus lite kickoff: HSI 8 MHz SYSCLK, configure PC13 (active low) and
//! toggle in a loop. Logs via `defmt` over SWD/RTT.

#![no_std]
#![no_main]

use rugus_runtime as _; // panic-probe + defmt-rtt
use rugus_runtime::entry;

use rugus_hal::GpioPin;
use rugus_hal_stm32f1::gpio::{BluePillLed, LedPin};
use rugus_hal_stm32f1::pac;
use rugus_hal_stm32f1::rcc;

#[entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("cortex-m peripherals already taken");
    let dp = pac::Peripherals::take().expect("device peripherals already taken");

    let clocks = rcc::init(&dp);
    rugus_runtime::enable_cycle_counter(&mut cp);

    defmt::info!(
        "rugus blink @ STM32F103C8 Blue Pill, SYSCLK {} MHz",
        clocks.sysclk_mhz()
    );

    let mut led = LedPin::new(&dp.RCC, BluePillLed::Pc13);
    defmt::info!("PC13 configured (active low); toggling at ~1 Hz");

    const BUSY_TICKS: u32 = 8_000_000;
    loop {
        led.toggle().ok();
        cortex_m::asm::delay(BUSY_TICKS);
    }
}
