//! Rugus blink — STM32F407G-DISC1.
//!
//! G3: HSE 8 MHz → PLL 168 MHz, configure LD4 (PD12) and toggle in a loop.
//! Logs via `defmt` over SWD/RTT.

#![no_std]
#![no_main]

use rugus_runtime as _; // panic-probe + defmt-rtt
use rugus_runtime::entry;

use rugus_hal::GpioPin;
use rugus_hal_stm32f4::gpio::{DiscoLed, LedPin};
use rugus_hal_stm32f4::pac;
use rugus_hal_stm32f4::rcc;

#[entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("cortex-m peripherals already taken");
    let dp = pac::Peripherals::take().expect("device peripherals already taken");

    let clocks = rcc::init(&dp);
    rugus_runtime::enable_cycle_counter(&mut cp);

    defmt::info!(
        "rugus blink @ STM32F407G-DISC1, SYSCLK {} MHz",
        clocks.sysclk_mhz()
    );

    let mut led = LedPin::new(&dp.RCC, DiscoLed::Green);
    defmt::info!("LD4 (PD12) configured; toggling at ~1 Hz");

    const BUSY_TICKS: u32 = 168_000_000;
    loop {
        led.toggle().ok();
        cortex_m::asm::delay(BUSY_TICKS);
    }
}
