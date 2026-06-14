//! Rugus blink — placa clon "Black" STM32F407VET6 (FK407M3-VET6 v1.1).
//!
//! Primer bring-up de la board (paso G2 del porte): valida el árbol de reloj
//! (HSE 8 MHz → PLL 168 MHz, igual que la F407G-DISC1), el SWD/RTT y el LED de
//! usuario reusando `rugus-hal-stm32f4` sin cambios.
//!
//! ## Hardware (FK407M3-VET6 v1.1 — confirmado por inspección física + barrido)
//!
//! - HSE: cristal **8.000 MHz** (marcado "ZHM 8.000" junto al MCU) → encaja con
//!   `rcc::init`, sin tocar el HAL.
//! - LED de usuario **verde en PC0** (hallado por barrido contable). El LED
//!   **blanco es de power** (fijo).
//! - SWD: header dedicado **`DIO`(PA13) / `CLK`(PA14)** + `5V`/`GND` — ST-Link V2
//!   externo (sin depurador onboard). Jumpers BOOT0/BOOT1 a GND ⇒ arranca de
//!   flash. Header `RX`/`TX`/`RST`/`3V3` aparte para USB-TTL serie.
//!
//! ## Flasheo / RTT
//!
//! ```bash
//! probe-rs run --chip STM32F407VETx \
//!   target/thumbv7em-none-eabihf/release/blink-stm32f407vet6-blackpill
//! ```

#![no_std]
#![no_main]

use rugus_runtime as _; // panic-probe + defmt-rtt
use rugus_runtime::entry;

use rugus_hal::GpioPin;
use rugus_hal_stm32f4::gpio::{Pin, PinConfig, Port};
use rugus_hal_stm32f4::pac;
use rugus_hal_stm32f4::rcc;

#[entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("cortex-m peripherals already taken");
    let dp = pac::Peripherals::take().expect("device peripherals already taken");

    let clocks = rcc::init(&dp);
    rugus_runtime::enable_cycle_counter(&mut cp);

    defmt::info!(
        "rugus blink @ Black STM32F407VET6 (FK407M3), SYSCLK {} MHz",
        clocks.sysclk_mhz()
    );

    // LED de usuario verde en PC0. El HAL no tiene un `LedPin` para esta board
    // (su `DiscoLed` está atado a PD12-15 de la DISC1), así que usamos el `Pin`
    // genérico.
    let mut led = Pin::new(Port::C, 0, PinConfig::output());
    defmt::info!("LED verde (PC0): 1 flash/segundo (sanity-check del reloj)");

    // Periodo de 1 s exacto a 168 MHz: flash corto (80 ms) + apagado (920 ms).
    // Contar los flashes en 10 s mide el reloj real: ~10 ⇒ HSE 8 MHz correcto.
    // PC0 es active-low (en el barrido encendía con nivel bajo): flash = set_low.
    const MS: u32 = 168_000;
    loop {
        let _ = led.set_low(); // encendido
        cortex_m::asm::delay(80 * MS);
        let _ = led.set_high(); // apagado
        cortex_m::asm::delay(920 * MS);
    }
}
