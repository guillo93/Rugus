//! Rugus F5.A.3 — **consola de energía** sobre tick dinámico en la STM32F407G-DISC1.
//!
//! Ejemplo dedicado para validar el comando `power` de la consola de operador
//! por un terminal UART real. A diferencia de `app-sandbox`, aquí NO hay tareas
//! CPU-bound: todas las tareas duermen con `cpu_sleep_ms`, de modo que el
//! scheduler entra en idle a menudo y, con la feature `tickless`, el SysTick se
//! reprograma al próximo plazo y el núcleo hace `wfi` entre medias. El resultado
//! es un **idle % alto y real** que el comando `power` reporta.
//!
//! La contabilidad de energía (F5.A.3) la lleva la capa de tiempo del backend
//! arch (`time::idle_ms`/`systick_irqs`/`stop_entries`); este `main` registra un
//! proveedor `PowerStats` (`rugus_kernel::set_power_provider`) que la consola
//! consulta. Como el modo STOP es exclusivo del F7, aquí `stop_entries` es
//! siempre 0 (correcto: el F407 no implementa esa ruta).
//!
//! Cableado del USB-TTL (3V3): GND↔GND, RX_del_TTL↔PA2 (TX de la placa),
//! TX_del_TTL↔PA3 (RX de la placa), 115200 8N1. Cruza RX/TX; no conectes Vcc.
//! Abre un terminal serie y escribe `help`, `ps`, `mem` o `power`.

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use core::ptr::addr_of_mut;

use cortex_m::peripheral::NVIC;
use rugus_arch_cortex_m::{platform_init, time, MpuLayout};
use rugus_core::sched::Priority;
use rugus_hal::GpioPin;
use rugus_hal_stm32f4::gpio::{DiscoLed, LedPin};
use rugus_hal_stm32f4::pac;
use rugus_hal_stm32f4::pac::{interrupt, Interrupt};
use rugus_hal_stm32f4::rcc;
use rugus_hal_stm32f4::usart::{self, Usart2, CONSOLE_BAUD};
use rugus_kernel::console::{Console, ConsoleOut, RxRing};
use rugus_kernel::PowerStats;
use rugus_runtime::entry;

#[repr(C, align(4096))]
struct Stack4k([u8; 4096]);

static mut STACK_SUP: Stack4k = Stack4k([0; 4096]);
static mut STACK_BLINK: Stack4k = Stack4k([0; 4096]);

/// LED de latido (LD4 verde), poseído por la tarea de parpadeo.
static mut HB_LED: Option<LedPin> = None;

/// Anillo de recepción de la consola: el handler `USART2` (productor) encola cada
/// byte; el supervisor (consumidor) lo drena hacia [`CONSOLE`]. SPSC sin bloqueo.
static RX_RING: RxRing = RxRing::new();
/// Consola de operador interactiva: parsea ps/mem/faults/power/...
static mut CONSOLE: Console = Console::new();
/// Puerto UART de la consola (PA2 TX / PA3 RX). Lo conduce el supervisor para el
/// eco y las respuestas; el RX llega por IRQ vía [`RX_RING`].
static mut CONSOLE_UART: Option<Usart2> = None;

/// Sumidero de salida de la consola sobre el UART: escribe byte a byte.
struct UartSink<'a>(&'a mut Usart2);

impl ConsoleOut for UartSink<'_> {
    fn write_str(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.0.write_byte(b);
        }
    }
}

/// Handler de USART2: drena el byte recibido al anillo de la consola. Leer `DR`
/// limpia `RXNE` y desactiva la pendiente de la IRQ.
#[interrupt]
fn USART2() {
    if let Some(b) = usart::isr_read_byte() {
        let _ = RX_RING.push(b);
    }
}

/// Proveedor de métricas de energía para la consola (F5.A.3): mapea los
/// contadores de la capa de tiempo del backend arch a `PowerStats`. En el F407
/// `stop_entries` es 0 (STOP es exclusivo del F7).
fn power_provider() -> PowerStats {
    PowerStats {
        uptime_ms: time::now_ms(),
        idle_ms: time::idle_ms(),
        systick_irqs: time::systick_irqs(),
        // El F407 no implementa modo STOP (es exclusivo del F7), así que no hay
        // entradas a STOP que contabilizar.
        stop_entries: 0,
    }
}

#[entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("cortex-m peripherals");
    let dp = pac::Peripherals::take().expect("device peripherals");

    let clocks = rcc::init(&dp);
    rugus_runtime::enable_cycle_counter(&mut cp);

    defmt::info!(
        "rugus console-power @ STM32F407G-DISC1, SYSCLK {} MHz (tickless)",
        clocks.sysclk_mhz()
    );

    // FPU-context + fault handlers + layout MPU.
    platform_init(&mut cp, &MpuLayout::STM32F407);

    // Base de tiempo del kernel: SysTick a 1 kHz; con `tickless`, el scheduler la
    // reprograma al próximo plazo en idle y el núcleo hace `wfi` entre medias.
    time::init(&mut cp.SYST, clocks.hclk);

    // Publica las métricas de energía para el comando `power` de la consola.
    // SAFETY: arranque single-thread; el proveedor se registra una sola vez.
    unsafe {
        rugus_kernel::set_power_provider(power_provider);
    }

    // SAFETY: arranque single-thread; el LED se inicializa una sola vez aquí.
    unsafe {
        HB_LED = Some(LedPin::new(&dp.RCC, DiscoLed::Green));
    }

    // Consola de operador: PA2 TX / PA3 RX @ 115200 8N1, RX por IRQ.
    // SAFETY: arranque single-thread; el UART se inicializa una sola vez.
    unsafe {
        let mut uart = Usart2::new(clocks.pclk1, CONSOLE_BAUD);
        uart.enable_rx_irq();
        NVIC::unmask(Interrupt::USART2);
        CONSOLE_UART = Some(uart);
    }
    defmt::info!("UART console ready (PA2/PA3 @ 115200, RX IRQ) — escribe `power`");

    // SAFETY: arranque single-thread; pilas estáticas vivas para todo el kernel.
    unsafe {
        rugus_kernel::install(None);
        rugus_kernel::spawn(
            &mut (*addr_of_mut!(STACK_SUP)).0,
            supervisor_task,
            Priority::Kernel,
        )
        .expect("spawn supervisor");
        rugus_kernel::spawn(
            &mut (*addr_of_mut!(STACK_BLINK)).0,
            blink_task,
            Priority::Kernel,
        )
        .expect("spawn blink");
        defmt::info!("scheduler: 2 tareas (consola + blink durmiente), starting");
        rugus_kernel::start();
    }
}

/// Supervisor: emite el banner una vez y drena la consola periódicamente. Duerme
/// entre sondeos (~30 ms) cediendo el CPU; con tickless el scheduler entra en
/// `wfi`, acumulando idle real que reporta el comando `power`.
fn supervisor_task() -> ! {
    loop {
        // SAFETY: solo esta tarea toca la consola y su UART.
        unsafe {
            if let Some(u) = CONSOLE_UART.as_mut() {
                let mut sink = UartSink(u);
                CONSOLE.greet(&mut sink);
                while let Some(b) = RX_RING.pop() {
                    CONSOLE.feed(b, &mut sink);
                }
            }
        }
        rugus_kernel::cpu_sleep_ms(30);
    }
}

/// Tarea de latido: parpadea LD4 (verde) cada 500 ms durmiendo entre toggles.
/// Su sueño largo deja al scheduler ocioso la mayor parte del tiempo → idle alto.
fn blink_task() -> ! {
    loop {
        // SAFETY: solo esta tarea toca el LED de latido.
        unsafe {
            if let Some(led) = (*addr_of_mut!(HB_LED)).as_mut() {
                let _ = led.toggle();
            }
        }
        rugus_kernel::cpu_sleep_ms(500);
    }
}
