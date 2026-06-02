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
//! - LD6 azul    : actividad de userland — la conmuta la PROPIA good_app vía IPC
//!   (syscall IpcSend → buzón del kernel → el supervisor toca el GPIO en su
//!   nombre). Userland no accede al GPIO directamente; apagado fijo si murió.
//! - LD3 naranja : salud del supervisor — fijo si el sistema está sano; parpadeo
//!   lento ~1 Hz si alguna tarea murió ("degradado").
//! - LD5 rojo    : fault contenido — se enciende y queda latcheado al primer
//!   fault que el failsafe contiene.

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use rugus_arch_cortex_m::{platform_init, time, MpuLayout};
use rugus_core::sched::Priority;
use rugus_core::syscall::user as svc_user;
use rugus_hal::GpioPin;
use rugus_hal_stm32f4::adc::Adc;
use rugus_hal_stm32f4::exti::{self, Button};
use rugus_hal_stm32f4::gpio::{DiscoLed, LedPin};
use rugus_hal_stm32f4::iwdg::Iwdg;
use rugus_hal_stm32f4::pac;
use rugus_hal_stm32f4::rcc;
use rugus_hal_stm32f4::timer::{PwmCheck, Timebase};
use rugus_hal_stm32f4::usart::{Usart2, CONSOLE_BAUD};
use rugus_kernel::status::{self, StatusLeds};
use rugus_runtime::entry;

#[repr(C, align(4096))]
struct Stack4k([u8; 4096]);

static mut STACK_KERNEL: Stack4k = Stack4k([0; 4096]);
static mut STACK_GOOD: Stack4k = Stack4k([0; 4096]);
static mut STACK_BAD: Stack4k = Stack4k([0; 4096]);
static mut STACK_HOG: Stack4k = Stack4k([0; 4096]);

/// Índice (= TaskId) de bad_app según el orden de spawn de `main`.
const BAD_IDX: usize = 1;
/// Índice (= TaskId) de good_app según el orden de spawn de `main`.
const GOOD_IDX: usize = 2;

/// Cadencia de muestreo del supervisor (~40 ms a 168 MHz): suficientemente
/// fino para que cada LED dibuje su patrón propio sin entrar en `wfi`.
const SAMPLE_CYCLES: u32 = 168_000_000 / 25;

/// Mensaje IPC "conmuta el LED de userland": good_app lo envía por syscall y el
/// supervisor privilegiado lo ejecuta sobre el GPIO. Protocolo opaco al kernel.
const IPC_TOGGLE_USER: u32 = 1;
/// Ping IPC de [`hog_app`]: la tarea CPU-bound lo emite periódicamente para
/// demostrar que avanza pese a no ceder nunca el CPU. El supervisor lo cuenta.
const IPC_HOG_PING: u32 = 2;

/// LEDs de estado del kernel (latido/salud/fault) agrupados para el servicio
/// reutilizable [`status`]. El mapeo rol→pin y el tragado de errores de GPIO es
/// lo único específico de esta placa.
struct StatusBoard {
    alive: LedPin,
    health: LedPin,
    fault: LedPin,
}

impl StatusLeds for StatusBoard {
    fn set_alive(&mut self, on: bool) {
        let _ = if on {
            self.alive.set_high()
        } else {
            self.alive.set_low()
        };
    }
    fn set_health(&mut self, on: bool) {
        let _ = if on {
            self.health.set_high()
        } else {
            self.health.set_low()
        };
    }
    fn set_fault(&mut self, on: bool) {
        let _ = if on {
            self.fault.set_high()
        } else {
            self.fault.set_low()
        };
    }
}

static mut STATUS: Option<StatusBoard> = None;
/// LED de actividad de userland: lo conduce la PROPIA good_app vía IPC (no es
/// estado del kernel, por eso queda fuera del servicio [`status`]).
static mut LED_USER: Option<LedPin> = None;
/// Watchdog independiente: el supervisor lo alimenta en cada muestreo. Si el
/// kernel se cuelga y deja de hacerlo, el IWDG resetea el chip (~2 s).
static mut WATCHDOG: Option<Iwdg> = None;
/// Botón B1 (PA0) cableado a EXTI0. Mantiene viva la config del IRQ; el conteo
/// de eventos lo lee el supervisor por [`exti::events`].
static mut BUTTON: Option<Button> = None;

fn kernel_task() -> ! {
    defmt::info!("kernel task (LD4) started");
    let mut last_log_s = u32::MAX;
    let mut respawns = 0u32;
    let mut last_btn = exti::events();
    let mut hog_pings = 0u32;
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
            // Estado del kernel (latido/salud/fault): patrones y latch viven en
            // el servicio reutilizable del kernel; la placa solo aporta los pines.
            if let Some(s) = STATUS.as_mut() {
                status::refresh(now, s);
            }
            // I/O userland por IPC: drena las peticiones que las apps enviaron
            // por syscall y actúa en su nombre (dominio Drivers). good_app pide
            // conmutar su LED; hog_app emite pings que contamos para evidenciar
            // que la preempción la mantiene viva pese a no ceder nunca.
            let good_alive = !rugus_kernel::task_killed(GOOD_IDX);
            while let Some(msg) = rugus_kernel::ipc_try_recv() {
                match msg {
                    IPC_TOGGLE_USER if good_alive => {
                        if let Some(led) = LED_USER.as_mut() {
                            let _ = led.toggle();
                        }
                    }
                    IPC_HOG_PING => hog_pings = hog_pings.wrapping_add(1),
                    _ => {}
                }
            }
            if !good_alive {
                if let Some(led) = LED_USER.as_mut() {
                    let _ = led.set_low();
                }
            }
        }
        // Log throttled a ~1/s (el muestreo de LEDs corre mucho más rápido).
        let now_s = now / 1000;
        if now_s != last_log_s {
            last_log_s = now_s;
            // Que el supervisor siga logueando pese a hog_app (bucle infinito sin
            // syscalls) demuestra la preempción: sin time-slice, hog monopolizaría
            // el CPU, el supervisor no alimentaría el IWDG y la placa se resetearía
            // a los ~2 s. `hog pings` creciendo confirma que hog también avanza.
            defmt::debug!(
                "supervisor: alive killed={=usize} hog_pings={=u32} @ {=u32} ms",
                killed,
                hog_pings,
                now
            );
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

fn good_app() -> ! {
    loop {
        // Conmuta su LED pidiéndoselo al driver privilegiado por IPC: userland
        // NO toca GPIO (lo prohíbe la MPU, dominio Drivers), enruta por syscall.
        let _ = svc_user::ipc_send(0, IPC_TOGGLE_USER);
        // Sleep real vía syscall: no busy-wait; el scheduler corre otras tareas.
        let _ = svc_user::sleep_ms(150);
    }
}

/// Tarea CPU-bound que NUNCA cede el CPU voluntariamente: un bucle cerrado sin
/// `sleep`/`yield`. Solo emite un ping IPC cada cierto número de vueltas (el
/// syscall encola y retorna; no es un punto de cesión). Es el testigo de la
/// preempción: sin time-slice, monopolizaría el núcleo y el supervisor moriría
/// (→ reset por watchdog); con F3.7, SysTick la expulsa cada rodaja.
fn hog_app() -> ! {
    let mut spins = 0u32;
    loop {
        spins = spins.wrapping_add(1);
        if spins % 2_000_000 == 0 {
            let _ = svc_user::ipc_send(0, IPC_HOG_PING);
        }
        core::hint::spin_loop();
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

    // Autotest USART2 (HDSEL single-wire loopback): valida el periférico por
    // RTT sin cablear pines — PA2 reinyecta en el receptor.
    usart_selftest(clocks.pclk1);

    platform_init(&mut cp, &MpuLayout::STM32F407);
    time::init(&mut cp.SYST, clocks.hclk);

    // Autotest de periféricos analógicos/temporización (TIM2 base µs, TIM3 PWM,
    // ADC1 VREFINT). El reloj de los timers es pclk1*2 (prescaler APB1 ≠ 1).
    peripheral_selftest(clocks.pclk1 * 2);

    // LEDs de estado (todos en GPIOD): verde=latido, naranja=salud, rojo=fault
    // (los tres conducidos por el servicio `status`); azul=actividad userland.
    unsafe {
        let mut fault = LedPin::new(&dp.RCC, DiscoLed::Red);
        let _ = fault.set_low();
        STATUS = Some(StatusBoard {
            alive: LedPin::new(&dp.RCC, DiscoLed::Green),
            health: LedPin::new(&dp.RCC, DiscoLed::Orange),
            fault,
        });
        LED_USER = Some(LedPin::new(&dp.RCC, DiscoLed::Blue));
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
        // El LED de fault lo conduce ahora el servicio `status` desde el latch
        // del kernel; no hace falta observer de plataforma solo para el LED.
        rugus_kernel::install(None);
        rugus_kernel::spawn(
            &mut (*core::ptr::addr_of_mut!(STACK_KERNEL)).0,
            kernel_task,
            Priority::Kernel,
        )
        .expect("spawn kernel");
        // bad_app y good_app comparten banda App y rotan justo (round-robin por
        // banda): el orden de spawn no decide cuál corre. GOOD_IDX debe coincidir
        // con el orden de spawn de userland.
        rugus_kernel::spawn_user(
            &mut (*core::ptr::addr_of_mut!(STACK_BAD)).0,
            bad_app,
            Priority::App,
        )
        .expect("spawn bad app");
        rugus_kernel::spawn_user(
            &mut (*core::ptr::addr_of_mut!(STACK_GOOD)).0,
            good_app,
            Priority::App,
        )
        .expect("spawn good app");
        // hog_app: bucle CPU-bound sin cesión. Es el testigo de la preempción.
        rugus_kernel::spawn_user(
            &mut (*core::ptr::addr_of_mut!(STACK_HOG)).0,
            hog_app,
            Priority::App,
        )
        .expect("spawn hog app");

        defmt::info!("scheduler: 4 tasks (1 kernel + 3 userland), starting");
        rugus_kernel::start();
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
        defmt::info!(
            "USART2 loopback selftest: PASS ({=usize} bytes)",
            PATTERN.len()
        );
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
        defmt::info!(
            "EXTI0 button selftest: PASS (events={=u32})",
            exti::events()
        );
    } else {
        defmt::warn!("EXTI0 button selftest: FAIL (no IRQ delivered)");
    }
}

/// Autotest de temporización y analógico, todo por RTT y sin hardware externo:
/// - TIM2 (base µs): cruza un `delay_us(50_000)` contra el SysTick (`now_ms`);
///   PASS si el delta cae en ~[45, 55] ms (ambos relojes son independientes).
/// - TIM3 (PWM): genera duty 250/1000 y lo mide muestreando `CNT < CCR`; PASS
///   si el duty estimado cae en [150, 350] por mil.
/// - ADC1 (VREFINT, canal 17): convierte la referencia interna; PASS si el
///   valor crudo de 12 bits cae en el rango plausible [800, 2400].
fn peripheral_selftest(timer_clk: u32) {
    let tb = Timebase::start(timer_clk);
    let t0 = time::now_ms();
    tb.delay_us(50_000);
    let dt = time::now_ms().wrapping_sub(t0);
    if (45..=55).contains(&dt) {
        defmt::info!(
            "TIM2 timebase selftest: PASS (delay 50 ms ~= {=u32} ms)",
            dt
        );
    } else {
        defmt::warn!("TIM2 timebase selftest: FAIL (delta={=u32} ms)", dt);
    }

    let pwm = PwmCheck::start(timer_clk, 999, 250);
    // El periodo PWM es 1000 ticks a 1 MHz (1 ms); con pocas muestras la ventana
    // de muestreo cae dentro de un solo periodo y el duty sale sesgado por la
    // fase. Con 2M muestras la ventana abarca decenas de periodos → media real.
    let duty = pwm.measure_duty_permille(2_000_000);
    if (150..=350).contains(&duty) {
        defmt::info!("TIM3 PWM selftest: PASS (duty={=u32} permille)", duty);
    } else {
        defmt::warn!("TIM3 PWM selftest: FAIL (duty={=u32} permille)", duty);
    }

    let adc = Adc::new();
    let raw = adc.read_vrefint_raw();
    if (800..=2400).contains(&raw) {
        defmt::info!("ADC1 VREFINT selftest: PASS (raw={=u16})", raw);
    } else {
        defmt::warn!("ADC1 VREFINT selftest: FAIL (raw={=u16})", raw);
    }
}

fn spin_delay() {
    for _ in 0..500_000 {
        core::hint::spin_loop();
    }
}
