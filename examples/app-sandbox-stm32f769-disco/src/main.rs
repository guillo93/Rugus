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
//! app userland no puede tocar GPIO, está en el dominio Drivers tras la MPU).
//! Cada LED tiene un patrón propio derivado del reloj monotónico (`now_ms`),
//! muestreado a cadencia rápida (~40 ms) para que se distingan a simple vista:
//! - LD Red    : latido del kernel — doble pulso tipo "lub-dub" cada 1 s.
//! - LD Green  : actividad de userland — la conmuta la PROPIA good_app vía IPC
//!   (syscall IpcSend → buzón del kernel → el supervisor toca el GPIO en su
//!   nombre). Userland no accede al GPIO directamente; apagado fijo si murió.
//! - LD Red2   : salud del supervisor — fijo si el sistema está sano; parpadeo
//!   lento ~1 Hz si alguna tarea murió ("degradado").
//! - LD Green2 : fault contenido — se enciende y queda latcheado al primer fault
//!   que el failsafe contiene.

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
use rugus_hal_stm32f7::exti::{self, Button};
use rugus_hal_stm32f7::gpio::{DiscoLed, LedPin};
use rugus_hal_stm32f7::iwdg::Iwdg;
use rugus_hal_stm32f7::pac;
use rugus_hal_stm32f7::rcc;
use rugus_hal_stm32f7::usart::{Usart2, CONSOLE_BAUD};
use rugus_runtime::entry;

#[repr(C, align(4096))]
struct Stack4k([u8; 4096]);

static mut STACK_KERNEL: Stack4k = Stack4k([0; 4096]);
static mut STACK_GOOD: Stack4k = Stack4k([0; 4096]);
static mut STACK_BAD: Stack4k = Stack4k([0; 4096]);

/// Índice (= TaskId) de bad_app según el orden de spawn de [`main`].
const BAD_IDX: usize = 1;
/// Índice (= TaskId) de good_app según el orden de spawn de [`main`].
const GOOD_IDX: usize = 2;

/// Cadencia de muestreo del supervisor (~40 ms a 216 MHz): suficientemente
/// fino para que cada LED dibuje su patrón propio sin entrar en `wfi`.
const SAMPLE_CYCLES: u32 = 216_000_000 / 25;

/// Mensaje IPC "conmuta el LED de userland": good_app lo envía por syscall y el
/// supervisor privilegiado lo ejecuta sobre el GPIO. Protocolo opaco al kernel.
const IPC_TOGGLE_USER: u32 = 1;

static mut LED_ALIVE: Option<LedPin> = None;
static mut LED_USER: Option<LedPin> = None;
static mut LED_SUPERVISOR: Option<LedPin> = None;
static mut LED_FAULT: Option<LedPin> = None;
/// Watchdog independiente: el supervisor lo alimenta en cada muestreo. Si el
/// kernel se cuelga y deja de hacerlo, el IWDG resetea el chip (~2 s).
static mut WATCHDOG: Option<Iwdg> = None;
/// Botón B1 (PA0) cableado a EXTI0. Mantiene viva la config del IRQ; el conteo
/// de eventos lo lee el supervisor por [`exti::events`].
static mut BUTTON: Option<Button> = None;

fn kernel_task() -> ! {
    defmt::info!("kernel task (LD Red) started");
    let mut last_log_s = u32::MAX;
    let mut respawns = 0u32;
    let mut last_btn = exti::events();
    loop {
        let now = time::now_ms();
        // IRQ→tarea: el handler EXTI0 contabiliza pulsaciones del botón B1; aquí
        // (contexto de tarea) observamos el contador y reaccionamos. Un IRQ real
        // de periférico llega así a código de tarea sin tocar el scheduler.
        let btn = exti::events();
        if btn != last_btn {
            defmt::info!("supervisor: button events={=u32}", btn);
            last_btn = btn;
        }
        // Alimenta el watchdog: mientras el supervisor late, el sistema vive. El
        // WFI terminal (todas las tareas muertas) deja de alimentarlo → reset.
        // SAFETY: solo esta tarea privilegiada toca el handle, cooperativa.
        unsafe {
            if let Some(wdt) = WATCHDOG.as_ref() {
                wdt.kick();
            }
        }
        // Autorreparación: si un fault mató a bad_app, la respawnea desde cero.
        // bad_app volverá a faultar (acceso prohibido) y el ciclo se repite, lo
        // que demuestra visiblemente kill→respawn→re-kill sin tumbar el sistema.
        if rugus_kernel::task_killed(BAD_IDX) && rugus_kernel::respawn(BAD_IDX) {
            respawns += 1;
            defmt::info!("supervisor: respawned bad_app (#{=u32})", respawns);
        }
        let killed = rugus_kernel::killed_count();
        // SAFETY: los LEDs solo los toca esta tarea privilegiada, cooperativa.
        unsafe {
            if let Some(led) = LED_ALIVE.as_mut() {
                let _ = if heartbeat(now) { led.set_high() } else { led.set_low() };
            }
            // I/O userland por IPC: drena las peticiones que good_app envió por
            // syscall y actúa sobre el GPIO en su nombre (dominio Drivers). Si
            // good_app murió, apaga el LED para reflejar que ya no hay actividad.
            if rugus_kernel::task_killed(GOOD_IDX) {
                if let Some(led) = LED_USER.as_mut() {
                    let _ = led.set_low();
                }
            } else {
                while let Some(msg) = rugus_kernel::ipc_try_recv() {
                    if msg == IPC_TOGGLE_USER {
                        if let Some(led) = LED_USER.as_mut() {
                            let _ = led.toggle();
                        }
                    }
                }
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
        // Cortex-M el WFI apaga el reloj de debug y ST-Link/probe-rs pierde RTT.
        // La ruta de bajo consumo (sleep/wake real) la ejercita `good_app`.
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

/// Parpadeo lento ~1 Hz para señalar estado degradado.
#[inline]
fn degraded_blink(now_ms: u32) -> bool {
    (now_ms / 500) % 2 == 0
}

fn good_app() -> ! {
    loop {
        // Conmuta su LED pidiéndoselo al driver privilegiado por IPC: userland
        // NO toca GPIO (lo prohíbe la MPU, dominio Drivers), enruta por syscall.
        let _ = svc_user::ipc_send(0, IPC_TOGGLE_USER);
        // Sleep real vía syscall: no busy-wait; el scheduler corre otras tareas.
        let _ = svc_user::sleep_ms(150);
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

    // Autotest USART2 (HDSEL single-wire loopback): valida el periférico por
    // RTT sin cablear pines — PA2 reinyecta en el receptor.
    usart_selftest(clocks.pclk1);

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

    // Botón B1 (PA0) por EXTI0 — primer IRQ no-SysTick. Autotest por SWIER (pende
    // el EXTI por software, igual que un flanco real) validado por RTT sin pulsar.
    unsafe {
        BUTTON = Some(Button::new());
    }
    button_selftest();

    // Watchdog independiente: a partir de aquí el supervisor debe alimentarlo en
    // cada latido o el chip se resetea (~2 s). Es la red de seguridad última.
    unsafe {
        WATCHDOG = Some(Iwdg::start());
    }
    defmt::info!("IWDG armed (~2 s reload)");

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

/// Autotest de USART2 por loopback single-wire (HDSEL): transmite un patrón y
/// lo lee de vuelta, reportando PASS/FAIL por RTT. Prueba el driver completo
/// (relojes, BRR, AF, TX, RX) sin hardware externo.
fn usart_selftest(pclk1: u32) {
    let mut u = Usart2::new_loopback(pclk1, CONSOLE_BAUD);
    const PATTERN: &[u8] = b"RUGUS-UART";
    let mut ok = true;
    for &tx in PATTERN {
        u.write_byte(tx);
        match u.read_byte_timeout(200_000) {
            Some(rx) if rx == tx => {}
            other => {
                defmt::warn!("USART2 loopback: tx={=u8} rx={:?}", tx, other);
                ok = false;
                break;
            }
        }
    }
    if ok {
        defmt::info!("USART2 loopback selftest: PASS ({=usize} bytes)", PATTERN.len());
    } else {
        defmt::warn!("USART2 loopback selftest: FAIL");
    }
}

/// Autotest del camino EXTI0: pende la línea del botón por software (`SWIER`) y
/// confirma que el handler la entregó (el contador de eventos sube). Prueba
/// NVIC→ISR→tarea sin pulsar el botón, reportando PASS/FAIL por RTT.
fn button_selftest() {
    let before = exti::events();
    // SAFETY: BUTTON se inicializó justo antes en main.
    unsafe {
        if let Some(btn) = BUTTON.as_ref() {
            btn.trigger_test();
        }
    }
    // El IRQ es asíncrono: espera acotada a que el handler corra.
    let mut ok = false;
    for _ in 0..100_000 {
        if exti::events() != before {
            ok = true;
            break;
        }
        core::hint::spin_loop();
    }
    if ok {
        defmt::info!("EXTI0 button selftest: PASS (events={=u32})", exti::events());
    } else {
        defmt::warn!("EXTI0 button selftest: FAIL (no IRQ delivered)");
    }
}

fn spin_delay() {
    for _ in 0..500_000 {
        core::hint::spin_loop();
    }
}
