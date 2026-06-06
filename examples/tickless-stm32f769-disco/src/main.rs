//! Rugus F5.A.1 — **tick dinámico (tickless)** en la STM32F769I-DISCO.
//!
//! Demuestra el primer paso de la línea de energía: cuando todas las tareas
//! duermen, el scheduler ya no deja que SysTick interrumpa cada milisegundo.
//! En su lugar calcula el **próximo plazo** (`next_wake_ms`) y la capa de tiempo
//! del backend Cortex-M reprograma SysTick a ese intervalo (acotado por su
//! reload de 24 bits), durmiendo el core con `wfi` hasta entonces. El reloj
//! monotónico (`now_ms`) se mantiene EXACTO: la ISR suma de golpe los ms del
//! intervalo extendido al expirar, y un despertar anticipado por IRQ externa se
//! contabiliza leyendo el contador.
//!
//! El binario activa la feature `tickless` del backend (`rugus-arch-cortex-m`);
//! la unificación de features de Cargo hace que el `CortexM::idle` que invoca el
//! scheduler en idle sea el que reprograma SysTick.
//!
//! Validación visible: el LED rojo (LD1/PJ13) parpadea a **1 Hz exacto** (prueba
//! de que el reloj no deriva con el tick dinámico) y por RTT se imprimen, en cada
//! parpadeo, `now_ms` y el nº de **interrupciones de SysTick** acumuladas. Con
//! tick fijo crecerían ~1000/s; con tick dinámico crecen muchísimo menos (el
//! ahorro de despertares ociosos), manteniendo `now_ms` clavado en pasos de
//! ~1000 ms.

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use core::ptr::addr_of_mut;

use rugus_runtime as _; // panic-probe + defmt-rtt
use rugus_runtime::entry;

use rugus_arch_cortex_m::{platform_init, time, MpuLayout};
use rugus_core::sched::Priority;
use rugus_hal::GpioPin;
use rugus_hal_stm32f7::cache;
use rugus_hal_stm32f7::gpio::{DiscoLed, LedPin};
use rugus_hal_stm32f7::pac;
use rugus_hal_stm32f7::rcc;

#[repr(C, align(4096))]
struct Stack4k([u8; 4096]);

static mut STACK_BLINK: Stack4k = Stack4k([0; 4096]);
static mut STACK_WORKER: Stack4k = Stack4k([0; 4096]);

/// LED de latido (LD Red), poseído por la tarea de parpadeo.
static mut HB_LED: Option<LedPin> = None;

#[entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("cortex-m peripherals");
    let dp = pac::Peripherals::take().expect("device peripherals");

    let clocks = rcc::init(&dp);
    cache::enable(&mut cp.SCB, &mut cp.CPUID);
    rugus_runtime::enable_cycle_counter(&mut cp);

    defmt::info!(
        "rugus tickless @ STM32F769I-DISCO, SYSCLK {} MHz (tick dinámico ON)",
        clocks.sysclk_mhz()
    );

    // FPU-context + fault handlers + layout MPU (tareas privilegiadas aquí).
    platform_init(&mut cp, &MpuLayout::STM32F769);

    // Base de tiempo del kernel: SysTick a 1 kHz; el tick dinámico la reprograma
    // en idle. `clocks.hclk` da las cuentas por ms.
    time::init(&mut cp.SYST, clocks.hclk);

    // SAFETY: arranque single-thread; el LED se inicializa una sola vez aquí.
    unsafe {
        HB_LED = Some(LedPin::new(&dp.RCC, DiscoLed::Red));
    }

    // SAFETY: arranque single-thread; pilas estáticas vivas para todo el kernel.
    unsafe {
        rugus_kernel::install(None);
        rugus_kernel::spawn(
            &mut (*addr_of_mut!(STACK_BLINK)).0,
            blink_task,
            Priority::Kernel,
        )
        .expect("spawn blink");
        rugus_kernel::spawn(
            &mut (*addr_of_mut!(STACK_WORKER)).0,
            worker_task,
            Priority::App,
        )
        .expect("spawn worker");
        defmt::info!("scheduler: 2 tareas (blink 1 Hz + worker 250 ms), starting");
        rugus_kernel::start();
    }
}

/// Tarea de latido: parpadea LD Red a 1 Hz y reporta el reloj y el contador de
/// interrupciones de SysTick. La clave de la demostración: el LED debe mantener
/// 1 Hz EXACTO mientras las IRQs de SysTick crecen mucho menos de 1000/s.
fn blink_task() -> ! {
    let mut last_ms = time::now_ms();
    let mut last_irqs = time::systick_irqs();
    loop {
        // SAFETY: solo esta tarea toca el LED de latido.
        unsafe {
            if let Some(led) = (*addr_of_mut!(HB_LED)).as_mut() {
                let _ = led.toggle();
            }
        }
        let now = time::now_ms();
        let irqs = time::systick_irqs();
        defmt::info!(
            "blink: t={=u32} ms (+{=u32}), systick_irqs={=u32} (+{=u32})",
            now,
            now.wrapping_sub(last_ms),
            irqs,
            irqs.wrapping_sub(last_irqs),
        );
        last_ms = now;
        last_irqs = irqs;
        // Duerme 1 s cediendo el CPU: con la worker también dormida, el scheduler
        // entra en idle y el tick dinámico reprograma SysTick.
        rugus_kernel::cpu_sleep_ms(1000);
    }
}

/// Tarea secundaria: duerme 250 ms en bucle. Aporta un plazo más cercano que el
/// del blink, demostrando que el tick dinámico se ajusta al próximo despertar
/// (no al más lejano) sin perder precisión.
fn worker_task() -> ! {
    loop {
        rugus_kernel::cpu_sleep_ms(250);
    }
}
