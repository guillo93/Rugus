//! Rugus lite appliance — F103 Blue Pill, fases 1–6 integradas.

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

mod heartbeat;
mod services;

use core::ptr::addr_of_mut;

use rugus_arch_cortex_m::CortexM;
use rugus_cli::Write;
use rugus_cli::{execute, parse};
use rugus_core::sched::{Priority, Scheduler};
use rugus_core::syscall::lite;
use rugus_hal::SerialPort;
use rugus_hal_stm32f1::i2c::I2c1;
use rugus_hal_stm32f1::pac;
use rugus_hal_stm32f1::rcc;
use rugus_hal_stm32f1::spi_sd::Spi1Sd;
use rugus_hal_stm32f1::uart::{Usart1, CLI_BAUD};
use rugus_hal_stm32f1::uart2::{Usart2, MODULE_BAUD};
use rugus_hal_stm32f1::wdt::Watchdog;
use rugus_runtime as _; // panic-probe + defmt-rtt
use rugus_runtime::entry;

type Sched = Scheduler<CortexM>;

static mut SCHEDULER: Sched = Sched::new();
static mut STACK_CLI: [u8; 1536] = [0; 1536];
static mut STACK_HB: [u8; 1024] = [0; 1024];
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
        let (on, delay) = heartbeat::step(act, tick);
        if on {
            heartbeat::led_on();
        } else {
            heartbeat::led_off();
        }
        cortex_m::asm::delay(delay);
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

    unsafe {
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
        defmt::info!("scheduler: cli + heartbeat tasks");
        sched.start();
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
