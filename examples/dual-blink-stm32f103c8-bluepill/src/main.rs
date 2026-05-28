//! Rugus dual-blink — dos tareas cooperativas en STM32F103C8 Blue Pill.
//!
//! Rugus lite: HSI 8 MHz, heap en SRAM interna, scheduler cooperativo con PendSV.
//! PC13 (único LED) lo alternan task A (~0.5 s) y task B (~0.33 s) vía `rugus_core::sched`.
//! Sin MPU — aislamiento cooperativo únicamente.

#![no_std]
#![no_main]

extern crate alloc;

use rugus_arch_cortex_m::CortexM;
use rugus_core::heap;
use rugus_core::sched::{Priority, Scheduler};
use rugus_hal_stm32f1::gpio::{BluePillLed, LedPin};
use rugus_hal_stm32f1::pac;
use rugus_hal_stm32f1::rcc;
use rugus_runtime::entry;

type Sched = Scheduler<CortexM>;

static mut SCHEDULER: Sched = Sched::new();
static mut STACK_A: [u8; 2048] = [0; 2048];
static mut STACK_B: [u8; 2048] = [0; 2048];

const TICKS_HALF_SEC: u32 = 8_000_000 / 2;
const TICKS_THIRD_SEC: u32 = 8_000_000 / 3;

fn task_a() -> ! {
    defmt::info!("task A (PC13) started");
    loop {
        toggle_pc13();
        defmt::debug!("task A toggle PC13");
        delay(TICKS_HALF_SEC);
        yield_cpu();
    }
}

fn task_b() -> ! {
    defmt::info!("task B (PC13) started");
    loop {
        toggle_pc13();
        defmt::debug!("task B toggle PC13");
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
        "rugus dual-blink @ STM32F103C8 Blue Pill, SYSCLK {} MHz",
        clocks.sysclk_mhz()
    );

    static mut HEAP: [u8; 4 * 1024] = [0; 4 * 1024];
    const HEAP_SIZE: usize = 4 * 1024;

    // SAFETY: región estática reservada; 20 KiB SRAM total en F103.
    unsafe {
        heap::init(core::ptr::addr_of_mut!(HEAP).cast(), HEAP_SIZE);
    }
    defmt::info!(
        "heap on internal SRAM ({=u32} KiB)",
        HEAP_SIZE as u32 / 1024
    );

    let _box: alloc::boxed::Box<u32> = alloc::boxed::Box::new(0);
    defmt::info!("heap alloc smoke test OK");

    let _ = LedPin::new(&dp.RCC, BluePillLed::Pc13);

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

fn toggle_pc13() {
    // SAFETY: BSRR/ODR atómico por bit; scheduler cooperativo sin preemption.
    unsafe {
        let g = &*pac::GPIOC::ptr();
        g.odr.modify(|r, w| w.bits(r.bits() ^ (1 << 13)));
    }
}
