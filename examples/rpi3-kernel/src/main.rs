//! Rugus G5 — el **scheduler del kernel compartido** (`rugus_core::sched`)
//! corriendo en la Raspberry Pi 3B+ vía `rugus-arch-cortex-a` (AArch64).
//!
//! A diferencia de `rpi3-sched` (scheduler hecho a mano para demostrar el
//! context switch), aquí corre el **mismo `Scheduler<A>` arch-agnóstico** que en
//! los Cortex-M, parametrizado con el backend `CortexA`. Es la prueba de la
//! convergencia real de kernel: una sola lógica de planificación, dos
//! arquitecturas. Dos tareas cooperativas que ceden el CPU con `yield_now`.
//!
//! Boot (EL2→EL1 + FP/SIMD + MMU + mini-UART) reutilizado de `rpi3-sched`.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

use rugus_arch_cortex_a::CortexA;
use rugus_core::sched::{Priority, Scheduler};

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

// Vector table mínima: cualquier excepción síncrona vuelca ESR/ELR y para. El
// scheduler es cooperativo (sin IRQ), pero conviene capturar faults.
global_asm!(
    r#"
.align 11
.global vector_table
vector_table:
    .rept 4
    .align 7
    b       el1_sync
    .endr
    .rept 4
    .align 7
    b       el1_sync
    .endr
    .rept 8
    .align 7
    b       el1_sync
    .endr
el1_sync:
    mrs     x0, esr_el1
    mrs     x1, elr_el1
    bl      rust_fault
1:  wfe
    b       1b
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

// ===================== MMU (idéntica a rpi3-sched) =====================
#[repr(C, align(4096))]
struct PageTable([u64; 512]);
static mut L1: PageTable = PageTable([0; 512]);
static mut L2: PageTable = PageTable([0; 512]);
unsafe fn mmu_init() {
    unsafe {
        let l2 = &mut (*addr_of_mut!(L2)).0;
        for (i, e) in l2.iter_mut().enumerate() {
            let pa = (i as u64) << 21;
            *e = if pa < 0x3F00_0000 {
                pa | 0b01 | (1 << 10) | (0b11 << 8) | (1 << 2)
            } else {
                pa | 0b01 | (1 << 10)
            };
        }
        let l1 = &mut (*addr_of_mut!(L1)).0;
        l1[0] = (addr_of!(L2) as u64) | 0b11;
        l1[1] = 0x4000_0000 | 0b01 | (1 << 10);
        let mair: u64 = 0x00 | (0xFF << 8);
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

#[no_mangle]
extern "C" fn rust_fault(esr: u64, elr: u64) {
    uart_puts("\n!! FAULT ESR=");
    uart_dec(esr);
    uart_puts(" ELR=");
    uart_dec(elr);
    uart_puts("\n");
}

// ===================== Kernel: Scheduler<CortexA> de rugus-core =====================
const STACK_WORDS: usize = 2048; // 16 KiB por tarea
#[repr(C, align(16))]
struct Stack([u8; STACK_WORDS]);
static mut STACK_A: Stack = Stack([0; STACK_WORDS]);
static mut STACK_B: Stack = Stack([0; STACK_WORDS]);

/// El scheduler arch-agnóstico de `rugus-core`, parametrizado con el backend
/// AArch64. Es EL MISMO tipo que corre en los Cortex-M.
static mut SCHED: Option<Scheduler<CortexA>> = None;

/// Cede el CPU al scheduler (cooperativo). Helper equivalente al `cpu_yield` de
/// `rugus-kernel` en los Cortex-M.
fn cpu_yield() {
    // SAFETY: scheduler único; cooperativo sin reentrada concurrente (single-core).
    unsafe {
        if let Some(s) = (*addr_of_mut!(SCHED)).as_mut() {
            s.yield_now();
        }
    }
}

fn task_a() -> ! {
    let mut n: u64 = 0;
    loop {
        uart_puts("[A] vuelta ");
        uart_dec(n);
        uart_puts("\n");
        n = n.wrapping_add(1);
        delay(20_000_000);
        cpu_yield();
    }
}

fn task_b() -> ! {
    let mut n: u64 = 0;
    loop {
        uart_puts("[B] vuelta ");
        uart_dec(n);
        uart_puts("\n");
        n = n.wrapping_add(1);
        delay(20_000_000);
        cpu_yield();
    }
}

#[no_mangle]
extern "C" fn kernel_main() -> ! {
    uart_init();
    uart_puts("\n=== RUGUS @ RPi 3B+ — kernel-core sobre rugus-arch-cortex-a ===\n");
    uart_puts("[1] EL1 + FP/SIMD + VBAR ok\n");
    uart_puts("[2] MMU...\n");
    unsafe { mmu_init() };
    uart_puts("[2] MMU ON\n");

    uart_puts("[3] Scheduler<CortexA> de rugus-core: spawn A, B...\n");
    // SAFETY: arranque single-thread; pilas estáticas vivas para todo el kernel.
    unsafe {
        SCHED = Some(Scheduler::default());
        let s = (*addr_of_mut!(SCHED)).as_mut().unwrap();
        s.spawn(&mut (*addr_of_mut!(STACK_A)).0, task_a, Priority::App)
            .ok();
        s.spawn(&mut (*addr_of_mut!(STACK_B)).0, task_b, Priority::App)
            .ok();
        uart_puts("[OK] arrancando el scheduler compartido (A/B cooperativas):\n");
        s.start();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    uart_puts("\n!! PANIC\n");
    loop {
        core::hint::spin_loop();
    }
}
