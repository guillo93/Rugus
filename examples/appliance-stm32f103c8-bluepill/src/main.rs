//! Rugus lite appliance — F103 Blue Pill, fases 1–6 integradas.

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

mod heartbeat;
mod services;

use core::ptr::{addr_of, addr_of_mut};

use rugus_arch_cortex_m::{enable_fault_handlers, set_fault_hook, CortexM};
use rugus_core::fault::FaultReport;
use rugus_core::sched::{Priority, Scheduler};
use rugus_core::syscall::lite;
use rugus_hal::SerialPort;
use rugus_hal_stm32f1::i2c::I2c1;
use rugus_hal_stm32f1::pac;
use rugus_hal_stm32f1::postmortem;
use rugus_hal_stm32f1::rcc;
use rugus_hal_stm32f1::spi_sd::Spi1Sd;
use rugus_hal_stm32f1::uart::{Usart1, CLI_BAUD};
use rugus_hal_stm32f1::uart2::{Usart2, MODULE_BAUD};
use rugus_hal_stm32f1::wdt::Watchdog;
use rugus_runtime as _; // panic-probe + defmt-rtt
use rugus_runtime::entry;
use rush::Write;
use rush::{execute, identify, parse};

type Sched = Scheduler<CortexM>;

static mut SCHEDULER: Sched = Sched::new();
static mut STACK_CLI: [u8; 1536] = [0; 1536];
static mut STACK_HB: [u8; 1024] = [0; 1024];
/// Pila de la tarea víctima de `sting`. Solo corre brevemente antes de faultar;
/// el failsafe la mata. Se reutiliza entre stings sucesivos (solo una viva).
static mut STACK_STING: [u8; 512] = [0; 512];
static mut CONSOLE: Option<Usart1> = None;
static mut LINE: [u8; 128] = [0; 128];
static mut LINE_LEN: usize = 0;
static mut IWDG_PTR: *const pac::IWDG = core::ptr::null();

struct UartWriter;

impl Write for UartWriter {
    fn write_str(&mut self, s: &str) -> Result<(), ()> {
        // SAFETY: consola única en tarea CLI cooperativa.
        unsafe {
            if let Some(u) = CONSOLE.as_mut() {
                u.write(s.as_bytes()).map_err(|_| ())?;
            }
        }
        Ok(())
    }
}

fn cli_task() -> ! {
    defmt::info!("cli task started");
    let mut writer = UartWriter;
    let banner = "\r\nRugus lite appliance ready.\r\nType `orbit` for help.\r\n\r\n";
    let _ = writer.write_str(banner);

    loop {
        cli_poll_line(&mut writer);
        services::poll_identify_usart2();
        yield_cpu();
    }
}

fn cli_poll_line(writer: &mut UartWriter) {
    // SAFETY: solo tarea CLI lee consola.
    let byte = unsafe { CONSOLE.as_mut().and_then(|u| u.try_read_byte()) };

    let Some(b) = byte else {
        return;
    };

    heartbeat::note(heartbeat::UART_RX);

    // Fast-path: byte de control ENQ (0x05) → respuesta IDENTIFY inmediata.
    if b == identify::ENQ {
        identify::write_signature(writer, identify::TIER, identify::CHIP);
        heartbeat::note(heartbeat::CLI_CMD);
        return;
    }

    unsafe {
        if b == b'\r' || b == b'\n' {
            if LINE_LEN > 0 {
                let line = core::str::from_utf8(&LINE[..LINE_LEN]).unwrap_or("");
                let cmd = parse(line);
                execute(cmd, line, writer);
                heartbeat::note(heartbeat::CLI_CMD);
                LINE_LEN = 0;
            }
        } else if b == 0x7F || b == 0x08 {
            if LINE_LEN > 0 {
                LINE_LEN -= 1;
                let _ = writer.write_str("\x08 \x08");
            }
        } else if LINE_LEN < LINE.len() {
            LINE[LINE_LEN] = b;
            LINE_LEN += 1;
            let ch = [b];
            if let Ok(s) = core::str::from_utf8(&ch) {
                let _ = writer.write_str(s);
            }
        }
    }
}

fn heartbeat_task() -> ! {
    defmt::info!("heartbeat task (PC13 activity LED)");
    heartbeat::led_off();
    let mut tick: u32 = 0;
    loop {
        let act = heartbeat::level();
        tick = tick.wrapping_add(1);
        let (on, delay_cycles) = heartbeat::step(act, tick);
        if on {
            heartbeat::led_on();
        } else {
            heartbeat::led_off();
        }
        // `step` devuelve el retardo en ciclos de CPU (interfaz histórica); a
        // 8 MHz HCLK son 8000 ciclos/ms. Dormimos ese tiempo cediendo el CPU.
        sleep_ms((delay_cycles / 8_000).max(1));
    }
}

/// Espera cooperativa de `ms` milisegundos: cede el CPU y alimenta el watchdog
/// mientras el reloj monotónico de SysTick no alcance el plazo. No hace
/// busy-wait: el scheduler sigue corriendo la tarea CLI (sondeo UART) entre
/// cesiones, así no se pierden bytes RX.
fn sleep_ms(ms: u32) {
    let start = rugus_arch_cortex_m::time::now_ms();
    while rugus_arch_cortex_m::time::elapsed_ms(start) < ms {
        kick_wdt();
        yield_cpu();
    }
}

#[entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("cortex-m");
    let dp = pac::Peripherals::take().expect("device");

    // Recarga IWDG heredado de un flash anterior (~100 ms) antes de init lenta.
    dp.IWDG.kr.write(|w| unsafe { w.key().bits(0xAAAA) });

    let clocks = rcc::init(&dp);
    rugus_runtime::enable_cycle_counter(&mut cp);
    // Reloj monotónico: SysTick a 1 kHz desde el HCLK. Base de uptime, sleep
    // cooperativo y timeouts; reemplaza los busy-delay por cesión de CPU.
    rugus_arch_cortex_m::time::init(&mut cp.SYST, clocks.hclk);

    // Post-mortem que cruza el reset: causa de reinicio (RCC_CSR) y, si el
    // arranque anterior murió por un fault contenido, kind + tarea desde los
    // backup registers (sobreviven al reset del IWDG).
    let reset_cause = postmortem::read_reset_cause(&dp.RCC).name();
    let last_fault = postmortem::take_fault(&dp.RCC, &dp.PWR, &dp.BKP);

    defmt::info!("appliance F103 boot");

    let wdt = Watchdog::disabled();
    unsafe {
        IWDG_PTR = &dp.IWDG as *const _;
    }

    let console = Usart1::new(&dp.RCC, dp.USART1, clocks.pclk2, CLI_BAUD);
    kick_wdt();
    let i2c = I2c1::new(&dp.RCC, dp.I2C1);
    kick_wdt();
    let sd = Spi1Sd::new(&dp.RCC, dp.SPI1);
    kick_wdt();
    let modules = Usart2::new(&dp.RCC, dp.USART2, clocks.pclk1, MODULE_BAUD);
    kick_wdt();

    services::init(&dp.RCC, i2c, sd, modules, wdt);
    kick_wdt();

    // Failsafe del kernel: handlers de fault dedicados + hook que mata la tarea
    // faultante (no panic global). En el Cortex-M3 del F103 no hay MPU, así que
    // MemManage nunca dispara; BusFault/UsageFault/HardFault sí, y el hook los
    // contiene matando solo la tarea culpable.
    enable_fault_handlers(&mut cp.SCB);
    unsafe {
        set_fault_hook(fault_hook);
        // Reporte de fault preciso: task id y dominio desde el scheduler vivo.
        rugus_core::syscall::register(rugus_core::syscall::Hooks {
            yield_now: yield_cpu,
            sleep_ms: |ms| (&mut *addr_of_mut!(SCHEDULER)).sleep_ms(ms),
            current_task_id: || (&*addr_of!(SCHEDULER)).current_id(),
            current_domain: || (&*addr_of!(SCHEDULER)).current_domain(),
            current_user_region: || (&*addr_of!(SCHEDULER)).current_user_region(),
            // El appliance lite no expone IPC userland: rechaza con Einval.
            ipc_send: |_chan, _msg| rugus_core::Errno::Einval as i32,
            // Tampoco expone sincronización userland (sin scheduler multitarea).
            mutex_lock: |_id| rugus_core::Errno::Einval as i32,
            mutex_unlock: |_id| rugus_core::Errno::Einval as i32,
            sem_wait: |_id| rugus_core::Errno::Einval as i32,
            sem_post: |_id| rugus_core::Errno::Einval as i32,
            // Ni IPC bloqueante por canal (sin multitarea que bloquear).
            chan_send: |_chan, _msg, _to| rugus_core::Errno::Einval as i32,
            chan_recv: |_chan, _to, _out| rugus_core::Errno::Einval as i32,
            // El appliance lite no usa el monitor de liveness del scheduler full:
            // el checkin es un no-op inofensivo.
            checkin: || {},
        });
        lite::register(services::hooks());
        CONSOLE = Some(console);
        let iwdg = &*IWDG_PTR;
        let mut wdt = Watchdog::configure(iwdg);
        wdt.arm(iwdg);
        services::set_wdt(wdt);
    }
    kick_wdt();

    defmt::info!("appliance ready");

    unsafe {
        let sched = &mut *addr_of_mut!(SCHEDULER);
        sched
            .spawn(&mut *addr_of_mut!(STACK_CLI), cli_task, Priority::App)
            .expect("spawn cli");
        sched
            .spawn(&mut *addr_of_mut!(STACK_HB), heartbeat_task, Priority::App)
            .expect("spawn heartbeat");
        services::set_task_count(2);
        services::set_stack_probe(task_stack_usage);
        services::set_sting_spawn(sting_spawn);
        services::set_boot_info(reset_cause, last_fault.map(|f| (f.kind, f.task)));
        defmt::info!("scheduler: cli + heartbeat tasks");
        sched.start();
    }
}

/// Política de fault del kernel lite: registra el fault, mata SOLO la tarea
/// faultante y reanuda la siguiente lista. El appliance sobrevive (watchdog +
/// heartbeat siguen vivos) en vez de tumbar todo el dispositivo con un panic
/// global. Si no quedan tareas vivas, el scheduler hace WFI y el IWDG resetea.
fn fault_hook(report: FaultReport) -> ! {
    defmt::error!(
        "task fault {} domain={} pc={=u32:#x} task={=u8} -> kill+resume",
        report.kind.name(),
        report.domain.name(),
        report.pc,
        report.task_id.0
    );
    // Graba el post-mortem en el dominio de respaldo ANTES de matar la tarea:
    // si el watchdog acaba reseteando, el próximo arranque podrá decir qué pasó.
    // SAFETY: contexto de fault, single-thread.
    unsafe {
        rugus_hal_stm32f1::postmortem::save_fault(report.kind as u8, report.task_id.0);
    }
    // Cuenta el fault contenido y deja la cicatriz visible en `ecosystem`/`scar`.
    services::note_fault(report.kind as u8, report.task_id.0);
    // SAFETY: en contexto de fault (handler mode), single-threaded; el scheduler
    // está activo y `current` es la tarea faultante.
    unsafe { (&mut *addr_of_mut!(SCHEDULER)).kill_current_and_resume(report) }
}

/// Sonda de stack para `services` (`coil`): high-water y total de la tarea
/// `idx`, leídos del scheduler vivo. Aislamos aquí el acceso al estático para
/// que la capa servicio no dependa de la dirección del scheduler.
fn task_stack_usage(idx: usize) -> (u32, u32) {
    // SAFETY: lectura del scheduler; cooperativo, sin reentrada concurrente.
    unsafe {
        let sched = &*addr_of!(SCHEDULER);
        (sched.stack_high_water(idx), sched.stack_len(idx))
    }
}

/// Tarea víctima de `sting`: ejecuta una instrucción indefinida → UsageFault.
/// El failsafe la mata y reanuda; CLI y heartbeat sobreviven. No retorna.
fn sting_task() -> ! {
    cortex_m::asm::udf()
}

/// Spawnea la tarea víctima de `sting` en el scheduler vivo. Devuelve 0 si la
/// armó, `Enomem` (-7) si no quedan slots (cada víctima consume uno hasta el
/// próximo reset). La invoca el hook `sting` desde la tarea CLI.
fn sting_spawn() -> i32 {
    // SAFETY: scheduler cooperativo, sin reentrada concurrente desde la CLI.
    unsafe {
        let sched = &mut *addr_of_mut!(SCHEDULER);
        match sched.spawn(&mut *addr_of_mut!(STACK_STING), sting_task, Priority::App) {
            Ok(_) => {
                services::set_task_count(sched.task_count() as u32);
                0
            }
            Err(_) => -7,
        }
    }
}

fn yield_cpu() {
    unsafe {
        (&mut *addr_of_mut!(SCHEDULER)).yield_now();
    }
}

fn kick_wdt() {
    // SAFETY: IWDG compartido con tarea heartbeat.
    unsafe {
        if !IWDG_PTR.is_null() {
            (&(*IWDG_PTR)).kr.write(|w| w.key().bits(0xAAAA));
        }
    }
}
