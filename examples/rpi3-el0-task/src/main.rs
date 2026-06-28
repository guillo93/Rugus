//! Rugus G6.2b — **tarea userland EL0 planificada por `rugus-core`** en RPi 3B+.
//!
//! Cierra la integración de userland en AArch64: el **mismo `Scheduler<CortexA>`**
//! de `rugus-core` planifica una tarea **EL0** (sin privilegio, aislada por MMU)
//! junto a una tarea **EL1** (kernel), que cooperan vía **syscalls (`SVC`)**.
//! Apoyado en G6.2a (frame de excepción unificado en el backend) y en el handler
//! de `SVC` del arch (`vectors::set_syscall_hook`).
//!
//! - La tarea EL1 (supervisor) imprime su marca y cede con `yield_now`.
//! - La tarea EL0 hace syscalls `putchar('U')` y `yield` en bucle; corre con
//!   `SP_EL0` en una región mapeada EL0 y su código en el bloque user; **no puede
//!   tocar memoria del kernel** (AP=EL1-only) ni ejecutar código del kernel
//!   (UXN). El frame de la tarea EL0 vive en una pila kernel del backend
//!   (EL1-only), inaccesible a userland.
//! - `spawn_user` (TaskMode::User) construye el contexto EL0 vía
//!   `init_task_stack(privileged=false)`; el `yield` de EL0 conmuta de tarea por
//!   el handler de `SVC` (cpu_switch anidado, como la preempción).
//!
//! Atajo de este hito: la tarea EL0 es un stub en ensamblador copiado al bloque
//! user (PIC). Una tarea EL0 escrita en Rust necesita una sección de enlazado
//! `.user` mapeada EL0 — queda como siguiente paso, igual que el aislamiento
//! usuario↔usuario por TTBR0+ASID.

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use core::arch::global_asm;
use core::panic::PanicInfo;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

use rugus_arch_cortex_a::{vectors, CortexA};
use rugus_core::fault::{FaultKind, FaultReport};
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

// Vectores tempranos: faults de arranque antes de instalar la tabla del arch.
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

// Stub de la tarea userland EL0 (se copia al bloque user 0x200000). PIC: bucle
// de putchar('U') + yield por syscall. b 1b es PC-relativo → válido tras copiar.
.global el0_stub_start
el0_stub_start:
    mov     x19, #0                  // contador (preservado por el frame)
1:
    mov     x8, #1                   // SYS_PUTCHAR
    mov     x0, #0x55                // 'U'
    svc     #0
    add     x19, x19, #1
    cmp     x19, #3
    b.ge    3f
    mov     x8, #2                   // SYS_YIELD
    svc     #0
    b       1b
3:
    movz    x1, #0x8, lsl #16        // 0x80000 (kernel, EL1-only)
    ldr     x2, [x1]                 // acceso prohibido → abort → el kernel mata la tarea
    b       .
.global el0_stub_end
el0_stub_end:
"#
);

extern "C" {
    static el0_stub_start: u8;
    static el0_stub_end: u8;
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
fn uart_hex(v: u64) {
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
const AP_EL0_RW: u64 = 0b01 << 6;
const PT_UXN: u64 = 1 << 54;

const USER_BLOCK_IDX: usize = 1;
const USER_BASE: u64 = (USER_BLOCK_IDX as u64) << 21; // 0x200000 (código EL0 copiado)
const USER_STACK_BASE: u64 = 0x28_0000; // pila EL0 (dentro del bloque user)
const USER_STACK_LEN: usize = 0x8_0000; // 0x280000..0x300000

unsafe fn mmu_init() {
    unsafe {
        let l2 = &mut (*addr_of_mut!(L2)).0;
        for (i, e) in l2.iter_mut().enumerate() {
            let pa = (i as u64) << 21;
            *e = if pa >= 0x3F00_0000 {
                pa | PT_BLOCK | PT_AF | ATTR_DEVICE
            } else if i == USER_BLOCK_IDX {
                pa | PT_BLOCK | PT_AF | PT_SH_INNER | ATTR_NORMAL | AP_EL0_RW
            } else {
                pa | PT_BLOCK | PT_AF | PT_SH_INNER | ATTR_NORMAL | PT_UXN
            };
        }
        let l1 = &mut (*addr_of_mut!(L1)).0;
        l1[0] = (addr_of!(L2) as u64) | PT_TABLE;
        l1[1] = 0x4000_0000 | PT_BLOCK | PT_AF | ATTR_DEVICE;
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
    uart_puts("\n!! FAULT (EL1) ESR=");
    uart_hex(esr);
    uart_puts(" ELR=");
    uart_hex(elr);
    uart_puts("\n");
}

// ===================== Scheduler + syscalls =====================
const STACK_WORDS: usize = 4096; // 32 KiB pila kernel del supervisor EL1
#[repr(C, align(16))]
struct Stack([u8; STACK_WORDS]);
static mut STACK_SUP: Stack = Stack([0; STACK_WORDS]);

static mut SCHED: Option<Scheduler<CortexA>> = None;

fn cpu_yield() {
    // SAFETY: scheduler único; cooperativo (single-core).
    unsafe {
        if let Some(s) = (*addr_of_mut!(SCHED)).as_mut() {
            s.yield_now();
        }
    }
}

/// Despachador de syscalls (lo invoca el handler de `SVC` desde EL0).
/// 1 = putchar(arg), 2 = yield. Devuelve 0 (o -1 si desconocida).
fn syscall(num: u64, arg: u64) -> u64 {
    match num {
        1 => {
            uart_send((arg & 0xFF) as u8);
            0
        }
        2 => {
            cpu_yield();
            0
        }
        _ => u64::MAX,
    }
}

/// Abort de EL0: **contención real** — informa, mata la tarea EL0 faultante y
/// reanuda otra (no retorna). El kernel sobrevive y las demás tareas siguen.
fn el0_fault(esr: u64, far: u64, elr: u64) {
    uart_puts("\n[EL1] abort de EL0 -> matando la tarea y siguiendo (contencion):\n  ESR=");
    uart_hex(esr);
    uart_puts(" FAR=");
    uart_hex(far);
    uart_puts(" ELR=");
    uart_hex(elr);
    uart_puts("\n");
    // SAFETY: scheduler único; `current` es la tarea EL0 faultante.
    unsafe {
        let s = (*addr_of_mut!(SCHED)).as_mut().unwrap();
        let report = FaultReport {
            kind: FaultKind::MemManage, // permission abort de EL0 = violación de protección
            pc: elr as u32,
            domain: s.current_domain(),
            task_id: s.current_id(),
            addr: Some(far as u32),
        };
        s.kill_current_and_resume(report)
    }
}

/// Tarea supervisora (EL1): imprime su marca y cede.
fn supervisor() -> ! {
    loop {
        uart_puts("[sup] ");
        cpu_yield();
    }
}

#[no_mangle]
extern "C" fn kernel_main() -> ! {
    uart_init();
    uart_puts("\n=== RUGUS @ RPi 3B+ — G6.2b: tarea EL0 planificada por rugus-core ===\n");
    uart_puts("[1] EL1 + FP/SIMD + VBAR temprano ok\n");
    uart_puts("[2] MMU (kernel EL1-only/UXN, bloque user EL0 en 0x200000)...\n");
    unsafe { mmu_init() };
    uart_puts("[2] MMU ON\n");

    // Copia el stub EL0 al bloque user.
    let src = addr_of!(el0_stub_start);
    let len = addr_of!(el0_stub_end) as usize - src as usize;
    uart_puts("[3] copiando stub EL0 a 0x200000 (");
    uart_hex(len as u64);
    uart_puts(" bytes)...\n");
    // SAFETY: destino en el bloque user (RAM, EL0+EL1 RW).
    unsafe {
        let dst = USER_BASE as *mut u8;
        for i in 0..len {
            write_volatile(dst.add(i), read_volatile(src.add(i)));
        }
        core::arch::asm!("dsb ish; ic iallu; dsb ish; isb");
    }

    uart_puts("[4] hooks de syscall/fault + vectores del arch...\n");
    vectors::set_syscall_hook(syscall);
    vectors::set_el0_fault_hook(el0_fault);
    vectors::install();

    uart_puts("[5] spawn: supervisor EL1 + tarea EL0 (spawn_user); arrancando\n");
    uart_puts("    esperado: U[sup]x3, EL0 toca el kernel -> abort -> kernel la mata\n");
    uart_puts("    y el supervisor EL1 SIGUE corriendo (contencion real)\n\n");
    // SAFETY: arranque single-thread; pilas vivas para todo el kernel.
    unsafe {
        SCHED = Some(Scheduler::default());
        let s = (*addr_of_mut!(SCHED)).as_mut().unwrap();
        s.spawn(&mut (*addr_of_mut!(STACK_SUP)).0, supervisor, Priority::App)
            .ok();
        // Pila de USUARIO en el bloque EL0; el código EL0 está en USER_BASE.
        let ustack = core::slice::from_raw_parts_mut(USER_STACK_BASE as *mut u8, USER_STACK_LEN);
        let el0_entry: fn() -> ! = core::mem::transmute(USER_BASE as usize);
        s.spawn_user(ustack, el0_entry, Priority::App).ok();
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
