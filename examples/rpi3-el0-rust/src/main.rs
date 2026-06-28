//! Rugus G6.3b — **tarea userland EL0 escrita en Rust** en RPi 3B+.
//!
//! Continúa G6.2b (tarea EL0 en asm copiada). Aquí la tarea EL0 es una **`fn`
//! Rust** colocada en la sección de enlazado `.user` (VMA 0x200000, mapeada EL0:
//! AP=01, UXN=0). Eso la hace práctica: puede usar su pila y aritmética Rust, no
//! solo registros + `svc`. La planifica el **mismo `Scheduler<CortexA>`** de
//! `rugus-core` junto a una tarea EL1, cooperando por syscalls.
//!
//! Restricciones del userland (por el aislamiento): la `fn` EL0 solo puede tocar
//! su región `.user` (código/datos) y su pila EL0; NO puede llamar a funciones
//! del kernel (bloque kernel con UXN → EL0 no las ejecuta) ni leer datos del
//! kernel (AP=EL1-only). Sus helpers de syscall van `#[inline(always)]` para que
//! el `svc` se emita dentro de `.user.text`.
//!
//! Atajo del hito: una sola región EL0 compartida (kernel↔usuario). El
//! aislamiento usuario↔usuario por TTBR0+ASID queda como siguiente paso.

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use core::arch::global_asm;
use core::panic::PanicInfo;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

use rugus_arch_cortex_a::{vectors, CortexA};
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

// ===================== MMU con bits AP =====================
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

const USER_BLOCK_IDX: usize = 1; // bloque 0x200000-0x400000: .user + pila EL0
const USER_STACK_BASE: u64 = 0x30_0000; // pila EL0 (lejos del código .user)
const USER_STACK_LEN: usize = 0x8_0000;

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

// ===================== Tarea userland EL0 en RUST (sección .user) =====================
// Helpers de syscall: `#[inline(always)]` → el `svc` se emite DENTRO de la `fn`
// EL0 (en `.user.text`), sin llamadas externas al kernel.
#[inline(always)]
fn sys_putchar(c: u8) {
    // SAFETY: syscall 1 (putchar); el handler de SVC lo atiende.
    unsafe { core::arch::asm!("svc #0", in("x8") 1u64, in("x0") c as u64) };
}
#[inline(always)]
fn sys_yield() {
    // SAFETY: syscall 2 (yield); conmuta de tarea y vuelve.
    unsafe { core::arch::asm!("svc #0", in("x8") 2u64, lateout("x0") _) };
}

/// Tarea EL0 en Rust: cuenta 0..9 en bucle (aritmética Rust real en userland) e
/// imprime el dígito por syscall, cediendo entre cada uno. Vive en `.user.text`.
#[link_section = ".user.text"]
fn el0_rust_task() -> ! {
    let mut d: u8 = 0;
    loop {
        sys_putchar(b'0' + d);
        d = (d + 1) % 10;
        sys_yield();
    }
}

// ===================== Scheduler + syscalls (EL1) =====================
const STACK_WORDS: usize = 4096;
#[repr(C, align(16))]
struct Stack([u8; STACK_WORDS]);
static mut STACK_SUP: Stack = Stack([0; STACK_WORDS]);

static mut SCHED: Option<Scheduler<CortexA>> = None;

fn cpu_yield() {
    // SAFETY: scheduler único; cooperativo.
    unsafe {
        if let Some(s) = (*addr_of_mut!(SCHED)).as_mut() {
            s.yield_now();
        }
    }
}

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

fn supervisor() -> ! {
    loop {
        uart_puts(" [sup] ");
        cpu_yield();
    }
}

#[no_mangle]
extern "C" fn kernel_main() -> ! {
    uart_init();
    uart_puts("\n=== RUGUS @ RPi 3B+ — G6.3b: tarea EL0 escrita en Rust (.user) ===\n");
    uart_puts("[1] EL1 + FP/SIMD + VBAR temprano ok\n");
    uart_puts("[2] MMU (kernel UXN, bloque user EL0 en 0x200000)...\n");
    unsafe { mmu_init() };
    uart_puts("[2] MMU ON\n");
    // El código de la tarea EL0 ya está en 0x200000 (lo cargó la GPU con la
    // imagen); coherencia de I-cache por si acaso.
    unsafe { core::arch::asm!("ic iallu; dsb ish; isb") };

    uart_puts("[3] hooks de syscall + vectores del arch...\n");
    vectors::set_syscall_hook(syscall);
    vectors::install();

    uart_puts("[4] spawn: supervisor EL1 + tarea EL0 en Rust (spawn_user)\n");
    uart_puts("    esperado: 0 [sup] 1 [sup] 2 [sup] ...  (digitos de la fn Rust EL0)\n\n");
    // SAFETY: arranque single-thread; pilas vivas para todo el kernel.
    unsafe {
        SCHED = Some(Scheduler::default());
        let s = (*addr_of_mut!(SCHED)).as_mut().unwrap();
        s.spawn(&mut (*addr_of_mut!(STACK_SUP)).0, supervisor, Priority::App)
            .ok();
        let ustack = core::slice::from_raw_parts_mut(USER_STACK_BASE as *mut u8, USER_STACK_LEN);
        s.spawn_user(ustack, el0_rust_task, Priority::App).ok();
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
