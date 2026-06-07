//! Rugus F5.A.2 — **modo STOP con wake por RTC (LSI)** en la STM32F769I-DISCO.
//!
//! Segundo paso de la línea de energía, sobre el tick dinámico (F5.A.1): cuando
//! el scheduler entra en idle y el **próximo plazo** supera un umbral, en vez de
//! un `wfi` con SysTick extendido (que mantiene HSE/PLL vivos) se entra en
//! **STOP**, apagando HSE y PLL. El **wakeup timer del RTC**, relojado por LSI
//! (~32 kHz), es la única base que sigue corriendo en STOP y reprograma el
//! despertar; al salir, [`time`]/`rcc::restore_after_stop` re-arman HSE/PLL a
//! 216 MHz. En plazos cortos (por debajo del umbral) se conserva el tick
//! dinámico de F5.A.1.
//!
//! El salto a STOP se inyecta en el backend arch (que es agnóstico del HAL) con
//! [`time::set_stop_handler`]: se le pasa `power::enter_stop_ms` como manejador y
//! el umbral en ms. El `wfi` del STOP corre con las IRQs enmascaradas, de modo
//! que el evento del RTC despierta el núcleo SIN entrar a la ISR `RTC_WKUP`.
//!
//! **Precisión:** mientras se duerme en STOP el tiempo lo lleva LSI (±5 %), así
//! que `now_ms` avanza con esa tolerancia SOLO durante el sueño; en RUN se
//! conserva la exactitud del tick dinámico.
//!
//! **Contabilidad de energía (F5.A.3):** la capa de tiempo acumula el tiempo
//! dormido (`time::idle_ms`); este ejemplo registra un proveedor `PowerStats`
//! (`rugus_kernel::set_power_provider`) que mapea uptime / idle / systick_irqs /
//! stop_entries, los mismos que expone la consola de operador con el comando
//! `power`. Aquí se imprimen por RTT en cada parpadeo.
//!
//! Validación visible: el LED rojo (LD1/PJ13) parpadea cada **2 s** (plazo por
//! encima del umbral → cada ciclo entra en STOP) y por RTT se imprimen, en cada
//! parpadeo, `now_ms`, el nº de **entradas a STOP** acumuladas (`stop_entries`,
//! +1 por parpadeo) y el tiempo ocioso acumulado con su **% de idle** (~99 %).
//! Para ver RTT durante el STOP se mantiene vivo el dominio de depuración con
//! `power::keep_debug_in_stop` (solo banco).

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use core::ptr::addr_of_mut;

use rugus_runtime as _; // panic-probe + defmt-rtt
use rugus_runtime::entry;

use rugus_arch_cortex_m::{platform_init, time, MpuLayout};
use rugus_core::sched::Priority;
use rugus_hal::GpioPin;
use rugus_hal_stm32f7::gpio::{DiscoLed, LedPin};
use rugus_hal_stm32f7::pac;
use rugus_hal_stm32f7::{cache, power, rcc};
use rugus_kernel::PowerStats;

/// Umbral de plazo a partir del cual el idle entra en STOP (ms). Plazos más
/// cortos siguen usando el tick dinámico (SysTick extendido + `wfi`).
const STOP_THRESHOLD_MS: u32 = 1000;

#[repr(C, align(4096))]
struct Stack4k([u8; 4096]);

static mut STACK_BLINK: Stack4k = Stack4k([0; 4096]);

/// LED de latido (LD Red), poseído por la tarea de parpadeo.
static mut HB_LED: Option<LedPin> = None;

/// Proveedor de métricas de energía para la consola/telemetría (F5.A.3): mapea
/// los contadores de la capa de tiempo del backend arch a `PowerStats`.
fn power_provider() -> PowerStats {
    PowerStats {
        uptime_ms: time::now_ms(),
        idle_ms: time::idle_ms(),
        systick_irqs: time::systick_irqs(),
        stop_entries: time::stop_entries(),
    }
}

#[entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("cortex-m peripherals");
    let dp = pac::Peripherals::take().expect("device peripherals");

    let clocks = rcc::init(&dp);
    cache::enable(&mut cp.SCB, &mut cp.CPUID);
    rugus_runtime::enable_cycle_counter(&mut cp);

    defmt::info!(
        "rugus STOP mode @ STM32F769I-DISCO, SYSCLK {} MHz (RTC/LSI wake)",
        clocks.sysclk_mhz()
    );

    // FPU-context + fault handlers + layout MPU (tareas privilegiadas aquí).
    platform_init(&mut cp, &MpuLayout::STM32F769);

    // Base de tiempo del kernel: SysTick a 1 kHz; el tick dinámico la reprograma
    // en plazos cortos. `clocks.hclk` da las cuentas por ms.
    time::init(&mut cp.SYST, clocks.hclk);

    // RTC + LSI + EXTI línea 22 para usar STOP como fuente de wake.
    power::init(&dp);
    // Mantiene el canal RTT vivo durante STOP (solo banco; sube el consumo).
    power::keep_debug_in_stop(&dp);

    // Inyecta el manejador de STOP en el backend arch: en idle, si el próximo
    // plazo ≥ umbral, `idle_until` llama a `enter_stop_ms` en vez del `wfi`.
    time::set_stop_handler(power::enter_stop_ms, STOP_THRESHOLD_MS);

    // Publica las métricas de energía para la consola de operador (F5.A.3).
    // SAFETY: arranque single-thread; el proveedor se registra una sola vez.
    unsafe {
        rugus_kernel::set_power_provider(power_provider);
    }

    // SAFETY: arranque single-thread; el LED se inicializa una sola vez aquí.
    unsafe {
        HB_LED = Some(LedPin::new(&dp.RCC, DiscoLed::Red));
    }

    // SAFETY: arranque single-thread; pila estática viva para todo el kernel.
    unsafe {
        rugus_kernel::install(None);
        rugus_kernel::spawn(
            &mut (*addr_of_mut!(STACK_BLINK)).0,
            blink_task,
            Priority::Kernel,
        )
        .expect("spawn blink");
        defmt::info!("scheduler: 1 tarea (blink 2 s → STOP por ciclo), starting");
        rugus_kernel::start();
    }
}

/// Tarea de latido: parpadea LD Red cada 2 s. Como 2000 ms ≥ umbral, cada ciclo
/// de idle entra en STOP; al despertar reporta el reloj y el contador de
/// entradas a STOP, que debe crecer +1 por parpadeo.
fn blink_task() -> ! {
    let mut last_ms = time::now_ms();
    let mut last_stops = time::stop_entries();
    loop {
        // SAFETY: solo esta tarea toca el LED de latido.
        unsafe {
            if let Some(led) = (*addr_of_mut!(HB_LED)).as_mut() {
                let _ = led.toggle();
            }
        }
        let now = time::now_ms();
        let stops = time::stop_entries();
        let p = power_provider();
        defmt::info!(
            "blink: t={=u32} ms (+{=u32}), stop_entries={=u32} (+{=u32}), idle={=u32} ms ({=u32}%)",
            now,
            now.wrapping_sub(last_ms),
            stops,
            stops.wrapping_sub(last_stops),
            p.idle_ms,
            p.idle_percent(),
        );
        last_ms = now;
        last_stops = stops;
        // Duerme 2 s cediendo el CPU: con el plazo por encima del umbral, el
        // scheduler entra en idle y el camino de STOP apaga HSE/PLL.
        rugus_kernel::cpu_sleep_ms(2000);
    }
}
