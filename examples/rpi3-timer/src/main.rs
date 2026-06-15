//! Rugus G5 — EL1 + excepciones + MMU + Generic Timer por IRQ en RPi 3B+ (AArch64).
//!
//! Construye sobre `hello-rpi3` (boot por SD + mini-UART) las capas que necesita
//! un kernel real en ARMv8-A, en orden de dependencia:
//!
//! 1. **EL2 → EL1**: la RPi 3 arranca en EL2; bajamos a EL1h (la RPi corre el
//!    kernel en EL1, como Linux). Se programan HCR_EL2 (EL1=AArch64), el offset
//!    del timer y el `eret` a EL1.
//! 2. **Vector table (VBAR_EL1)**: 16 entradas; manejamos sync (imprime el ESR y
//!    para) e IRQ (tick del timer) del grupo "Current EL con SPx" (EL1h).
//! 3. **MMU**: mapeo identidad con tablas L1/L2 — RAM (0..0x3F00_0000) como
//!    Normal WB cacheable, periféricos como Device. Habilita caches I/D. Es
//!    prerrequisito de los atomics de ARMv8 (futuro scheduler).
//! 4. **Generic Timer (CNTP) + ARM local IRQ routing** (0x4000_0000): en la RPi3
//!    el timer NO va por GIC (deshabilitado de fábrica) sino por el enrutado de
//!    interrupciones "ARM local" al core 0. El `tick` ahora viene de una IRQ
//!    real, no de un busy-delay.
//!
//! Cada fase imprime una traza por el mini-UART: si algo cuelga, el serie dice
//! en qué capa (disciplina de bring-up sin depurador).

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use core::ptr::{read_volatile, write_volatile};

// ===================== Boot: EL2 → EL1 + VBAR + bss =====================
global_asm!(
    r#"
.section ".text.boot"
.global _start
_start:
    mrs     x0, mpidr_el1
    and     x0, x0, #0xFF
    cbnz    x0, halt                 // solo el core 0 sigue

    mrs     x0, CurrentEL
    lsr     x0, x0, #2
    cmp     x0, #2
    b.ne    in_el1                   // si ya estamos en EL1, saltar la bajada

    // --- En EL2: preparar la bajada a EL1h ---
    mrs     x0, cnthctl_el2
    orr     x0, x0, #3               // EL1PCTEN | EL1PCEN: EL1 puede usar el timer
    msr     cnthctl_el2, x0
    msr     cntvoff_el2, xzr

    mov     x0, #(1 << 31)           // HCR_EL2.RW = 1 → EL1 en AArch64
    msr     hcr_el2, x0

    mov     x0, #0x0800
    movk    x0, #0x30d0, lsl #16     // SCTLR_EL1 con bits RES1, MMU/caches OFF
    msr     sctlr_el1, x0

    mov     x0, #0x3c5               // SPSR_EL2: EL1h (M=0101), DAIF enmascarado
    msr     spsr_el2, x0
    adr     x0, in_el1
    msr     elr_el2, x0
    eret

in_el1:
    ldr     x0, =_stack_top
    mov     sp, x0
    adr     x0, vector_table         // VBAR_EL1
    msr     vbar_el1, x0

    // Habilita FP/SIMD en EL1 (CPACR_EL1.FPEN=0b11). Tras el reset el acceso a
    // NEON/FP está atrapado (ESR EC=0x7); el codegen de Rust vectoriza bucles
    // (p. ej. el llenado de las tablas MMU) con instrucciones SIMD → excepción.
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

// ===================== Vector table EL1 =====================
// 16 entradas de 0x80 bytes, tabla alineada a 2 KiB. Corremos en EL1h (SP_ELx),
// así que las excepciones caen en el grupo "Current EL with SPx" (offset 0x200).
global_asm!(
    r#"
.macro SAVE_CTX
    sub     sp, sp, #256
    stp     x0,  x1,  [sp, #16 * 0]
    stp     x2,  x3,  [sp, #16 * 1]
    stp     x4,  x5,  [sp, #16 * 2]
    stp     x6,  x7,  [sp, #16 * 3]
    stp     x8,  x9,  [sp, #16 * 4]
    stp     x10, x11, [sp, #16 * 5]
    stp     x12, x13, [sp, #16 * 6]
    stp     x14, x15, [sp, #16 * 7]
    stp     x16, x17, [sp, #16 * 8]
    stp     x18, x29, [sp, #16 * 9]
    str     x30,      [sp, #16 * 10]
.endm

.macro RESTORE_CTX
    ldp     x0,  x1,  [sp, #16 * 0]
    ldp     x2,  x3,  [sp, #16 * 1]
    ldp     x4,  x5,  [sp, #16 * 2]
    ldp     x6,  x7,  [sp, #16 * 3]
    ldp     x8,  x9,  [sp, #16 * 4]
    ldp     x10, x11, [sp, #16 * 5]
    ldp     x12, x13, [sp, #16 * 6]
    ldp     x14, x15, [sp, #16 * 7]
    ldp     x16, x17, [sp, #16 * 8]
    ldp     x18, x29, [sp, #16 * 9]
    ldr     x30,      [sp, #16 * 10]
    add     sp, sp, #256
.endm

.align 11
.global vector_table
vector_table:
    // --- Current EL with SP0 (no usado) ---
    .align 7
    b       el1_sync
    .align 7
    b       el1_irq
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    // --- Current EL with SPx (EL1h) — AQUÍ caen nuestras excepciones ---
    .align 7
el1_sync_vec:
    b       el1_sync
    .align 7
el1_irq_vec:
    b       el1_irq
    .align 7
    b       hang_exc                 // FIQ
    .align 7
    b       hang_exc                 // SError
    // --- Lower EL AArch64 (no usado) ---
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    // --- Lower EL AArch32 (no usado) ---
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc

el1_irq:
    SAVE_CTX
    bl      rust_irq_handler
    RESTORE_CTX
    eret

el1_sync:
    SAVE_CTX
    mrs     x0, esr_el1
    mrs     x1, elr_el1
    bl      rust_sync_handler        // imprime y no retorna
1:  wfe
    b       1b

hang_exc:
    bl      rust_unexpected_exc
1:  wfe
    b       1b
"#
);

// ===================== mini-UART (igual que hello-rpi3) =====================
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
    // SAFETY: registro MMIO válido de la BCM2837.
    unsafe { write_volatile(a as *mut u32, v) }
}
#[inline]
fn mr(a: usize) -> u32 {
    // SAFETY: registro MMIO válido de la BCM2837.
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
    uart_puts("0x");
    let mut started = false;
    for i in (0..16).rev() {
        let nib = ((v >> (i * 4)) & 0xF) as u8;
        if nib != 0 || started || i == 0 {
            started = true;
            uart_send(if nib < 10 { b'0' + nib } else { b'a' + nib - 10 });
        }
    }
    let _ = &mut v;
}

fn uart_dec(mut v: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    uart_puts(core::str::from_utf8(&buf[i..]).unwrap_or("?"));
}

// ===================== MMU (tablas L1/L2, mapeo identidad) =====================
#[repr(C, align(4096))]
struct PageTable([u64; 512]);

static mut L1: PageTable = PageTable([0; 512]);
static mut L2: PageTable = PageTable([0; 512]);

// Atributos de descriptor (ARMv8 stage 1, granule 4 KiB).
const PT_VALID_BLOCK: u64 = 0b01; // entrada de bloque (L1=1 GiB, L2=2 MiB)
const PT_TABLE: u64 = 0b11; // entrada de tabla (apunta a otra tabla)
const PT_AF: u64 = 1 << 10; // Access Flag (si no, falla al primer acceso)
const PT_SH_INNER: u64 = 0b11 << 8; // Inner Shareable (para memoria normal)
const ATTR_DEVICE: u64 = 0 << 2; // AttrIndx=0 → MAIR attr0 (Device-nGnRnE)
const ATTR_NORMAL: u64 = 1 << 2; // AttrIndx=1 → MAIR attr1 (Normal WB)

/// Construye las tablas y enciende la MMU. El primer 1 GiB se mapea con bloques
/// de 2 MiB (L2) para separar RAM (Normal) de periféricos (Device); el segundo
/// GiB (0x4000_0000, ARM local peripherals) con un bloque L1 de 1 GiB Device.
unsafe fn mmu_init() {
    unsafe {
        let l2 = &mut (*core::ptr::addr_of_mut!(L2)).0;
        for (i, e) in l2.iter_mut().enumerate() {
            let pa = (i as u64) << 21; // i * 2 MiB
            *e = if pa < 0x3F00_0000 {
                pa | PT_VALID_BLOCK | PT_AF | PT_SH_INNER | ATTR_NORMAL
            } else {
                pa | PT_VALID_BLOCK | PT_AF | ATTR_DEVICE // periféricos
            };
        }
        let l1 = &mut (*core::ptr::addr_of_mut!(L1)).0;
        l1[0] = (core::ptr::addr_of!(L2) as u64) | PT_TABLE; // 0..1 GiB → L2
        l1[1] = 0x4000_0000 | PT_VALID_BLOCK | PT_AF | ATTR_DEVICE; // 1..2 GiB → ARM local

        // MAIR: attr0 = Device-nGnRnE (0x00), attr1 = Normal WB WA (0xFF).
        let mair: u64 = 0x00 | (0xFF << 8);
        core::arch::asm!("msr mair_el1, {}", in(reg) mair);

        // TCR_EL1: T0SZ=25 (VA 39 bits), 4 KiB granule, WB WA cacheable, inner
        // shareable; IPS desde la capacidad de PA del chip.
        let pa_range = {
            let mmfr0: u64;
            core::arch::asm!("mrs {}, id_aa64mmfr0_el1", out(reg) mmfr0);
            mmfr0 & 0xF
        };
        let tcr: u64 = 25 // T0SZ
            | (0b01 << 8)  // IRGN0 = WB WA
            | (0b01 << 10) // ORGN0 = WB WA
            | (0b11 << 12) // SH0 = inner shareable
            | (0b00 << 14) // TG0 = 4 KiB
            | (pa_range << 32); // IPS
        core::arch::asm!("msr tcr_el1, {}", in(reg) tcr);

        core::arch::asm!("msr ttbr0_el1, {}", in(reg) core::ptr::addr_of!(L1) as u64);
        core::arch::asm!("dsb ish; isb");

        // Enciende MMU (M) + D-cache (C) + I-cache (I) en SCTLR_EL1.
        let mut sctlr: u64;
        core::arch::asm!("mrs {}, sctlr_el1", out(reg) sctlr);
        sctlr |= (1 << 0) | (1 << 2) | (1 << 12);
        core::arch::asm!("msr sctlr_el1, {}; isb", in(reg) sctlr);
    }
}

// ===================== Generic Timer + ARM local routing =====================
/// ARM local peripherals (QA7). El timer físico se enruta al core 0 aquí, no por
/// GIC (deshabilitado en la RPi3 de fábrica).
const CORE0_TIMER_IRQCNTL: usize = 0x4000_0040;
const CORE0_IRQ_SOURCE: usize = 0x4000_0060;
const CNTPNSIRQ: u32 = 1 << 1; // physical non-secure timer → IRQ

/// Frecuencia del Generic Timer (Hz). Latcheada del HW (CNTFRQ_EL0).
static mut TIMER_HZ: u64 = 0;

/// Programa el siguiente disparo a `TIMER_HZ` cuentas (~1 s) y habilita el timer.
unsafe fn timer_rearm() {
    unsafe {
        let hz = read_volatile(core::ptr::addr_of!(TIMER_HZ));
        core::arch::asm!("msr cntp_tval_el0, {}", in(reg) hz);
        core::arch::asm!("msr cntp_ctl_el0, {}", in(reg) 1u64); // enable, sin máscara
    }
}

unsafe fn timer_init() {
    unsafe {
        let freq: u64;
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq);
        write_volatile(core::ptr::addr_of_mut!(TIMER_HZ), freq);
        timer_rearm();
        // Enruta el IRQ del timer físico NS al core 0.
        mw(CORE0_TIMER_IRQCNTL, CNTPNSIRQ);
    }
}

// ===================== Handlers en Rust =====================
static mut TICKS: u64 = 0;

#[no_mangle]
extern "C" fn rust_irq_handler() {
    // SAFETY: contexto de IRQ; acceso exclusivo a periféricos del timer.
    unsafe {
        let src = mr(CORE0_IRQ_SOURCE);
        if src & CNTPNSIRQ != 0 {
            let t = read_volatile(core::ptr::addr_of!(TICKS)) + 1;
            write_volatile(core::ptr::addr_of_mut!(TICKS), t);
            uart_puts("rugus rpi3 IRQ tick ");
            uart_dec(t);
            uart_puts("\n");
            timer_rearm(); // siguiente disparo
        } else {
            uart_puts("irq: fuente inesperada ");
            uart_hex(src as u64);
            uart_puts("\n");
        }
    }
}

#[no_mangle]
extern "C" fn rust_sync_handler(esr: u64, elr: u64) {
    uart_puts("\n!! EXCEPCION SYNC en EL1: ESR=");
    uart_hex(esr);
    uart_puts(" ELR=");
    uart_hex(elr);
    uart_puts("\n   (clase EC = ");
    uart_hex((esr >> 26) & 0x3F);
    uart_puts(")\n");
}

#[no_mangle]
extern "C" fn rust_unexpected_exc() {
    uart_puts("\n!! excepcion inesperada (FIQ/SError/lower EL)\n");
}

#[no_mangle]
extern "C" fn kernel_main() -> ! {
    uart_init();
    uart_puts("\n=== RUGUS @ RPi 3B+ — G5: EL1 + MMU + Timer (AArch64) ===\n");

    // Fase 1: ¿en qué EL quedamos?
    let el: u64;
    // SAFETY: lectura de registro de sistema.
    unsafe { core::arch::asm!("mrs {}, CurrentEL", out(reg) el) };
    uart_puts("[1] nivel de excepcion actual: EL");
    uart_dec((el >> 2) & 3);
    uart_puts("  (esperado EL1)\n");

    uart_puts("[2] vector table (VBAR_EL1) instalada en boot\n");

    // Fase 3: MMU.
    uart_puts("[3] activando MMU (identidad: RAM normal + periféricos device)...\n");
    // SAFETY: tablas estáticas propias; arranque single-thread.
    unsafe { mmu_init() };
    uart_puts("[3] MMU ON (caches I/D activas)\n");

    // Fase 4: timer + IRQs.
    uart_puts("[4] armando Generic Timer (CNTP) + ruteo IRQ al core 0...\n");
    // SAFETY: periféricos del timer; arranque single-thread.
    unsafe { timer_init() };
    let hz = unsafe { read_volatile(core::ptr::addr_of!(TIMER_HZ)) };
    uart_puts("[4] timer armado, CNTFRQ=");
    uart_dec(hz);
    uart_puts(" Hz. Desenmascarando IRQs...\n");

    // Desenmascara IRQs (DAIF.I = 0). A partir de aquí, los ticks llegan por IRQ.
    // SAFETY: vector table y handler listos.
    unsafe { core::arch::asm!("msr daifclr, #2") };

    uart_puts("[OK] esperando ticks por interrupcion real (1/s):\n");
    loop {
        // SAFETY: dormir hasta la próxima IRQ.
        unsafe { core::arch::asm!("wfi") };
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
