//! Rugus — **lectura de OTP del BCM2837 vía el mailbox del VideoCore**.
//!
//! Base de la PSK persistente en la RPi: la OTP (one-time-programmable) son
//! fusibles que sobreviven a reinicios y reflasheos. Aquí, de forma **solo
//! lectura** (no quema nada), se valida el protocolo del **mailbox del
//! VideoCore** leyendo:
//! - el **serial de placa** (tag 0x00010004) — valor conocido, test no destructivo;
//! - las **8 filas customer de OTP** (tag 0x00030021) — donde iría la PSK.
//!
//! Sin MMU a propósito: con MMU/caché desactivadas (estado de arranque), el
//! buffer del mailbox es no-cacheable → coherente con el VideoCore sin
//! mantenimiento de caché. Al integrarlo en la consola (MMU on) habrá que
//! limpiar/invalidar el buffer alrededor de la llamada.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};

core::arch::global_asm!(
    r#"
.section ".text.boot"
.global _start
_start:
    mrs     x0, mpidr_el1
    and     x0, x0, #0xFF
    cbnz    x0, halt
    mrs     x0, CurrentEL
    lsr     x0, x0, #2
    cmp     x0, #2
    b.ne    in_el1
    mrs     x0, cnthctl_el2
    orr     x0, x0, #3
    msr     cnthctl_el2, x0
    msr     cntvoff_el2, xzr
    mov     x0, #(1 << 31)
    msr     hcr_el2, x0
    mov     x0, #0x0800
    movk    x0, #0x30d0, lsl #16
    msr     sctlr_el1, x0
    mov     x0, #0x3c5
    msr     spsr_el2, x0
    adr     x0, in_el1
    msr     elr_el2, x0
    eret
in_el1:
    ldr     x0, =_stack_top
    mov     sp, x0
    adr     x0, early_vectors
    msr     vbar_el1, x0
    mov     x0, #(3 << 20)          // CPACR_EL1.FPEN: habilita FP/SIMD
    msr     cpacr_el1, x0
    isb
    ldr     x0, =__bss_start
    ldr     x1, =__bss_end
1:  cmp     x0, x1
    b.ge    2f
    str     xzr, [x0], #8
    b       1b
2:  bl      kernel_main
halt:
    wfe
    b       halt

.align 11
.global early_vectors
early_vectors:
    .rept 16
    .align 7
    b       early_hang
    .endr
early_hang:
    wfe
    b       early_hang
"#
);

// ===================== mini-UART =====================
const MMIO_BASE: usize = 0x3F00_0000;
const GPFSEL1: usize = MMIO_BASE + 0x0020_0004;
const GPPUD: usize = MMIO_BASE + 0x0020_0094;
const GPPUDCLK0: usize = MMIO_BASE + 0x0020_0098;
const AUX_ENABLES: usize = MMIO_BASE + 0x0021_5004;
const AUX_MU_IO: usize = MMIO_BASE + 0x0021_5040;
const AUX_MU_IER: usize = MMIO_BASE + 0x0021_5044;
const AUX_MU_LCR: usize = MMIO_BASE + 0x0021_504C;
const AUX_MU_MCR: usize = MMIO_BASE + 0x0021_5050;
const AUX_MU_LSR: usize = MMIO_BASE + 0x0021_5054;
const AUX_MU_CNTL: usize = MMIO_BASE + 0x0021_5060;
const AUX_MU_BAUD: usize = MMIO_BASE + 0x0021_5068;
const LSR_TX_EMPTY: u32 = 1 << 5;

#[inline]
fn mw(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}
#[inline]
fn mr(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}
fn delay(n: u32) {
    for _ in 0..n {
        core::hint::spin_loop();
    }
}
fn uart_init() {
    mw(AUX_ENABLES, mr(AUX_ENABLES) | 1);
    mw(AUX_MU_CNTL, 0);
    mw(AUX_MU_IER, 0);
    mw(AUX_MU_LCR, 3);
    mw(AUX_MU_MCR, 0);
    mw(AUX_MU_BAUD, 270);
    let mut sel = mr(GPFSEL1);
    sel &= !((0b111 << 12) | (0b111 << 15));
    sel |= (0b010 << 12) | (0b010 << 15);
    mw(GPFSEL1, sel);
    mw(GPPUD, 0);
    delay(150);
    mw(GPPUDCLK0, (1 << 14) | (1 << 15));
    delay(150);
    mw(GPPUDCLK0, 0);
    mw(AUX_MU_CNTL, 3);
}
fn uart_send(b: u8) {
    while mr(AUX_MU_LSR) & LSR_TX_EMPTY == 0 {}
    mw(AUX_MU_IO, b as u32);
}
fn uart_puts(s: &str) {
    for &b in s.as_bytes() {
        if b == b'\n' {
            uart_send(b'\r');
        }
        uart_send(b);
    }
}
fn uart_hex32(v: u32) {
    const H: &[u8; 16] = b"0123456789abcdef";
    for i in (0..8).rev() {
        uart_send(H[((v >> (i * 4)) & 0xF) as usize]);
    }
}

// ===================== Mailbox del VideoCore (canal 8, property tags) =====================
const VC_MBOX: usize = MMIO_BASE + 0x0000_B880;
const MBOX_READ: usize = VC_MBOX;
const MBOX_STATUS: usize = VC_MBOX + 0x18;
const MBOX_WRITE: usize = VC_MBOX + 0x20;
const MBOX_FULL: u32 = 0x8000_0000;
const MBOX_EMPTY: u32 = 0x4000_0000;
const MBOX_CH_PROP: u32 = 8;
const MBOX_RESP_OK: u32 = 0x8000_0000;

/// Buffer del mailbox, alineado a 16 (los 4 bits bajos llevan el canal).
#[repr(C, align(16))]
struct MboxBuf([u32; 36]);
static mut MBOX: MboxBuf = MboxBuf([0; 36]);

/// Realiza una llamada al mailbox con el buffer ya rellenado. Devuelve `true`
/// si el VideoCore respondió OK. (Sin MMU/caché → buffer coherente.)
fn mailbox_call() -> bool {
    let addr = core::ptr::addr_of!(MBOX) as u32;
    let msg = (addr & !0xF) | MBOX_CH_PROP;
    // SAFETY: registros MMIO del mailbox; sincronización por los flags FULL/EMPTY.
    unsafe {
        core::arch::asm!("dsb sy");
        while mr(MBOX_STATUS) & MBOX_FULL != 0 {}
        mw(MBOX_WRITE, msg);
        loop {
            while mr(MBOX_STATUS) & MBOX_EMPTY != 0 {}
            if mr(MBOX_READ) == msg {
                break;
            }
        }
        core::arch::asm!("dsb sy");
        read_volatile(core::ptr::addr_of!(MBOX.0[1])) == MBOX_RESP_OK
    }
}

/// Lee el serial de 64 bits de la placa (tag 0x00010004). Test no destructivo.
fn read_board_serial() -> (u32, u32) {
    // SAFETY: arranque single-thread; buffer estático.
    unsafe {
        let b = core::ptr::addr_of_mut!(MBOX.0) as *mut u32;
        write_volatile(b.add(0), 8 * 4); // tamaño total
        write_volatile(b.add(1), 0); // request
        write_volatile(b.add(2), 0x0001_0004); // GET_BOARD_SERIAL
        write_volatile(b.add(3), 8); // tamaño del buffer de valor
        write_volatile(b.add(4), 0); // req/resp
        write_volatile(b.add(5), 0); // serial lo
        write_volatile(b.add(6), 0); // serial hi
        write_volatile(b.add(7), 0); // end tag
        let ok = mailbox_call();
        if ok {
            (read_volatile(b.add(5)), read_volatile(b.add(6)))
        } else {
            (0, 0)
        }
    }
}

/// Lee `n` filas customer de OTP (tag 0x00030021), a partir de la fila 0.
/// Devuelve cuántas filas leyó y las deja en `out`.
fn read_customer_otp(out: &mut [u32; 8]) -> usize {
    // SAFETY: arranque single-thread; buffer estático.
    unsafe {
        let b = core::ptr::addr_of_mut!(MBOX.0) as *mut u32;
        let n = 8u32;
        let val_size = (2 + n) * 4; // row_offset + num_rows + n words
        write_volatile(b.add(0), (5 + 2 + n + 1) * 4); // tamaño total
        write_volatile(b.add(1), 0);
        write_volatile(b.add(2), 0x0003_0021); // GET_CUSTOMER_OTP
        write_volatile(b.add(3), val_size);
        write_volatile(b.add(4), 0);
        write_volatile(b.add(5), 0); // row offset
        write_volatile(b.add(6), n); // num rows
        for i in 0..n {
            write_volatile(b.add(7 + i as usize), 0);
        }
        write_volatile(b.add(7 + n as usize), 0); // end tag
        if !mailbox_call() {
            return 0;
        }
        // Respuesta: [5]=row_offset, [6]=num_rows, [7..]=valores.
        let got = read_volatile(b.add(6)).min(8);
        for i in 0..got {
            out[i as usize] = read_volatile(b.add(7 + i as usize));
        }
        got as usize
    }
}

#[no_mangle]
extern "C" fn kernel_main() -> ! {
    uart_init();
    // Imprime el informe en BUCLE (con pausa) para que cualquier captura por
    // serie lo pille, independientemente del momento del reset.
    loop {
    uart_puts("\n=== RUGUS @ RPi 3B+ — lectura de OTP por mailbox del VideoCore ===\n");

    let (lo, hi) = read_board_serial();
    uart_puts("[serial de placa] 0x");
    uart_hex32(hi);
    uart_hex32(lo);
    uart_puts("  (test no destructivo del mailbox)\n");

    let mut otp = [0u32; 8];
    let n = read_customer_otp(&mut otp);
    uart_puts("[customer OTP] ");
    if n == 0 {
        uart_puts("lectura fallida\n");
    } else {
        uart_puts("filas leidas, valores:\n");
        let mut all_zero = true;
        for (i, &w) in otp.iter().enumerate().take(n) {
            uart_puts("  fila ");
            uart_send(b'0' + i as u8);
            uart_puts(" = 0x");
            uart_hex32(w);
            uart_puts("\n");
            if w != 0 {
                all_zero = false;
            }
        }
        uart_puts(if all_zero {
            "\n[estado] OTP customer VACIA -> la placa NO esta aprovisionada (fusibles intactos).\n"
        } else {
            "\n[estado] OTP customer con datos -> la placa esta aprovisionada.\n"
        });
    }
    uart_puts("[OK] mecanismo de PSK persistente por OTP (solo lectura) validado.\n");
        delay(60_000_000);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
