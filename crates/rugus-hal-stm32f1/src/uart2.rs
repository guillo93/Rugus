//! USART2 — PA2 TX, PA3 RX (bus de módulos LoRa/BLE).
//!
//! La RX puede operar en dos modos sobre el mismo periférico:
//!
//! - **Polled** (arranque): durante el init AT del HM-20 (`hm20::init_with_kick`,
//!   antes de arrancar el scheduler) se lee `SR.RXNE` directamente. En ese
//!   contexto single-thread sin contención del scheduler el sondeo es fiable.
//! - **Por interrupción** (runtime): tras el init se llama [`Usart2::enable_rx_irq`],
//!   que habilita `RXNEIE` y enruta cada byte a un ring buffer SPSC (mismo patrón
//!   que USART1). Así el descubrimiento IDENTIFY sobre el puente BLE no pierde
//!   bytes cuando la tarea CLI cooperativa tarda en sondear (p. ej. el heartbeat
//!   a mitad de una rebanada de retardo).
//!
//! [`Usart2::try_read_byte`] conmuta entre ambos modos según el flag interno, de
//! modo que tanto el driver `hm20` (boot) como `poll_identify_usart2` (runtime)
//! usan la misma llamada sin saber qué modo está activo.

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::pac;
use pac::interrupt;
use rugus_hal::SerialPort;

/// Capacidad del ring RX (potencia de 2 para el índice módulo barato).
const RX_BUF_LEN: usize = 256;
static mut RX_BUF: [u8; RX_BUF_LEN] = [0; RX_BUF_LEN];
/// Índice de escritura (solo ISR productor).
static RX_HEAD: AtomicUsize = AtomicUsize::new(0);
/// Índice de lectura (solo tarea consumidora).
static RX_TAIL: AtomicUsize = AtomicUsize::new(0);
/// Bytes descartados por ring lleno / overrun HW (diagnóstico).
static RX_OVERRUNS: AtomicUsize = AtomicUsize::new(0);

/// Empuja un byte al ring (productor único: ISR USART2).
fn rx_push(b: u8) {
    let head = RX_HEAD.load(Ordering::Relaxed);
    let next = (head + 1) % RX_BUF_LEN;
    if next == RX_TAIL.load(Ordering::Acquire) {
        RX_OVERRUNS.fetch_add(1, Ordering::Relaxed);
        return;
    }
    // SAFETY: `head` < RX_BUF_LEN y el ISR es el único escritor de RX_BUF/RX_HEAD.
    unsafe {
        (*core::ptr::addr_of_mut!(RX_BUF))[head] = b;
    }
    RX_HEAD.store(next, Ordering::Release);
}

/// Saca un byte del ring (consumidor único: tarea CLI).
fn rx_pop() -> Option<u8> {
    let tail = RX_TAIL.load(Ordering::Relaxed);
    if tail == RX_HEAD.load(Ordering::Acquire) {
        return None;
    }
    // SAFETY: `tail` < RX_BUF_LEN y la tarea es la única lectora de RX_TAIL.
    let b = unsafe { (*core::ptr::addr_of!(RX_BUF))[tail] };
    RX_TAIL.store((tail + 1) % RX_BUF_LEN, Ordering::Release);
    Some(b)
}

/// Total de bytes RX descartados desde el arranque (ring lleno u overrun HW).
pub fn rx_overruns() -> usize {
    RX_OVERRUNS.load(Ordering::Relaxed)
}

/// ISR de USART2: drena DR a ring en cada RXNE; leer DR limpia RXNE y ORE.
#[interrupt]
fn USART2() {
    // SAFETY: handler exclusivo de USART2; solo lee SR/DR del periférico.
    let usart = unsafe { &*pac::USART2::ptr() };
    let sr = usart.sr.read();
    if sr.ore().bit() {
        RX_OVERRUNS.fetch_add(1, Ordering::Relaxed);
    }
    if sr.rxne().bit() || sr.ore().bit() {
        let b = usart.dr.read().dr().bits() as u8;
        rx_push(b);
    }
}

/// Baud inicial del bus de módulos = baud de fábrica del HM-20 (9600).
///
/// El driver [`crate::hm20`] re-sincroniza el USART2 al baud real del módulo
/// durante el sondeo, así que este valor solo fija el punto de partida. 9600
/// desde el HSI de 8 MHz del F103 tiene error <0.1 %, robusto sin depender de
/// HSE/PLL.
pub const MODULE_BAUD: u32 = 9600;

/// Error UART módulos.
pub type UartError = core::convert::Infallible;

/// Handle bloqueante USART2 en PA2/PA3.
pub struct Usart2 {
    usart: pac::USART2,
    pclk1: u32,
    /// `true` tras [`enable_rx_irq`](Usart2::enable_rx_irq): la RX llega por ISR
    /// al ring; `false` durante el boot (lectura polled de `SR.RXNE`).
    irq_rx: bool,
}

impl Usart2 {
    /// Inicializa USART2: PA2 TX, PA3 RX, 8N1 @ `baud`.
    pub fn new(rcc: &pac::RCC, usart: pac::USART2, pclk1: u32, baud: u32) -> Self {
        rcc.apb2enr.modify(|_, w| w.iopaen().set_bit());
        rcc.apb1enr.modify(|_, w| w.usart2en().set_bit());
        let _ = rcc.apb2enr.read().bits();
        let _ = rcc.apb1enr.read().bits();

        // PA2/PA3 in CRL: pin2 bits 8-11, pin3 bits 12-15.
        const TX: u32 = 0b1011; // AF push-pull
        const RX: u32 = 0b0100; // floating in
                                // SAFETY: solo CRL PA2/PA3.
        unsafe {
            let g = &*pac::GPIOA::ptr();
            g.crl.modify(|r, w| {
                let mut v = r.bits();
                v = (v & !(0xF << 8)) | (TX << 8);
                v = (v & !(0xF << 12)) | (RX << 12);
                w.bits(v)
            });
        }

        configure_usart(&usart, pclk1, baud);
        Self {
            usart,
            pclk1,
            irq_rx: false,
        }
    }

    /// Conmuta la RX a modo interrupción: habilita `RXNEIE`, desenmascara el
    /// vector NVIC y enruta los bytes al ring SPSC. Llamar UNA vez tras el init
    /// AT del HM-20 (que usa lecturas polled). Drena primero los restos del
    /// shift register y el ring para no arrastrar bytes del sondeo de arranque.
    pub fn enable_rx_irq(&mut self) {
        // Drena cualquier byte polled pendiente del init AT.
        while self.usart.sr.read().rxne().bit() {
            let _ = self.usart.dr.read().dr().bits();
        }
        RX_HEAD.store(0, Ordering::Release);
        RX_TAIL.store(0, Ordering::Release);
        self.irq_rx = true;
        self.usart.cr1.modify(|_, w| w.rxneie().set_bit());
        // SAFETY: única habilitación del vector USART2; el handler está definido.
        unsafe {
            cortex_m::peripheral::NVIC::unmask(pac::Interrupt::USART2);
        }
    }

    /// Reconfigura el baud rate del bus (p. ej. tras `AT+BAUD` en HM-20).
    /// Reasevera `RXNEIE` si la RX por interrupción ya estaba activa, porque
    /// `configure_usart` reescribe CR1 por completo.
    pub fn set_baud(&mut self, baud: u32) {
        configure_usart(&self.usart, self.pclk1, baud);
        if self.irq_rx {
            self.usart.cr1.modify(|_, w| w.rxneie().set_bit());
        }
    }

    /// Lee un byte sin bloquear. En modo interrupción saca del ring; en modo
    /// polled (boot) lee `SR.RXNE` directo.
    pub fn try_read_byte(&mut self) -> Option<u8> {
        if self.irq_rx {
            rx_pop()
        } else if self.usart.sr.read().rxne().bit() {
            Some(self.usart.dr.read().dr().bits() as u8)
        } else {
            None
        }
    }

    /// Escribe un byte (polling TXE).
    pub fn write_byte(&mut self, b: u8) {
        while !self.usart.sr.read().txe().bit() {}
        self.usart.dr.write(|w| w.dr().bits(u16::from(b)));
    }

    /// Escribe AT probe y retorna true si hay eco RX (módulo presente).
    pub fn probe_module(&mut self) -> bool {
        let _ = self.write(b"AT");
        cortex_m::asm::delay(800_000);
        let mut buf = [0u8; 4];
        let mut got = 0;
        for _ in 0..1000 {
            if self.usart.sr.read().rxne().bit() {
                buf[got] = self.usart.dr.read().dr().bits() as u8;
                got += 1;
                if got >= 2 {
                    return buf[0] == b'A' || buf[0] == b'O' || buf[1] == b'K';
                }
            }
        }
        false
    }
}

impl SerialPort for Usart2 {
    type Error = UartError;

    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        for &b in buf {
            while !self.usart.sr.read().txe().bit() {}
            self.usart.dr.write(|w| w.dr().bits(u16::from(b)));
        }
        Ok(buf.len())
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        while !self.usart.sr.read().rxne().bit() {}
        buf[0] = self.usart.dr.read().dr().bits() as u8;
        Ok(1)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        while !self.usart.sr.read().tc().bit() {}
        Ok(())
    }
}

fn configure_usart(usart: &pac::USART2, pclk: u32, baud: u32) {
    usart.cr1.write(|w| w.ue().clear_bit());
    let div = (pclk + baud / 2) / baud;
    usart
        .brr
        .write(|w| unsafe { w.bits((div / 16) << 4 | (div % 16)) });
    usart.cr1.write(|w| {
        w.te().set_bit();
        w.re().set_bit();
        w.ue().set_bit()
    });
}
