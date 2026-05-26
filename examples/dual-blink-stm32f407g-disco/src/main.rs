//! Rugus dual-blink — dos tareas cooperativas en STM32F407G-DISC1.
//!
//! G3: RCC 168 MHz, heap en SRAM interna, scheduler cooperativo con PendSV.
//! LD4 (PD12) task A, LD6 (PD15) task B vía `rugus_core::sched`.

#![no_std]
#![no_main]

extern crate alloc;

use rugus_arch_cortex_m::CortexM;
use rugus_core::heap;
use rugus_core::sched::{Priority, Scheduler};
use rugus_hal_stm32f4::gpio::{DiscoLed, LedPin};
use rugus_hal_stm32f4::pac;
use rugus_hal_stm32f4::rcc;
use rugus_runtime::entry;

type Sched = Scheduler<CortexM>;

static mut SCHEDULER: Sched = Sched::new();
static mut STACK_A: [u8; 4096] = [0; 4096];
static mut STACK_B: [u8; 4096] = [0; 4096];

const TICKS_HALF_SEC: u32 = 168_000_000 / 2;
const TICKS_THIRD_SEC: u32 = 168_000_000 / 3;

fn task_a() -> ! {
    defmt::info!("task A (LD4) started");
    loop {
        toggle_led(DiscoLed::Green);
        defmt::debug!("task A toggle LD4");
        delay(TICKS_HALF_SEC);
        yield_cpu();
    }
}

fn task_b() -> ! {
    defmt::info!("task B (LD6) started");
    loop {
        toggle_led(DiscoLed::Blue);
        defmt::debug!("task B toggle LD6");
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
        "rugus dual-blink @ STM32F407G-DISC1, SYSCLK {} MHz",
        clocks.sysclk_mhz()
    );

    static mut HEAP: [u8; 32 * 1024] = [0; 32 * 1024];
    const HEAP_SIZE: usize = 32 * 1024;

    // SAFETY: región estática reservada; no SDRAM en F407 DISC1.
    unsafe {
        heap::init(core::ptr::addr_of_mut!(HEAP).cast(), HEAP_SIZE);
    }
    defmt::info!(
        "heap on internal SRAM ({=u32} KiB)",
        HEAP_SIZE as u32 / 1024
    );

    let _box: alloc::boxed::Box<u32> = alloc::boxed::Box::new(0);
    defmt::info!("heap alloc smoke test OK");

    let _ = LedPin::new(&dp.RCC, DiscoLed::Green);
    let _ = LedPin::new(&dp.RCC, DiscoLed::Blue);

    // SAFETY: main es el único writer antes de start(); refs desde addr_of_mut!.
    unsafe {
        let sched = &mut *core::ptr::addr_of_mut!(SCHEDULER);
        sched
            .spawn(
                &mut *core::ptr::addr_of_mut!(STACK_A),
                task_a,
                Priority::App,
            )
            .expect("spawn task A");
        sched
            .spawn(
                &mut *core::ptr::addr_of_mut!(STACK_B),
                task_b,
                Priority::App,
            )
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
        let g = &*pac::GPIOD::ptr();
        let bit = match led {
            DiscoLed::Green => 1 << 12,
            DiscoLed::Blue => 1 << 15,
            _ => return,
        };
        g.odr.modify(|r, w| w.bits(r.bits() ^ bit));
    }
}
