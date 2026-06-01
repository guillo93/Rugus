//! Rugus app-sandbox — STM32F407G-DISC1, sobre la capa `rugus-kernel`.
//!
//! El `main` solo aporta lo específico de la placa (relojes, MPU layout, heap,
//! LEDs y las tareas); el scheduler, los hooks de syscall y el hook de fault los
//! posee y cablea `rugus-kernel`.
//!
//! Tareas:
//! - kernel (priv): supervisa y refleja el estado del sistema en los 4 LEDs.
//! - good_app (user): duerme vía syscall; sobrevive indefinidamente.
//! - bad_app (user): tras unos ciclos accede a periféricos → MemManage; el
//!   kernel la mata y el resto sigue.
//!
//! Visualización por LEDs (todos los maneja la tarea kernel privilegiada: una
//! app userland no puede tocar GPIO, está en el dominio Drivers tras la MPU).
//! Cada LED tiene un patrón propio derivado del reloj monotónico (`now_ms`),
//! muestreado a cadencia rápida (~40 ms) para que se distingan a simple vista:
//! - LD4 verde   : latido del kernel — doble pulso tipo "lub-dub" cada 1 s.
//! - LD6 azul    : actividad de userland — onda cuadrada ~3 Hz mientras good_app
//!   vive; apagado fijo si murió.
//! - LD3 naranja : salud del supervisor — fijo si el sistema está sano; parpadeo
//!   lento ~1 Hz si alguna tarea murió ("degradado").
//! - LD5 rojo    : fault contenido — se enciende y queda latcheado al primer
//!   fault que el failsafe contiene.

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use rugus_arch_cortex_m::{platform_init, time, MpuLayout};
use rugus_core::fault::FaultReport;
use rugus_core::sched::Priority;
use rugus_core::syscall::user as svc_user;
use rugus_hal::GpioPin;
use rugus_hal_stm32f4::gpio::{DiscoLed, LedPin};
use rugus_hal_stm32f4::pac;
use rugus_hal_stm32f4::rcc;
use rugus_runtime::entry;

#[repr(C, align(4096))]
struct Stack4k([u8; 4096]);

static mut STACK_KERNEL: Stack4k = Stack4k([0; 4096]);
static mut STACK_GOOD: Stack4k = Stack4k([0; 4096]);
static mut STACK_BAD: Stack4k = Stack4k([0; 4096]);

/// Índice (= TaskId) de good_app según el orden de spawn de [`main`].
const GOOD_IDX: usize = 2;

/// Cadencia de muestreo del supervisor (~40 ms a 168 MHz): suficientemente
/// fino para que cada LED dibuje su patrón propio sin entrar en `wfi`.
const SAMPLE_CYCLES: u32 = 168_000_000 / 25;

static mut LED_ALIVE: Option<LedPin> = None;
static mut LED_USER: Option<LedPin> = None;
static mut LED_SUPERVISOR: Option<LedPin> = None;
static mut LED_FAULT: Option<LedPin> = None;

fn kernel_task() -> ! {
    defmt::info!("kernel task (LD4) started");
    let mut last_log_s = u32::MAX;
    loop {
        let now = time::now_ms();
        let killed = rugus_kernel::killed_count();
        // SAFETY: los LEDs solo los toca esta tarea privilegiada, cooperativa.
        unsafe {
            if let Some(led) = LED_ALIVE.as_mut() {
                let _ = if heartbeat(now) { led.set_high() } else { led.set_low() };
            }
            if let Some(led) = LED_USER.as_mut() {
                let on = !rugus_kernel::task_killed(GOOD_IDX) && user_activity(now);
                let _ = if on { led.set_high() } else { led.set_low() };
            }
            if let Some(led) = LED_SUPERVISOR.as_mut() {
                let on = if killed == 0 { true } else { degraded_blink(now) };
                let _ = if on { led.set_high() } else { led.set_low() };
            }
        }
        // Log throttled a ~1/s (el muestreo de LEDs corre mucho más rápido).
        let now_s = now / 1000;
        if now_s != last_log_s {
            last_log_s = now_s;
            defmt::debug!("supervisor: alive killed={=usize} @ {=u32} ms", killed, now);
        }
        // Muestreo ACTIVO (paced busy-wait + yield), no `sleep`: mantiene una
        // tarea siempre lista para que el scheduler no entre en `wfi`. En
        // STM32F4 el WFI apaga el reloj de debug y ST-Link/probe-rs pierde RTT
        // (incluso con DBGMCU.DBG_SLEEP). La ruta de bajo consumo (sleep/wake
        // real) la ejercita `good_app`.
        cortex_m::asm::delay(SAMPLE_CYCLES);
        rugus_kernel::cpu_yield();
    }
}

/// Latido "lub-dub": doble pulso corto al inicio de cada ventana de 1 s.
#[inline]
fn heartbeat(now_ms: u32) -> bool {
    let t = now_ms % 1000;
    t < 80 || (200..280).contains(&t)
}

/// Actividad de userland: onda cuadrada ~3 Hz (periodo ~333 ms).
#[inline]
fn user_activity(now_ms: u32) -> bool {
    (now_ms / 166) % 2 == 0
}

/// Parpadeo lento ~1 Hz para señalar estado degradado.
#[inline]
fn degraded_blink(now_ms: u32) -> bool {
    (now_ms / 500) % 2 == 0
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
    // no rescata RTT por ST-Link en F4, por eso el supervisor late activo.
    unsafe {
        core::ptr::write_volatile(0xE004_2004 as *mut u32, 0b111);
    }
    let mut cp = cortex_m::Peripherals::take().expect("cortex-m peripherals");
    let dp = pac::Peripherals::take().expect("device peripherals");

    let clocks = rcc::init(&dp);
    rugus_runtime::enable_cycle_counter(&mut cp);

    defmt::info!(
        "rugus app-sandbox @ STM32F407G-DISC1, SYSCLK {} MHz, ABI {=u16}",
        clocks.sysclk_mhz(),
        rugus_core::syscall::ABI_VERSION
    );

    static mut HEAP: [u8; 32 * 1024] = [0; 32 * 1024];
    const HEAP_SIZE: usize = 32 * 1024;
    unsafe {
        rugus_core::heap::init(core::ptr::addr_of_mut!(HEAP).cast(), HEAP_SIZE);
    }

    platform_init(&mut cp, &MpuLayout::STM32F407);
    time::init(&mut cp.SYST, clocks.hclk);

    // LEDs de estado (todos en GPIOD): verde=kernel, azul=user, naranja=salud.
    unsafe {
        LED_ALIVE = Some(LedPin::new(&dp.RCC, DiscoLed::Green));
        LED_USER = Some(LedPin::new(&dp.RCC, DiscoLed::Blue));
        LED_SUPERVISOR = Some(LedPin::new(&dp.RCC, DiscoLed::Orange));
        let mut fault_led = LedPin::new(&dp.RCC, DiscoLed::Red);
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

/// Observador de fault de plataforma: latchea el LED rojo al primer fault
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
