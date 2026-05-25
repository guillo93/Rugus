//! Rugus dual-blink — dos tareas cooperativas en STM32F769I-DISCO.
//!
//! G1: RCC 216 MHz, cache, SDRAM + heap, scheduler cooperativo con PendSV.
//! LD1 (PJ13) y LD2 (PJ5) parpadean en paralelo vía `rugus_core::sched`.

#![no_std]
#![no_main]

extern crate alloc;

use rugus_arch_cortex_m::CortexM;
use rugus_core::heap;
use rugus_core::sched::{Priority, Scheduler};
use rugus_hal_stm32f7::cache;
use rugus_hal_stm32f7::fmc::{self, SDRAM_BASE};
use rugus_hal_stm32f7::gpio::{DiscoLed, LedPin};
use rugus_hal_stm32f7::pac;
use rugus_hal_stm32f7::rcc;
use rugus_runtime::entry;

type Sched = Scheduler<CortexM>;

static mut SCHEDULER: Sched = Sched::new();
static mut STACK_A: [u8; 4096] = [0; 4096];
static mut STACK_B: [u8; 4096] = [0; 4096];

const TICKS_HALF_SEC: u32 = 216_000_000 / 2;
const TICKS_THIRD_SEC: u32 = 216_000_000 / 3;

fn task_a() -> ! {
    defmt::info!("task A (LD1) started");
    loop {
        toggle_led(DiscoLed::Red);
        defmt::debug!("task A toggle LD1");
        delay(TICKS_HALF_SEC);
        yield_cpu();
    }
}

fn task_b() -> ! {
    defmt::info!("task B (LD2) started");
    loop {
        toggle_led(DiscoLed::Green);
        defmt::debug!("task B toggle LD2");
        delay(TICKS_THIRD_SEC);
        yield_cpu();
    }
}

#[entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("cortex-m peripherals");
    let dp = pac::Peripherals::take().expect("device peripherals");

    let clocks = rcc::init(&dp);
    rugus_runtime::enable_cycle_counter(&mut cp);

    defmt::info!(
        "rugus dual-blink @ STM32F769I-DISCO, SYSCLK {} MHz",
        clocks.sysclk_mhz()
    );

    static mut HEAP_FALLBACK: [u8; 64 * 1024] = [0; 64 * 1024];
    const HEAP_FALLBACK_SIZE: usize = 64 * 1024;

    // SDRAM antes de D-cache: verify accede sin cache; MPU completa en G2.
    match fmc::init(&dp, &mut cp.SCB, &mut cp.CPUID) {
        Ok(()) => {
            defmt::info!("SDRAM OK @ {=u32}", SDRAM_BASE as u32);
            const HEAP_SIZE: usize = 256 * 1024;
            // SAFETY: SDRAM inicializada; región reservada en memory.x.
            unsafe {
                heap::init(SDRAM_BASE as *mut u8, HEAP_SIZE);
            }
        }
        Err(fmc::SdramError::CommandTimeout) => {
            defmt::warn!("SDRAM init: command timeout; heap on internal RAM");
            unsafe {
                heap::init(
                    core::ptr::addr_of_mut!(HEAP_FALLBACK).cast(),
                    HEAP_FALLBACK_SIZE,
                );
            }
        }
        Err(fmc::SdramError::VerifyFailed) => {
            defmt::warn!("SDRAM init: verify failed; heap on internal RAM");
            unsafe {
                heap::init(
                    core::ptr::addr_of_mut!(HEAP_FALLBACK).cast(),
                    HEAP_FALLBACK_SIZE,
                );
            }
        }
    }
    cache::enable(&mut cp.SCB, &mut cp.CPUID);

    let _box: alloc::boxed::Box<u32> = alloc::boxed::Box::new(0);
    defmt::info!("heap alloc smoke test OK");

    let _ = LedPin::new(&dp.RCC, DiscoLed::Red);
    let _ = LedPin::new(&dp.RCC, DiscoLed::Green);

    // SAFETY: main es el único writer antes de start(); refs desde addr_of_mut!.
    unsafe {
        let sched = &mut *core::ptr::addr_of_mut!(SCHEDULER);
        sched
            .spawn(&mut *core::ptr::addr_of_mut!(STACK_A), task_a, Priority::App)
            .expect("spawn task A");
        sched
            .spawn(&mut *core::ptr::addr_of_mut!(STACK_B), task_b, Priority::App)
            .expect("spawn task B");
        defmt::info!("scheduler: 2 tasks, starting cooperative run");
        sched.start();
    }
}

fn yield_cpu() {
    // SAFETY: scheduler activo; yield cooperativo.
    unsafe {
        (&mut *core::ptr::addr_of_mut!(SCHEDULER)).yield_now();
    }
}

fn delay(ticks: u32) {
    cortex_m::asm::delay(ticks);
}

fn toggle_led(led: DiscoLed) {
    // SAFETY: BSRR/ODR atómico por bit; scheduler cooperativo sin preemption.
    unsafe {
        match led {
            DiscoLed::Red => {
                let g = &*pac::GPIOJ::ptr();
                g.odr.modify(|r, w| w.bits(r.bits() ^ (1 << 13)));
            }
            DiscoLed::Green => {
                let g = &*pac::GPIOJ::ptr();
                g.odr.modify(|r, w| w.bits(r.bits() ^ (1 << 5)));
            }
            _ => {}
        }
    }
}
