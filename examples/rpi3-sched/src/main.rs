//! Rugus G5 — scheduler preemptivo AArch64 en RPi 3B+: context switch + 2 tareas
//! alternadas por la IRQ del timer.
//!
//! Sobre `rpi3-timer` (EL1 + MMU + Generic Timer por IRQ) añade lo que convierte
//! "interrupciones" en "kernel": el **cambio de contexto**. Cada tarea tiene su
//! pila y su frame de registros guardado; en cada IRQ del timer el handler
//! vuelca el contexto de la tarea actual a su pila, el scheduler elige la
//! siguiente y se restaura su contexto. Las tareas **no ceden** — las preempta
//! el timer (round-robin). Es el "hola scheduler" de la segunda arquitectura.
//!
//! Frame de excepción (272 B, alineado a 16): x0..x30 en [0..248], ELR_EL1 en
//! [256], SPSR_EL1 en [264]. El layout lo comparten el handler IRQ (asm) y
//! `init_task` (Rust) — deben cuadrar exactamente.

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
    mov     x0, #(3 << 20)           // CPACR_EL1.FPEN: habilita FP/SIMD (EC=0x7 si no)
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

// ===================== Vector table + context switch =====================
global_asm!(
    r#"
.align 11
.global vector_table
vector_table:
    .align 7
    b       el1_sync
    .align 7
    b       el1_irq
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       el1_sync       // Current EL SPx: sync
    .align 7
    b       el1_irq        // Current EL SPx: IRQ ← preempción
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc

// IRQ: guarda contexto completo en la pila de la tarea, llama al scheduler
// (devuelve el SP de la tarea siguiente) y restaura ese contexto.
el1_irq:
    sub     sp, sp, #272
    stp     x0,  x1,  [sp, #16 * 0]
    stp     x2,  x3,  [sp, #16 * 1]
    stp     x4,  x5,  [sp, #16 * 2]
    stp     x6,  x7,  [sp, #16 * 3]
    stp     x8,  x9,  [sp, #16 * 4]
    stp     x10, x11, [sp, #16 * 5]
    stp     x12, x13, [sp, #16 * 6]
    stp     x14, x15, [sp, #16 * 7]
    stp     x16, x17, [sp, #16 * 8]
    stp     x18, x19, [sp, #16 * 9]
    stp     x20, x21, [sp, #16 * 10]
    stp     x22, x23, [sp, #16 * 11]
    stp     x24, x25, [sp, #16 * 12]
    stp     x26, x27, [sp, #16 * 13]
    stp     x28, x29, [sp, #16 * 14]
    str     x30,      [sp, #240]
    mrs     x9,  elr_el1
    mrs     x10, spsr_el1
    stp     x9,  x10, [sp, #256]
    mov     x0, sp
    bl      rust_schedule            // x0 = sp actual → devuelve sp siguiente
    mov     sp, x0
    b       restore_and_eret

// Arranca la primera tarea: restaura su contexto y eret (no retorna).
.global start_first
start_first:
    mov     sp, x0
restore_and_eret:
    ldp     x9,  x10, [sp, #256]
    msr     elr_el1, x9
    msr     spsr_el1, x10
    ldp     x0,  x1,  [sp, #16 * 0]
    ldp     x2,  x3,  [sp, #16 * 1]
    ldp     x4,  x5,  [sp, #16 * 2]
    ldp     x6,  x7,  [sp, #16 * 3]
    ldp     x8,  x9,  [sp, #16 * 4]
    ldp     x10, x11, [sp, #16 * 5]
    ldp     x12, x13, [sp, #16 * 6]
    ldp     x14, x15, [sp, #16 * 7]
    ldp     x16, x17, [sp, #16 * 8]
    ldp     x18, x19, [sp, #16 * 9]
    ldp     x20, x21, [sp, #16 * 10]
    ldp     x22, x23, [sp, #16 * 11]
    ldp     x24, x25, [sp, #16 * 12]
    ldp     x26, x27, [sp, #16 * 13]
    ldp     x28, x29, [sp, #16 * 14]
    ldr     x30,      [sp, #240]
    add     sp, sp, #272
    eret

el1_sync:
    mrs     x0, esr_el1
    mrs     x1, elr_el1
    bl      rust_sync_handler
1:  wfe
    b       1b

hang_exc:
    bl      rust_unexpected_exc
1:  wfe
    b       1b
"#
);

extern "C" {
    fn start_first(sp: u64) -> !;
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

// ===================== MMU (idéntica a rpi3-timer) =====================
#[repr(C, align(4096))]
struct PageTable([u64; 512]);
static mut L1: PageTable = PageTable([0; 512]);
static mut L2: PageTable = PageTable([0; 512]);
const PT_VALID_BLOCK: u64 = 0b01;
const PT_TABLE: u64 = 0b11;
const PT_AF: u64 = 1 << 10;
const PT_SH_INNER: u64 = 0b11 << 8;
const ATTR_DEVICE: u64 = 0 << 2;
const ATTR_NORMAL: u64 = 1 << 2;

unsafe fn mmu_init() {
    unsafe {
        let l2 = &mut (*addr_of_mut!(L2)).0;
        for (i, e) in l2.iter_mut().enumerate() {
            let pa = (i as u64) << 21;
            *e = if pa < 0x3F00_0000 {
                pa | PT_VALID_BLOCK | PT_AF | PT_SH_INNER | ATTR_NORMAL
            } else {
                pa | PT_VALID_BLOCK | PT_AF | ATTR_DEVICE
            };
        }
        let l1 = &mut (*addr_of_mut!(L1)).0;
        l1[0] = (addr_of!(L2) as u64) | PT_TABLE;
        l1[1] = 0x4000_0000 | PT_VALID_BLOCK | PT_AF | ATTR_DEVICE;
        let mair: u64 = 0x00 | (0xFF << 8);
        core::arch::asm!("msr mair_el1, {}", in(reg) mair);
        let pa_range = {
            let m: u64;
            core::arch::asm!("mrs {}, id_aa64mmfr0_el1", out(reg) m);
            m & 0xF
        };
        let tcr: u64 =
            25 | (0b01 << 8) | (0b01 << 10) | (0b11 << 12) | (0b00 << 14) | (pa_range << 32);
        core::arch::asm!("msr tcr_el1, {}", in(reg) tcr);
        core::arch::asm!("msr ttbr0_el1, {}", in(reg) addr_of!(L1) as u64);
        core::arch::asm!("dsb ish; isb");
        let mut sctlr: u64;
        core::arch::asm!("mrs {}, sctlr_el1", out(reg) sctlr);
        sctlr |= (1 << 0) | (1 << 2) | (1 << 12);
        core::arch::asm!("msr sctlr_el1, {}; isb", in(reg) sctlr);
    }
}

// ===================== Timer (quantum del scheduler) =====================
const CORE0_TIMER_IRQCNTL: usize = 0x4000_0040;
const CORE0_IRQ_SOURCE: usize = 0x4000_0060;
const CNTPNSIRQ: u32 = 1 << 1;
static mut QUANTUM: u64 = 0; // cuentas del timer por rodaja

unsafe fn timer_rearm() {
    unsafe {
        let q = read_volatile(addr_of!(QUANTUM));
        core::arch::asm!("msr cntp_tval_el0, {}", in(reg) q);
        core::arch::asm!("msr cntp_ctl_el0, {}", in(reg) 1u64);
    }
}
unsafe fn timer_init() {
    unsafe {
        let freq: u64;
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq);
        write_volatile(addr_of_mut!(QUANTUM), freq / 3); // ~0.33 s por rodaja
        timer_rearm();
        mw(CORE0_TIMER_IRQCNTL, CNTPNSIRQ);
    }
}

// ===================== Scheduler =====================
const NTASKS: usize = 2;
const TASK_FRAME: usize = 272; // bytes del frame de excepción
const STACK_WORDS: usize = 1024; // 8 KiB por tarea

#[repr(C, align(16))]
struct Stack([u64; STACK_WORDS]);
static mut STACK_A: Stack = Stack([0; STACK_WORDS]);
static mut STACK_B: Stack = Stack([0; STACK_WORDS]);

static mut TCB_SP: [u64; NTASKS] = [0; NTASKS];
static mut CURRENT: usize = 0;
static mut SWITCHES: u64 = 0;

/// Construye el frame inicial de una tarea en el tope de su pila: todos los
/// GPR a 0, `ELR_EL1`=entry, `SPSR_EL1`=EL1h con IRQ habilitado (para que el
/// timer la preempte). Devuelve el SP que apunta al frame.
unsafe fn init_task(stack_top: usize, entry: u64) -> u64 {
    let sp = stack_top - TASK_FRAME;
    let f = sp as *mut u64;
    unsafe {
        for i in 0..(TASK_FRAME / 8) {
            write_volatile(f.add(i), 0);
        }
        write_volatile(f.add(32), entry); // offset 256 → ELR_EL1
        write_volatile(f.add(33), 0x5); // offset 264 → SPSR: EL1h, DAIF=0 (IRQ on)
    }
    sp as u64
}

/// Llamado desde el handler IRQ con el SP de la tarea interrumpida. Atiende el
/// timer, guarda ese SP, elige la siguiente tarea (round-robin) y devuelve su SP.
#[no_mangle]
extern "C" fn rust_schedule(sp: u64) -> u64 {
    unsafe {
        if mr(CORE0_IRQ_SOURCE) & CNTPNSIRQ != 0 {
            timer_rearm();
        }
        let tcb = addr_of_mut!(TCB_SP) as *mut u64;
        let cur = read_volatile(addr_of!(CURRENT));
        write_volatile(tcb.add(cur), sp); // guarda el SP de la tarea saliente
        let next = (cur + 1) % NTASKS;
        write_volatile(addr_of_mut!(CURRENT), next);
        write_volatile(addr_of_mut!(SWITCHES), read_volatile(addr_of!(SWITCHES)) + 1);
        read_volatile(tcb.add(next)) // SP de la tarea entrante
    }
}

/// Cuerpo común de las tareas: imprime su marca en bucle SIN ceder; la preempta
/// el timer. El entrelazado ocasional de bytes entre A y B es esperado (UART
/// compartido sin lock) y es justo la evidencia visual de la preempción.
fn task_body(mark: &str) -> ! {
    let mut n: u64 = 0;
    loop {
        uart_puts(mark);
        uart_dec(n);
        uart_puts(" ");
        n = n.wrapping_add(1);
        delay(8_000_000); // ~varias marcas por rodaja
    }
}

extern "C" fn task_a() -> ! {
    task_body("\nA#")
}
extern "C" fn task_b() -> ! {
    task_body("\nB#")
}

#[no_mangle]
extern "C" fn rust_sync_handler(esr: u64, elr: u64) {
    uart_puts("\n!! SYNC EL1 ESR=");
    uart_dec(esr);
    uart_puts(" ELR=");
    uart_dec(elr);
    uart_puts("\n");
}
#[no_mangle]
extern "C" fn rust_unexpected_exc() {
    uart_puts("\n!! excepcion inesperada\n");
}

#[no_mangle]
extern "C" fn kernel_main() -> ! {
    uart_init();
    uart_puts("\n=== RUGUS @ RPi 3B+ — G5: scheduler preemptivo (AArch64) ===\n");
    uart_puts("[1] EL1 + VBAR + FP/SIMD ok\n");

    uart_puts("[2] MMU...\n");
    unsafe { mmu_init() };
    uart_puts("[2] MMU ON\n");

    uart_puts("[3] timer (quantum ~0.33s)...\n");
    unsafe { timer_init() };
    uart_puts("[3] timer armado\n");

    // Crea dos tareas con su frame inicial.
    uart_puts("[4] creando 2 tareas (A, B) y saltando a la primera...\n");
    let (sp0, sp1) = unsafe {
        let ea = (task_a as extern "C" fn() -> !) as usize as u64;
        let eb = (task_b as extern "C" fn() -> !) as usize as u64;
        let a = init_task(addr_of!(STACK_A) as usize + STACK_WORDS * 8, ea);
        let b = init_task(addr_of!(STACK_B) as usize + STACK_WORDS * 8, eb);
        (a, b)
    };
    unsafe {
        let tcb = addr_of_mut!(TCB_SP) as *mut u64;
        write_volatile(tcb.add(0), sp0);
        write_volatile(tcb.add(1), sp1);
        write_volatile(addr_of_mut!(CURRENT), 0);
    }

    uart_puts("[OK] preempcion en marcha (A/B alternan por el timer):\n");
    // Desenmascara IRQs y salta a la tarea 0. A partir de aquí manda el timer.
    unsafe {
        core::arch::asm!("msr daifclr, #2");
        start_first(sp0);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
