//! Rugus hello-rpi3 — primer bring-up bare-metal en Raspberry Pi 3B+ (G5 paso 0).
//!
//! Segunda arquitectura de la flota: **AArch64** (Cortex-A53, BCM2837B0), frente
//! a los Cortex-M (ARMv7-M) de las STM32. Este ejemplo valida la cadena completa
//! sin depurador: toolchain `aarch64-unknown-none` → boot por la GPU desde la SD
//! → CPU ejecutando → salida por el **mini-UART** (GPIO14/15).
//!
//! No usa todavía `rugus-arch-aarch64` ni el kernel: es el equivalente al "blink"
//! de las STM32 (G0), el suelo sobre el que crecerá G5 (MMU, EL1/EL0, GIC,
//! scheduler).
//!
//! ## Arranque
//!
//! La GPU carga `kernel8.img` en `0x80000` y salta ahí con los 4 cores activos.
//! `_start` aparca los cores 1–3 (`wfe`) y deja correr solo el 0, que monta la
//! pila, limpia `.bss` y entra en [`kernel_main`]. Corremos en el EL de arranque
//! (EL2 en la 3B+); bajar a EL1 llega con G5.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};

// Vector de arranque: solo el core 0 sigue; los demás duermen. Define la pila y
// limpia .bss antes de saltar a Rust. `kernel8.img` se carga en 0x80000.
global_asm!(
    r#"
.section ".text.boot"
.global _start
_start:
    mrs     x0, mpidr_el1        // ¿qué core soy?
    and     x0, x0, #0xFF
    cbz     x0, 2f               // core 0 → continúa
1:  wfe                          // cores 1-3 → duermen para siempre
    b       1b
2:  ldr     x0, =_stack_top
    mov     sp, x0
    ldr     x0, =__bss_start     // limpia .bss (x0..x1) a cero
    ldr     x1, =__bss_end
3:  cmp     x0, x1
    b.ge    4f
    str     xzr, [x0], #8
    b       3b
4:  bl      kernel_main          // no retorna
5:  wfe
    b       5b
"#
);

/// Base de periféricos en la BCM2837 (RPi 2/3). En la RPi 1 era `0x2000_0000`.
const MMIO_BASE: usize = 0x3F00_0000;

// GPIO.
const GPFSEL1: usize = MMIO_BASE + 0x0020_0004;
const GPPUD: usize = MMIO_BASE + 0x0020_0094;
const GPPUDCLK0: usize = MMIO_BASE + 0x0020_0098;

// Mini-UART (AUX / UART1).
const AUX_ENABLES: usize = MMIO_BASE + 0x0021_5004;
const AUX_MU_IO: usize = MMIO_BASE + 0x0021_5040;
const AUX_MU_IER: usize = MMIO_BASE + 0x0021_5044;
const AUX_MU_LCR: usize = MMIO_BASE + 0x0021_504C;
const AUX_MU_MCR: usize = MMIO_BASE + 0x0021_5050;
const AUX_MU_LSR: usize = MMIO_BASE + 0x0021_5054;
const AUX_MU_CNTL: usize = MMIO_BASE + 0x0021_5060;
const AUX_MU_BAUD: usize = MMIO_BASE + 0x0021_5068;

/// `LSR` bit 5: el transmisor puede aceptar otro byte.
const LSR_TX_EMPTY: u32 = 1 << 5;

#[inline]
fn mmio_write(addr: usize, val: u32) {
    // SAFETY: registros MMIO válidos de la BCM2837; acceso de arranque.
    unsafe { write_volatile(addr as *mut u32, val) }
}

#[inline]
fn mmio_read(addr: usize) -> u32 {
    // SAFETY: registros MMIO válidos de la BCM2837.
    unsafe { read_volatile(addr as *const u32) }
}

/// Espera ocupada corta (para los flancos de GPPUD).
fn delay(cycles: u32) {
    for _ in 0..cycles {
        core::hint::spin_loop();
    }
}

/// Inicializa el mini-UART en GPIO14 (TXD1) / GPIO15 (RXD1) a 115200 8N1.
///
/// El baudrate del mini-UART cuelga del reloj del VPU (`core_freq`); con
/// `core_freq=250` (fijado en `config.txt`) el divisor es `250e6/(8*115200)-1`.
fn uart_init() {
    mmio_write(AUX_ENABLES, mmio_read(AUX_ENABLES) | 1); // habilita mini-UART
    mmio_write(AUX_MU_CNTL, 0); // TX/RX off mientras configuramos
    mmio_write(AUX_MU_IER, 0); // sin interrupciones
    mmio_write(AUX_MU_LCR, 3); // modo 8 bits (bit0+bit1; erratum del datasheet)
    mmio_write(AUX_MU_MCR, 0); // RTS fijo
    mmio_write(AUX_MU_BAUD, 270); // 115200 @ core_freq=250 MHz

    // GPIO14/15 → función alternativa ALT5 (TXD1/RXD1).
    let mut sel = mmio_read(GPFSEL1);
    sel &= !((0b111 << 12) | (0b111 << 15)); // limpia FSEL14/15
    sel |= (0b010 << 12) | (0b010 << 15); // ALT5
    mmio_write(GPFSEL1, sel);

    // Desactiva pull-up/down en GPIO14/15 (secuencia GPPUD de la BCM2837).
    mmio_write(GPPUD, 0);
    delay(150);
    mmio_write(GPPUDCLK0, (1 << 14) | (1 << 15));
    delay(150);
    mmio_write(GPPUDCLK0, 0);

    mmio_write(AUX_MU_CNTL, 3); // TX + RX on
}

/// Transmite un byte (polling de `LSR.TX_EMPTY`).
fn uart_send(b: u8) {
    while mmio_read(AUX_MU_LSR) & LSR_TX_EMPTY == 0 {}
    mmio_write(AUX_MU_IO, b as u32);
}

/// Escribe una cadena; `\n` se expande a `\r\n` para terminales serie.
fn uart_puts(s: &str) {
    for &b in s.as_bytes() {
        if b == b'\n' {
            uart_send(b'\r');
        }
        uart_send(b);
    }
}

#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    uart_init();
    uart_puts("\n=== RUGUS @ Raspberry Pi 3B+ (AArch64 / Cortex-A53) ===\n");
    uart_puts("hello-rpi3: boot OK, mini-UART vivo. Segunda arquitectura en pie.\n");

    // Latido por UART para confirmar que el core sigue corriendo.
    let mut tick: u32 = 0;
    loop {
        uart_puts("rugus rpi3 tick ");
        // Imprime `tick` en decimal sin alloc.
        let mut buf = [0u8; 10];
        let mut n = tick;
        let mut i = buf.len();
        loop {
            i -= 1;
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        uart_puts(core::str::from_utf8(&buf[i..]).unwrap_or("?"));
        uart_puts("\n");
        tick = tick.wrapping_add(1);
        delay(50_000_000); // ~1 s aproximado (sin reloj calibrado aún)
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
