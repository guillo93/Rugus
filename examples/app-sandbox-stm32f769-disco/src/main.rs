//! Rugus app-sandbox — STM32F769I-DISCO, sobre la capa `rugus-kernel`.
//!
//! El `main` solo aporta lo específico de la placa (relojes, SDRAM, caché, MPU
//! layout, heap, LEDs y las tareas); el scheduler, los hooks de syscall y el
//! hook de fault los posee y cablea `rugus-kernel`.
//!
//! Tareas:
//! - kernel (priv): supervisa y refleja el estado del sistema en los LEDs.
//! - good_app (user): duerme vía syscall; sobrevive indefinidamente.
//! - bad_app (user): tras unos ciclos accede a periféricos → MemManage; el
//!   kernel la mata y el resto sigue.
//!
//! Visualización por LEDs (todos los maneja la tarea kernel privilegiada: una
//! app userland no puede tocar GPIO, está en el dominio Drivers tras la MPU):
//! - LD Red    : latido del kernel (parpadea mientras el kernel vive).
//! - LD Green  : latido de userland (parpadea mientras good_app sigue viva).
//! - LD Red2   : salud del supervisor (fijo si ninguna tarea murió; se apaga al
//!   primer kill → "degradado").
//! - LD Green2 : fault contenido (se enciende y queda latcheado al primer fault
//!   que el failsafe contiene).

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use rugus_arch_cortex_m::{platform_init, time, MpuLayout};
use rugus_core::fault::FaultReport;
use rugus_core::sched::Priority;
use rugus_core::syscall::user as svc_user;
use rugus_hal::GpioPin;
use rugus_hal_stm32f7::cache;
use rugus_hal_stm32f7::fmc::{self, SDRAM_BASE};
use rugus_hal_stm32f7::gpio::{DiscoLed, LedPin};
use rugus_hal_stm32f7::pac;
use rugus_hal_stm32f7::rcc;
use rugus_runtime::entry;

#[repr(C, align(4096))]
struct Stack4k([u8; 4096]);

static mut STACK_KERNEL: Stack4k = Stack4k([0; 4096]);
static mut STACK_GOOD: Stack4k = Stack4k([0; 4096]);
static mut STACK_BAD: Stack4k = Stack4k([0; 4096]);

/// Índice (= TaskId) de good_app según el orden de spawn de [`main`].
const GOOD_IDX: usize = 2;

/// Cadencia del heartbeat del supervisor (~250 ms a 216 MHz).
const HEARTBEAT_CYCLES: u32 = 216_000_000 / 4;

static mut LED_ALIVE: Option<LedPin> = None;
static mut LED_USER: Option<LedPin> = None;
static mut LED_SUPERVISOR: Option<LedPin> = None;
static mut LED_FAULT: Option<LedPin> = None;

fn kernel_task() -> ! {
    defmt::info!("kernel task (LD Red) started");
    loop {
        // SAFETY: los LEDs solo los toca esta tarea privilegiada, cooperativa.
        unsafe {
            if let Some(led) = LED_ALIVE.as_mut() {
                let _ = led.toggle();
            }
            // Latido de userland: parpadea mientras good_app no haya muerto.
            if let Some(led) = LED_USER.as_mut() {
                if rugus_kernel::task_killed(GOOD_IDX) {
                    let _ = led.set_low();
                } else {
                    let _ = led.toggle();
                }
            }
            // Supervisor: fijo si el sistema está sano; apagado si algo murió.
            if let Some(led) = LED_SUPERVISOR.as_mut() {
                if rugus_kernel::killed_count() == 0 {
                    let _ = led.set_high();
                } else {
                    let _ = led.set_low();
                }
            }
        }
        defmt::debug!(
            "supervisor: alive killed={=usize} @ {=u32} ms",
            rugus_kernel::killed_count(),
            time::now_ms()
        );
        // Heartbeat ACTIVO (paced busy-wait + yield), no `sleep`: mantiene una
        // tarea siempre lista para que el scheduler no entre en `wfi`. En
        // Cortex-M el WFI apaga el reloj de debug y ST-Link/probe-rs pierde RTT,
        // así que para un sandbox de visualización el supervisor late de forma
        // activa. La ruta de bajo consumo (sleep/wake real) la ejercita `good_app`.
        cortex_m::asm::delay(HEARTBEAT_CYCLES);
        rugus_kernel::cpu_yield();
    }
}

fn good_app() -> ! {
    loop {
        // Sleep real vía syscall: no busy-wait; el scheduler corre otras tareas.
        let _ = svc_user::sleep_ms(200);
    }
}

fn bad_app() -> ! {
    let mut rounds = 0u32;
    loop {
        rounds += 1;
        let _ = svc_user::yield_now();
        if rounds >= 3 {
            // Acceso prohibido a dominio Drivers — MemManage en user mode.
            unsafe {
                let _ = core::ptr::read_volatile(0x4000_0000 as *const u32);
            }
        }
        spin_delay();
    }
}

#[entry]
fn main() -> ! {
    // DBGMCU: permite que el debugger siga conectado en sleep/stop/standby. Útil
    // en hardware para inspeccionar el WFI terminal (todas las tareas muertas);
    // no rescata RTT por ST-Link, por eso el supervisor late activo.
    unsafe {
        core::ptr::write_volatile(0xE004_2004 as *mut u32, 0b111);
    }
    let mut cp = cortex_m::Peripherals::take().expect("cortex-m peripherals");
    let dp = pac::Peripherals::take().expect("device peripherals");

    let clocks = rcc::init(&dp);
    rugus_runtime::enable_cycle_counter(&mut cp);

    defmt::info!(
        "rugus app-sandbox @ STM32F769I-DISCO, SYSCLK {} MHz, ABI {=u16}",
        clocks.sysclk_mhz(),
        rugus_core::syscall::ABI_VERSION
    );

    static mut HEAP_FALLBACK: [u8; 64 * 1024] = [0; 64 * 1024];
    match fmc::init(&dp, &mut cp.SCB, &mut cp.CPUID) {
        Ok(()) => {
            defmt::info!("SDRAM OK @ {=u32}", SDRAM_BASE as u32);
            const HEAP_SIZE: usize = 256 * 1024;
            unsafe {
                rugus_core::heap::init(SDRAM_BASE as *mut u8, HEAP_SIZE);
            }
        }
        Err(_e) => {
            defmt::warn!("SDRAM init failed; heap fallback");
            unsafe {
                rugus_core::heap::init(core::ptr::addr_of_mut!(HEAP_FALLBACK).cast(), 64 * 1024);
            }
        }
    }
    cache::enable(&mut cp.SCB, &mut cp.CPUID);

    platform_init(&mut cp, &MpuLayout::STM32F769);
    time::init(&mut cp.SYST, clocks.hclk);

    // LEDs de estado: Red=kernel, Green=user, Red2=salud, Green2=fault.
    unsafe {
        LED_ALIVE = Some(LedPin::new(&dp.RCC, DiscoLed::Red));
        LED_USER = Some(LedPin::new(&dp.RCC, DiscoLed::Green));
        LED_SUPERVISOR = Some(LedPin::new(&dp.RCC, DiscoLed::Red2));
        let mut fault_led = LedPin::new(&dp.RCC, DiscoLed::Green2);
        let _ = fault_led.set_low();
        LED_FAULT = Some(fault_led);
    }

    unsafe {
        rugus_kernel::install(Some(on_fault));
        rugus_kernel::spawn(&mut (*core::ptr::addr_of_mut!(STACK_KERNEL)).0, kernel_task, Priority::Kernel)
            .expect("spawn kernel");
        // bad_app y good_app comparten banda App y rotan justo (round-robin por
        // banda): el orden de spawn no decide cuál corre. GOOD_IDX debe coincidir
        // con el orden de spawn de userland.
        rugus_kernel::spawn_user(&mut (*core::ptr::addr_of_mut!(STACK_BAD)).0, bad_app, Priority::App)
            .expect("spawn bad app");
        rugus_kernel::spawn_user(&mut (*core::ptr::addr_of_mut!(STACK_GOOD)).0, good_app, Priority::App)
            .expect("spawn good app");

        defmt::info!("scheduler: 3 tasks (1 kernel + 2 userland), starting");
        rugus_kernel::start();
    }
}

/// Observador de fault de plataforma: latchea el LED de fault al primer fault
/// contenido. El kernel ya loguea el `FaultReport`; aquí solo el efecto visual.
fn on_fault(_report: &FaultReport) {
    // SAFETY: contexto de fault, single-thread; el LED solo se toca aquí y en main.
    unsafe {
        if let Some(led) = LED_FAULT.as_mut() {
            let _ = led.set_high();
        }
    }
}

fn spin_delay() {
    for _ in 0..500_000 {
        core::hint::spin_loop();
    }
}
