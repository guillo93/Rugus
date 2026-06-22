//! Rugus G5 — **preempción real** del scheduler de `rugus-core` en RPi 3B+.
//!
//! Cierra el lazo de la convergencia de kernel multiarquitectura: el **mismo**
//! `Scheduler<A>` arch-agnóstico que en los Cortex-M, aquí preemptado por el
//! **Generic Timer** vía `rugus-arch-cortex-a`. A diferencia de `rpi3-kernel`
//! (cooperativo, las tareas ceden con `yield_now`), aquí las tareas **no ceden**:
//! corren un bucle cerrado y las desaloja el timer cada quantum.
//!
//! Cableado idéntico al de Cortex-M:
//! - `time::set_preempt_hook(preempt_trampoline)` enruta la ISR del timer al
//!   `Scheduler::preempt_tick` (el mismo método que dispara SysTick en el M).
//! - `vectors::install()` fija la tabla `VBAR_EL1` del arch (handler de IRQ que
//!   salva el contexto y conmuta de tarea de forma anidada).
//! - `time::init(slice_ms)` arma el Generic Timer; `start()` entra en la primera
//!   tarea con IRQs habilitadas (porque la preempción quedó armada).
//!
//! El cambio de contexto efectivo lo hace `preempt_tick` cada `SLICE_TICKS`
//! vencimientos del quantum, igual que en el M; el frame de excepción del
//! handler es transparente para el scheduler.
//!
//! Boot (EL2→EL1 + FP/SIMD + MMU + mini-UART) reutilizado de `rpi3-kernel`. La
//! tabla de vectores de arranque es mínima (faults tempranos); la del arch la
//! instala `kernel_main` antes de armar el timer.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

use rugus_arch_cortex_a::{time, vectors, CortexA};
use rugus_core::sched::{Priority, Scheduler};

// ===================== Boot: EL2 → EL1 + VBAR temprano + FP/SIMD + bss =====================
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
    adr     x0, early_vectors
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

// Tabla de vectores temprana: captura faults síncronos durante el arranque
// (antes de instalar la tabla del arch). Cualquier excepción vuelca ESR/ELR.
global_asm!(
    r#"
.align 11
.global early_vectors
early_vectors:
    .rept 16
    .align 7
    b       el1_sync_early
    .endr
el1_sync_early:
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

// ===================== MMU (idéntica a rpi3-kernel) =====================
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
        // MAIR: attr 0 = Device-nGnRnE (0x00), attr 1 = Normal WB (0xFF).
        let mair: u64 = 0xFF << 8;
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

// ===================== Kernel: Scheduler<CortexA> preemptado por el timer =====================
const STACK_WORDS: usize = 2048; // 16 KiB por tarea
#[repr(C, align(16))]
struct Stack([u8; STACK_WORDS]);
static mut STACK_A: Stack = Stack([0; STACK_WORDS]);
static mut STACK_B: Stack = Stack([0; STACK_WORDS]);

/// El scheduler arch-agnóstico de `rugus-core`, parametrizado con el backend
/// AArch64. EL MISMO tipo que corre en los Cortex-M.
static mut SCHED: Option<Scheduler<CortexA>> = None;

/// Trampolín de preempción: lo llama la ISR del Generic Timer en cada
/// vencimiento del quantum (vía `time::fire_preempt_hook`). Rutea al
/// `preempt_tick` del scheduler — idéntico al trampolín de SysTick en Cortex-M.
fn preempt_trampoline() {
    // SAFETY: scheduler único; `preempt_tick` solo corre en la ISR del timer,
    // que el modo hilo enmascara mientras toca el scheduler (single-core).
    unsafe {
        if let Some(s) = (*addr_of_mut!(SCHED)).as_mut() {
            s.preempt_tick();
        }
    }
}

/// Cuerpo de tarea: imprime su marca en bucle **sin ceder**; la desaloja el
/// timer. El entrelazado de bloques A/B es la evidencia visual de la preempción.
fn task_body(mark: &str) -> ! {
    let mut n: u64 = 0;
    loop {
        uart_puts(mark);
        uart_dec(n);
        uart_puts(" ");
        n = n.wrapping_add(1);
        delay(6_000_000); // varias marcas por quantum
    }
}

fn task_a() -> ! {
    task_body("\nA#")
}
fn task_b() -> ! {
    task_body("\nB#")
}

#[no_mangle]
extern "C" fn kernel_main() -> ! {
    uart_init();
    uart_puts("\n=== RUGUS @ RPi 3B+ — G5: preempcion real (rugus-core sobre AArch64) ===\n");
    uart_puts("[1] EL1 + FP/SIMD + VBAR temprano ok\n");
    uart_puts("[2] MMU...\n");
    unsafe { mmu_init() };
    uart_puts("[2] MMU ON\n");

    uart_puts("[3] Scheduler<CortexA> de rugus-core: spawn A, B (no ceden)...\n");
    // SAFETY: arranque single-thread; pilas estáticas vivas para todo el kernel.
    unsafe {
        SCHED = Some(Scheduler::default());
        let s = (*addr_of_mut!(SCHED)).as_mut().unwrap();
        s.spawn(&mut (*addr_of_mut!(STACK_A)).0, task_a, Priority::App)
            .ok();
        s.spawn(&mut (*addr_of_mut!(STACK_B)).0, task_b, Priority::App)
            .ok();
    }

    uart_puts("[4] vectores del arch (VBAR_EL1) + Generic Timer (quantum 20 ms)...\n");
    time::set_preempt_hook(preempt_trampoline);
    vectors::install();
    time::init(20); // 20 ms × SLICE_TICKS del scheduler = quantum efectivo

    uart_puts("[OK] preempcion en marcha (A/B alternan por el timer, sin ceder):\n");
    // SAFETY: scheduler listo; `start` entra en la 1ª tarea con IRQs habilitadas
    // (la preempción quedó armada por `time::init`).
    unsafe {
        let s = (*addr_of_mut!(SCHED)).as_mut().unwrap();
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
