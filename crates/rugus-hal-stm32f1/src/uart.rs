//! USART1 — PA9 TX, PA10 RX @ 115200 (consola de la shell `rush`).
//!
//! La recepción es por interrupción (RXNE) hacia un ring buffer SPSC: el ISR
//! `USART1` es el único productor y la tarea CLI el único consumidor. El RX
//! polled de 1 byte sin FIFO perdía bytes en ráfaga (a 115200 un byte llega
//! cada ~87 µs y el scheduler cooperativo podía no sondear a tiempo); el ring
//! desacopla la llegada del consumo y absorbe la ráfaga.

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::pac;
use pac::interrupt;
use rugus_hal::SerialPort;

/// Baud rate por defecto de la consola `rush`.
pub const CLI_BAUD: u32 = 115_200;

/// Capacidad del ring RX (potencia de 2 para el índice módulo barato).
const RX_BUF_LEN: usize = 256;
static mut RX_BUF: [u8; RX_BUF_LEN] = [0; RX_BUF_LEN];
/// Índice de escritura (solo ISR productor).
static RX_HEAD: AtomicUsize = AtomicUsize::new(0);
/// Índice de lectura (solo tarea consumidora).
static RX_TAIL: AtomicUsize = AtomicUsize::new(0);
/// Bytes descartados por ring lleno / overrun HW (diagnóstico).
static RX_OVERRUNS: AtomicUsize = AtomicUsize::new(0);

/// Empuja un byte al ring (productor único: ISR USART1).
fn rx_push(b: u8) {
    let head = RX_HEAD.load(Ordering::Relaxed);
    let next = (head + 1) % RX_BUF_LEN;
    // Ring lleno: descarta el byte nuevo y cuenta el overrun (no se pisa al
    // consumidor). Preferible perder el más reciente a corromper el índice.
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

/// ISR de USART1: drena DR a ring en cada RXNE; leer DR limpia RXNE y ORE.
#[interrupt]
fn USART1() {
    // SAFETY: handler exclusivo de USART1; solo lee SR/DR del periférico.
    let usart = unsafe { &*pac::USART1::ptr() };
    let sr = usart.sr.read();
    if sr.ore().bit() {
        // Overrun HW: el dato previo se perdió en el shift register. Leer DR
        // limpia ORE; contamos el byte perdido.
        RX_OVERRUNS.fetch_add(1, Ordering::Relaxed);
    }
    if sr.rxne().bit() || sr.ore().bit() {
        let b = usart.dr.read().dr().bits() as u8;
        rx_push(b);
    }
}

/// Error de UART (infallible en operaciones bloqueantes actuales).
pub type UartError = core::convert::Infallible;

/// Handle bloqueante para USART1 en PA9/PA10.
pub struct Usart1 {
    usart: pac::USART1,
}

impl Usart1 {
    /// Inicializa USART1: PA9 TX, PA10 RX, 8N1 @ `baud` con `pclk2` Hz.
    /// Habilita RX por interrupción hacia el ring buffer SPSC.
    pub fn new(rcc: &pac::RCC, usart: pac::USART1, pclk2: u32, baud: u32) -> Self {
        enable_clocks(rcc);
        configure_pins();
        configure_usart(&usart, pclk2, baud);
        // RXNEIE: cada byte recibido genera IRQ que lo drena al ring.
        usart.cr1.modify(|_, w| w.rxneie().set_bit());
        // SAFETY: única habilitación del vector USART1; el handler está definido.
        unsafe {
            cortex_m::peripheral::NVIC::unmask(pac::Interrupt::USART1);
        }
        Self { usart }
    }

    /// Saca el siguiente byte recibido del ring; no bloquea.
    pub fn try_read_byte(&mut self) -> Option<u8> {
        rx_pop()
    }

    /// Escribe un byte (polling TXE).
    pub fn write_byte(&mut self, b: u8) {
        while !self.usart.sr.read().txe().bit() {}
        // SAFETY: TXE indica DR vacío.
        self.usart.dr.write(|w| w.dr().bits(u16::from(b)));
    }

    /// Lee un byte del ring; bloquea (spin) hasta que haya uno.
    pub fn read_byte(&mut self) -> u8 {
        loop {
            if let Some(b) = rx_pop() {
                return b;
            }
        }
    }
}

impl SerialPort for Usart1 {
    type Error = UartError;

    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        for (i, &b) in buf.iter().enumerate() {
            self.write_byte(b);
            if i + 1 == buf.len() {
                break;
            }
        }
        Ok(buf.len())
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        buf[0] = self.read_byte();
        Ok(1)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        while !self.usart.sr.read().tc().bit() {}
        Ok(())
    }
}

fn enable_clocks(rcc: &pac::RCC) {
    rcc.apb2enr.modify(|_, w| {
        w.iopaen().set_bit();
        w.usart1en().set_bit()
    });
    let _ = rcc.apb2enr.read().bits();
}

fn configure_pins() {
    // PA9: AF push-pull TX, PA10: floating input RX.
    // CRH: PA8-PA15. PA9 nibble bits 4-7, PA10 bits 8-11.
    // TX: CNF=10 (AF push-pull), MODE=11 (50 MHz) → 0b1011
    // RX: CNF=01 (floating in), MODE=00 → 0b0100
    const TX_NIBBLE: u32 = 0b1011;
    const RX_NIBBLE: u32 = 0b0100;
    // SAFETY: solo GPIOA CRH para PA9/PA10.
    unsafe {
        let g = &*pac::GPIOA::ptr();
        g.crh.modify(|r, w| {
            let mut v = r.bits();
            v = (v & !(0xF << 4)) | (TX_NIBBLE << 4);
            v = (v & !(0xF << 8)) | (RX_NIBBLE << 8);
            w.bits(v)
        });
    }
}

fn configure_usart(usart: &pac::USART1, pclk2: u32, baud: u32) {
    usart.cr1.write(|w| w.ue().clear_bit());
    let div = (pclk2 + baud / 2) / baud;
    let mantissa = div / 16;
    let fraction = div % 16;
    usart
        .brr
        .write(|w| unsafe { w.bits(mantissa << 4 | fraction) });
    usart.cr1.write(|w| {
        w.te().set_bit();
        w.re().set_bit();
        w.ue().set_bit()
    });
}
