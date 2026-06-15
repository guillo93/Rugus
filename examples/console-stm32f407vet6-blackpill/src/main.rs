//! Rugus G2.bp-full — **consola `rush` autenticada** sobre USART1 (header RX/TX)
//! en el clon STM32F407VET6 (FK407M3-VET6 v1.1).
//!
//! Convergencia de la consola del tier full F4 al léxico universal `rush`
//! (kernel multipersonalidad): los mismos verbos que la personalidad lite del
//! F103 y la consola de red del F769 (`cosmos`/`ecosystem`/`letargo`/`coil`/
//! `scar` + GPIO de placa), con **canal gateado**: sin autenticación
//! challenge-response HMAC (`knock`/`prove`) solo pasan IDENTIFY y el propio
//! handshake. La PSK vive en el sector 7 de la flash interna del VET6 (ver
//! [`psk`]/[`rugus_hal_stm32f4::flash`]), se aprovisiona una única vez con
//! `enroll` y sobrevive a los reflasheos (el linker excluye el sector).
//!
//! Conserva el carácter original del ejemplo (F5.A.3): tareas durmientes sobre
//! tick dinámico (`tickless`) → idle real alto, que ahora reporta el verbo
//! `letargo` vía el proveedor `PowerStats`.
//!
//! Cableado del USB-TTL al header `RX/TX`: GND↔GND, RX_del_TTL↔TX(PA9),
//! TX_del_TTL↔RX(PA10), 115200 8N1. Cruza RX/TX; no conectes Vcc.
//! Sesión: `knock` → `prove <hmac>` → verbos (`cosmos`, `coil`, `letargo`, …).

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

mod auth;
mod board;
mod psk;

use core::ptr::addr_of_mut;

use cortex_m::peripheral::NVIC;
use rugus_arch_cortex_m::{platform_init, time, MpuLayout};
use rugus_core::sched::Priority;
use rugus_hal::GpioPin;
use rugus_hal_stm32f4::flash::FlashWindow;
use rugus_hal_stm32f4::gpio::{Pin, PinConfig, Port};
use rugus_hal_stm32f4::pac;
use rugus_hal_stm32f4::pac::{interrupt, Interrupt};
use rugus_hal_stm32f4::rcc;
use rugus_hal_stm32f4::usart::{self, Usart1, CONSOLE_BAUD};
use rugus_kernel::console::RxRing;
use rugus_kernel::PowerStats;
use rugus_runtime::entry;
use rush::{execute_authed, identify, parse, AuthHooks, Session, Write};

/// Identidad para IDENTIFY/ENQ: tier full sobre silicio F407.
const TIER: &str = "full";
/// Chip reportado en la firma IDENTIFY.
const CHIP: &str = "f407vet6";

#[repr(C, align(4096))]
struct Stack4k([u8; 4096]);

static mut STACK_SUP: Stack4k = Stack4k([0; 4096]);
static mut STACK_BLINK: Stack4k = Stack4k([0; 4096]);

/// LED de latido (LD4 verde), poseído por la tarea de parpadeo.
static mut HB_LED: Option<Pin> = None;

/// Anillo de recepción de la consola: el handler `USART1` (productor) encola
/// cada byte; el supervisor (consumidor) lo drena línea a línea. SPSC sin bloqueo.
static RX_RING: RxRing = RxRing::new();
/// Puerto UART de la consola (PA9 TX / PA10 RX (header RX/TX)). Lo conduce el supervisor para el
/// eco y las respuestas; el RX llega por IRQ vía [`RX_RING`].
static mut CONSOLE_UART: Option<Usart1> = None;
/// Línea de comando en construcción (editada con eco + backspace).
static mut LINE: [u8; 128] = [0; 128];
static mut LINE_LEN: usize = 0;
/// Sesión de autenticación de la consola USART1 (challenge-response HMAC).
static mut SESSION: Session = Session::new();
/// Ganchos de autenticación (PSK en flash interna + HMAC + nonce); en `main`.
static mut AUTH_HOOKS: Option<AuthHooks> = None;

/// Sumidero de salida `rush` sobre el UART: escribe byte a byte.
struct UartSink;

impl Write for UartSink {
    fn write_str(&mut self, s: &str) -> Result<(), ()> {
        // SAFETY: consola única, conducida solo por la tarea supervisora.
        unsafe {
            if let Some(u) = CONSOLE_UART.as_mut() {
                for &b in s.as_bytes() {
                    u.write_byte(b);
                }
            }
        }
        Ok(())
    }
}

/// Handler de USART1 (header RX/TX): drena el byte recibido al anillo de la consola. Leer `DR`
/// limpia `RXNE` y desactiva la pendiente de la IRQ.
#[interrupt]
fn USART1() {
    if let Some(b) = usart::isr_read_byte_usart1() {
        let _ = RX_RING.push(b);
    }
}

/// Proveedor de métricas de energía para el verbo `letargo`: mapea los
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
        "rugus console @ Black STM32F407VET6 (FK407M3), SYSCLK {} MHz (tickless, rush)",
        clocks.sysclk_mhz()
    );

    // FPU-context + fault handlers + layout MPU.
    platform_init(&mut cp, &MpuLayout::STM32F407VE);

    // Base de tiempo del kernel: SysTick a 1 kHz; con `tickless`, el scheduler la
    // reprograma al próximo plazo en idle y el núcleo hace `wfi` entre medias.
    time::init(&mut cp.SYST, clocks.hclk);

    // SAFETY: arranque single-thread; instalaciones únicas antes de `start()`.
    unsafe {
        // Publica las métricas de energía para el verbo `letargo`.
        rugus_kernel::set_power_provider(power_provider);

        // Almacén de PSK: ventana de flash interna (sector 7 del VET6, excluido
        // del linker en memory.x). Instancia única, propiedad del módulo `psk`.
        psk::install(FlashWindow::new());

        // Telemetría de faults persistente (F4.4): vive en `.uninit` y la sella
        // `telemetry_init` validando el magic. DEBE correr antes de registrar la
        // personalidad full: sus hooks `cosmos`/`ecosystem`/`scar` leen
        // `boot_count`/`total_faults`/`safe_mode`, que hacen `assume_init` sobre
        // esta región; sin sellarla, la consola se cae al primer verbo
        // informativo (lección del F769, F6.4b).
        let warm = rugus_kernel::telemetry_init();
        defmt::info!(
            "fault telemetry: {=str} boot (boot_count={=u32}, total_faults={=u32})",
            if warm { "warm" } else { "cold" },
            rugus_kernel::boot_count(),
            rugus_kernel::total_faults(),
        );

        // Ganchos de autenticación de canal (PSK + HMAC + nonce).
        AUTH_HOOKS = Some(auth::hooks());
        defmt::info!(
            "PSK store (flash sector 7): provisioned={=bool}",
            psk::provisioned()
        );

        // Personalidad full: registra la tabla `lite::Hooks` compartida para que
        // los verbos `rush` (cosmos/ecosystem/letargo/coil/scar + GPIO de placa)
        // operen sobre datos reales del kernel y el silicio F4, igual léxico que
        // el F103 lite y el F769.
        rugus_core::syscall::lite::register(rugus_personality_full::hooks(board::ops()));
    }

    // SAFETY: arranque single-thread; el LED se inicializa una sola vez aquí.
    unsafe {
        HB_LED = Some(Pin::new(Port::C, 0, PinConfig::output())); // LED verde PC0
    }

    // Consola de operador: USART1 PA9 TX / PA10 RX (header RX/TX) @ 115200 8N1.
    // SAFETY: arranque single-thread; el UART se inicializa una sola vez.
    unsafe {
        let mut uart = Usart1::new(clocks.pclk2, CONSOLE_BAUD);
        uart.enable_rx_irq();
        NVIC::unmask(Interrupt::USART1);
        CONSOLE_UART = Some(uart);
    }
    defmt::info!("rush console ready (USART1 PA9/PA10, header RX/TX @ 115200) — knock/prove");

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
        defmt::info!("scheduler: 2 tareas (consola rush + blink durmiente), starting");
        rugus_kernel::start();
    }
}

/// Supervisor: emite el banner una vez y drena la consola `rush` periódicamente.
/// Duerme entre sondeos (~30 ms) cediendo el CPU; con tickless el scheduler entra
/// en `wfi`, acumulando idle real que reporta el verbo `letargo`.
fn supervisor_task() -> ! {
    let mut sink = UartSink;
    let _ = sink.write_str(
        "\r\nRugus F407VET6 (blackpill) console (rush).\r\nCanal gateado: aut\u{e9}nticate con `knock` y `prove`.\r\n\r\n",
    );
    loop {
        while cli_poll_byte(&mut sink) {}
        rugus_kernel::cpu_sleep_ms(30);
    }
}

/// Procesa un byte del ring RX de la consola. Devuelve `true` si consumió uno
/// (puede haber más en cola, drenar de nuevo), `false` si el ring está vacío
/// (la tarea puede dormir hasta el próximo plazo sin perder bytes: la RX es por
/// IRQ a un buffer SPSC).
fn cli_poll_byte(sink: &mut UartSink) -> bool {
    let Some(b) = RX_RING.pop() else {
        return false;
    };

    // Fast-path: byte de control ENQ (0x05) → respuesta IDENTIFY inmediata.
    if b == identify::ENQ {
        identify::write_signature(sink, TIER, CHIP);
        return true;
    }

    // SAFETY: solo la tarea supervisora edita la línea y la sesión.
    unsafe {
        if b == b'\r' || b == b'\n' {
            if LINE_LEN > 0 {
                let _ = sink.write_str("\r\n");
                let line = core::str::from_utf8(&LINE[..LINE_LEN]).unwrap_or("");
                let cmd = parse(line);
                // Todo gateado: sin sesión autenticada solo pasan IDENTIFY y el
                // propio handshake (knock/prove/lock/enroll). El resto exige PSK.
                if let Some(hooks) = AUTH_HOOKS.as_ref() {
                    execute_authed(cmd, line, sink, &mut SESSION, hooks);
                }
                LINE_LEN = 0;
            }
        } else if b == 0x7F || b == 0x08 {
            if LINE_LEN > 0 {
                LINE_LEN -= 1;
                let _ = sink.write_str("\x08 \x08");
            }
        } else if LINE_LEN < LINE.len() {
            LINE[LINE_LEN] = b;
            LINE_LEN += 1;
            let ch = [b];
            if let Ok(s) = core::str::from_utf8(&ch) {
                let _ = sink.write_str(s);
            }
        }
    }
    true
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
