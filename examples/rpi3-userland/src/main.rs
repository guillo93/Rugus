//! Rugus G6.1 — **aislamiento EL0/MMU** en RPi 3B+ (AArch64), prueba aislada.
//!
//! Primer paso hacia `HAS_MEMORY_PROTECTION` en el backend AArch64: demuestra
//! los primitivos de protección de memoria de ARMv8-A antes de integrarlos en el
//! `Scheduler<CortexA>` (misma metodología que `rpi3-sched`→`rpi3-kernel`).
//!
//! Qué prueba, todo por el mini-UART:
//! 1. **EL0**: el kernel (EL1) baja a EL0 (`eret` con `SPSR.M=EL0t`) y ejecuta
//!    una rutina userland.
//! 2. **Syscall (`SVC`)**: la rutina EL0 hace `svc #0` → trampa síncrona a EL1,
//!    que la atiende (imprime un carácter) y vuelve a EL0 con `eret`.
//! 3. **Aislamiento (MMU)**: la rutina EL0 intenta LEER memoria del kernel
//!    (0x80000, marcada `AP=EL1-only`) → *data abort* desde EL inferior, que EL1
//!    **contiene** e informa (no se cuelga ni corrompe nada).
//!
//! El mecanismo: una sola tabla de traducción identidad, con los **bits AP** del
//! descriptor de bloque separando privilegio — bloque del kernel `AP=00`
//! (EL1 RW, EL0 sin acceso) y un bloque userland (2 MiB en 0x200000) `AP=01`
//! (EL0+EL1 RW, ejecutable en EL0). El despacho de excepciones lee `ESR_EL1.EC`
//! para distinguir `SVC` (0x15) de *abort* (0x24).

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

// ===================== Boot: EL2 → EL1 + VBAR + FP/SIMD + bss =====================
global_asm!(
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
    adr     x0, vector_table
    msr     vbar_el1, x0
    mov     x0, #(3 << 20)
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
"#
);

// ===================== Tabla de vectores + handlers de excepción =====================
global_asm!(
    r#"
.align 11
.global vector_table
vector_table:
    // Current EL con SP_EL0 (no usado).
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    // Current EL con SP_ELx (EL1h): faults del kernel.
    .align 7
    b       el1_sync
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    // Lower EL AArch64 (EL0): aquí entran SVC y los aborts de userland.
    .align 7
    b       el0_sync
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    // Lower EL AArch32: no soportado.
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc

// Síncrona desde EL0: despacha por ESR.EC — SVC (0x15) o abort (resto).
el0_sync:
    sub     sp, sp, #64
    stp     x0,  x1,  [sp, #0]
    stp     x29, x30, [sp, #16]
    mrs     x9,  esr_el1
    mrs     x10, elr_el1
    stp     x9,  x10, [sp, #32]
    mrs     x11, spsr_el1
    str     x11, [sp, #48]
    lsr     x9,  x9, #26             // EC = ESR[31:26]
    cmp     x9,  #0x15               // SVC64
    b.ne    el0_fault
    // --- syscall: el argumento (carácter) está en el x0 guardado ---
    ldr     x0, [sp, #0]
    bl      rust_svc
    // restaura el contexto mínimo y vuelve a EL0
    ldp     x9,  x10, [sp, #32]
    msr     elr_el1,  x10
    ldr     x11, [sp, #48]
    msr     spsr_el1, x11
    ldp     x0,  x1,  [sp, #0]
    ldp     x29, x30, [sp, #16]
    add     sp,  sp, #64
    eret
el0_fault:
    // --- abort de EL0: lo contiene EL1, informa y para ---
    mrs     x0, esr_el1
    mrs     x1, far_el1
    bl      rust_abort
1:  wfe
    b       1b

el1_sync:
    mrs     x0, esr_el1
    mrs     x1, elr_el1
    bl      rust_kernel_fault
1:  wfe
    b       1b

hang_exc:
    bl      rust_unexpected
1:  wfe
    b       1b

// Rutina userland (se COPIA al bloque EL0 en 0x200000 y se ejecuta en EL0).
// Position-independent: sin referencias absolutas salvo la dirección del kernel
// que intentamos leer a propósito (0x80000) para provocar el fault de aislamiento.
.global user_stub_start
user_stub_start:
    mov     w0, #0x55                // 'U'
    svc     #0                       // syscall: pide imprimir el carácter
    movz    x1, #0x8, lsl #16        // x1 = 0x80000 (imagen del kernel, EL1-only)
    ldr     x2, [x1]                 // LECTURA prohibida desde EL0 → data abort
    b       .                        // (no se alcanza)
.global user_stub_end
user_stub_end:
"#
);

extern "C" {
    static user_stub_start: u8;
    static user_stub_end: u8;
}

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
fn uart_hex(mut v: u64) {
    const H: &[u8; 16] = b"0123456789abcdef";
    uart_puts("0x");
    let mut started = false;
    for i in (0..16).rev() {
        let nib = ((v >> (i * 4)) & 0xF) as usize;
        if nib != 0 || started || i == 0 {
            uart_send(H[nib]);
            started = true;
        }
    }
    let _ = &mut v;
}

// ===================== MMU con bits AP (kernel EL1-only / user EL0) =====================
#[repr(C, align(4096))]
struct PageTable([u64; 512]);
static mut L1: PageTable = PageTable([0; 512]);
static mut L2: PageTable = PageTable([0; 512]);

const PT_BLOCK: u64 = 0b01;
const PT_TABLE: u64 = 0b11;
const PT_AF: u64 = 1 << 10;
const PT_SH_INNER: u64 = 0b11 << 8;
const ATTR_NORMAL: u64 = 1 << 2;
const ATTR_DEVICE: u64 = 0 << 2;
const AP_EL0_RW: u64 = 0b01 << 6; // EL1 RW + EL0 RW
const PT_UXN: u64 = 1 << 54; // EL0 execute-never

/// Índice de bloque L2 (cada entrada = 2 MiB) reservado a userland: 0x200000.
const USER_BLOCK_IDX: usize = 1;
/// Base física del bloque userland y tope de su pila EL0.
const USER_BASE: u64 = (USER_BLOCK_IDX as u64) << 21; // 0x200000
const USER_STACK_TOP: u64 = USER_BASE + 0x10_0000; // 0x300000 (dentro del bloque)

unsafe fn mmu_init() {
    unsafe {
        let l2 = &mut (*addr_of_mut!(L2)).0;
        for (i, e) in l2.iter_mut().enumerate() {
            let pa = (i as u64) << 21;
            *e = if pa >= 0x3F00_0000 {
                pa | PT_BLOCK | PT_AF | ATTR_DEVICE // periféricos (EL1-only)
            } else if i == USER_BLOCK_IDX {
                // Bloque userland: EL0+EL1 RW, ejecutable en EL0 (UXN=0).
                pa | PT_BLOCK | PT_AF | PT_SH_INNER | ATTR_NORMAL | AP_EL0_RW
            } else {
                // RAM del kernel: EL1 RW, EL0 SIN ACCESO (AP=00), no ejecutable EL0.
                pa | PT_BLOCK | PT_AF | PT_SH_INNER | ATTR_NORMAL | PT_UXN
            };
        }
        let l1 = &mut (*addr_of_mut!(L1)).0;
        l1[0] = (addr_of!(L2) as u64) | PT_TABLE;
        l1[1] = 0x4000_0000 | PT_BLOCK | PT_AF | ATTR_DEVICE;
        let mair: u64 = 0xFF << 8; // attr0=Device, attr1=Normal WB
        core::arch::asm!("msr mair_el1, {}", in(reg) mair);
        let m: u64;
        core::arch::asm!("mrs {}, id_aa64mmfr0_el1", out(reg) m);
        let tcr: u64 = 25 | (0b01 << 8) | (0b01 << 10) | (0b11 << 12) | ((m & 0xF) << 32);
        core::arch::asm!("msr tcr_el1, {}", in(reg) tcr);
        core::arch::asm!("msr ttbr0_el1, {}", in(reg) addr_of!(L1) as u64);
        core::arch::asm!("dsb ish; isb");
        let mut sctlr: u64;
        core::arch::asm!("mrs {}, sctlr_el1", out(reg) sctlr);
        sctlr |= (1 << 0) | (1 << 2) | (1 << 12);
        core::arch::asm!("msr sctlr_el1, {}; isb", in(reg) sctlr);
    }
}

// ===================== Handlers en Rust =====================
#[no_mangle]
extern "C" fn rust_svc(arg: u64) {
    uart_puts("[EL0->EL1] syscall recibido: putchar '");
    uart_send((arg & 0xFF) as u8);
    uart_puts("'\n");
}

#[no_mangle]
extern "C" fn rust_abort(esr: u64, far: u64) {
    let ec = (esr >> 26) & 0x3F;
    uart_puts("\n[EL1] *** abort de EL0 CONTENIDO (aislamiento OK) ***\n");
    uart_puts("  EC=");
    uart_hex(ec);
    uart_puts(" (0x24 = data abort, lower EL)\n  ESR=");
    uart_hex(esr);
    uart_puts("\n  FAR=");
    uart_hex(far);
    uart_puts("  <- direccion del kernel que EL0 intento leer\n");
    uart_puts("\n[OK] EL0 aislado: el acceso prohibido fallo sin tumbar el kernel.\n");
}

#[no_mangle]
extern "C" fn rust_kernel_fault(esr: u64, elr: u64) {
    uart_puts("\n!! FAULT de KERNEL (EL1) ESR=");
    uart_hex(esr);
    uart_puts(" ELR=");
    uart_hex(elr);
    uart_puts("\n");
}

#[no_mangle]
extern "C" fn rust_unexpected() {
    uart_puts("\n!! excepcion inesperada\n");
}

// ===================== Entrada a EL0 =====================
/// Baja a EL0: fija `SP_EL0`, `ELR_EL1`=entrada userland, `SPSR_EL1`=EL0t y
/// `eret`. No retorna: a partir de aquí EL0 corre y vuelve a EL1 por SVC/abort.
unsafe fn enter_el0(entry: u64, stack_top: u64) -> ! {
    unsafe {
        core::arch::asm!(
            "msr sp_el0, {sp}",
            "msr elr_el1, {pc}",
            "msr spsr_el1, xzr",     // M=EL0t, DAIF=0
            "isb",
            "eret",
            sp = in(reg) stack_top,
            pc = in(reg) entry,
            options(noreturn),
        );
    }
}

#[no_mangle]
extern "C" fn kernel_main() -> ! {
    uart_init();
    uart_puts("\n=== RUGUS @ RPi 3B+ — G6.1: aislamiento EL0/MMU (AArch64) ===\n");
    uart_puts("[1] EL1 + FP/SIMD + VBAR ok\n");
    uart_puts("[2] MMU con bits AP (kernel EL1-only / user EL0 en 0x200000)...\n");
    unsafe { mmu_init() };
    uart_puts("[2] MMU ON\n");

    // Copia la rutina userland al bloque EL0 (0x200000).
    let s = addr_of!(user_stub_start);
    let e = addr_of!(user_stub_end);
    let (src, len) = (s, e as usize - s as usize);
    uart_puts("[3] copiando rutina userland a 0x200000 (");
    uart_hex(len as u64);
    uart_puts(" bytes)...\n");
    // SAFETY: destino en el bloque userland (RAM libre, EL0+EL1 RW), `len` bytes.
    unsafe {
        let dst = USER_BASE as *mut u8;
        for i in 0..len {
            write_volatile(dst.add(i), read_volatile(src.add(i)));
        }
        core::arch::asm!("dsb ish; ic iallu; dsb ish; isb"); // coherencia I-cache
    }

    uart_puts("[4] eret a EL0 (la rutina hara: syscall 'U' y luego LEER 0x80000)\n\n");
    // SAFETY: entrada y pila dentro del bloque userland mapeado EL0.
    unsafe { enter_el0(USER_BASE, USER_STACK_TOP) }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    uart_puts("\n!! PANIC\n");
    loop {
        core::hint::spin_loop();
    }
}
