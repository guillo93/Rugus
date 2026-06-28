//! Rugus G6.3c — **aislamiento userland↔userland por TTBR0** en RPi 3B+.
//!
//! Completa el trío EL0: hasta ahora el aislamiento era kernel↔usuario (una sola
//! región EL0 compartida). Aquí **dos tareas EL0 no se ven entre sí**: cada una
//! tiene su propia **tabla de traducción** (`TTBR0_EL1` por tarea), que el
//! backend conmuta en cada cambio de contexto vía `on_task_switch` →
//! `vectors::set_addr_space_hook`.
//!
//! - Tarea A: su código/datos/pila en el bloque 1 (0x200000), mapeado EL0 SOLO
//!   en la tabla de A. Tarea B: bloque 2 (0x400000), EL0 solo en la de B. El
//!   bloque del otro queda EL1-only en cada tabla → inalcanzable desde EL0.
//! - Ambas corren el mismo código (el de B es una copia del de A) e incrementan
//!   un **contador privado** (`MARKER`) en su propia región: A imprime A,B,C…,
//!   B imprime a,b,c… **independientes**. Si compartieran memoria, las dos
//!   secuencias interferirían; que sean independientes prueba el aislamiento.
//!
//! Atajo del hito: identidad + flush de TLB en cada switch (correcto y simple).
//! La optimización por **ASID** (entradas no-globales etiquetadas, sin flush)
//! queda como follow-up. El `stack_base` (VA de la pila, en bloque distinto por
//! tarea) discrimina la tabla en el hook.

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use core::arch::global_asm;
use core::panic::PanicInfo;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

use rugus_arch_cortex_a::{vectors, CortexA};
use rugus_core::sched::{Priority, Scheduler};

// ===================== Boot =====================
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

// ===================== Tablas de páginas: 3 (kernel, A, B) =====================
#[repr(C, align(4096))]
struct PageTable([u64; 512]);
static mut L1_K: PageTable = PageTable([0; 512]);
static mut L2_K: PageTable = PageTable([0; 512]);
static mut L1_A: PageTable = PageTable([0; 512]);
static mut L2_A: PageTable = PageTable([0; 512]);
static mut L1_B: PageTable = PageTable([0; 512]);
static mut L2_B: PageTable = PageTable([0; 512]);

const PT_BLOCK: u64 = 0b01;
const PT_TABLE: u64 = 0b11;
const PT_AF: u64 = 1 << 10;
const PT_SH_INNER: u64 = 0b11 << 8;
const ATTR_NORMAL: u64 = 1 << 2;
const ATTR_DEVICE: u64 = 0 << 2;
const AP_EL0_RW: u64 = 0b01 << 6;
const PT_UXN: u64 = 1 << 54;

const BLOCK_A: usize = 1; // 0x200000 (código/datos/pila de A)
const BLOCK_B: usize = 2; // 0x400000 (de B)
const PHYS_A: u64 = (BLOCK_A as u64) << 21; // 0x200000
const PHYS_B: u64 = (BLOCK_B as u64) << 21; // 0x400000
const A_STACK_TOP: u64 = 0x3C_0000; // pila EL0 de A (bloque 1)
const A_STACK_BASE: u64 = 0x38_0000;
const B_STACK_TOP: u64 = 0x5C_0000; // pila EL0 de B (bloque 2)
const B_STACK_BASE: u64 = 0x58_0000;

/// Llena un L2: `el0_block` (si lo hay) → EL0 RW+exec; el resto Normal EL1-only
/// (UXN); periféricos Device. Una L2 cubre 1 GiB (512 × 2 MiB).
fn build_l2(l2: &mut [u64; 512], el0_block: Option<usize>) {
    for (i, e) in l2.iter_mut().enumerate() {
        let pa = (i as u64) << 21;
        *e = if pa >= 0x3F00_0000 {
            pa | PT_BLOCK | PT_AF | ATTR_DEVICE
        } else if Some(i) == el0_block {
            pa | PT_BLOCK | PT_AF | PT_SH_INNER | ATTR_NORMAL | AP_EL0_RW // UXN=0 → EL0 ejecuta
        } else {
            pa | PT_BLOCK | PT_AF | PT_SH_INNER | ATTR_NORMAL | PT_UXN // EL1-only
        };
    }
}

unsafe fn mmu_init() {
    unsafe {
        build_l2(&mut (*addr_of_mut!(L2_K)).0, None); // kernel: ningún bloque EL0
        build_l2(&mut (*addr_of_mut!(L2_A)).0, Some(BLOCK_A));
        build_l2(&mut (*addr_of_mut!(L2_B)).0, Some(BLOCK_B));
        for (l1, l2) in [
            (addr_of_mut!(L1_K), addr_of!(L2_K)),
            (addr_of_mut!(L1_A), addr_of!(L2_A)),
            (addr_of_mut!(L1_B), addr_of!(L2_B)),
        ] {
            (*l1).0[0] = (l2 as u64) | PT_TABLE;
            (*l1).0[1] = 0x4000_0000 | PT_BLOCK | PT_AF | ATTR_DEVICE;
        }
        let mair: u64 = 0xFF << 8;
        core::arch::asm!("msr mair_el1, {}", in(reg) mair);
        let m: u64;
        core::arch::asm!("mrs {}, id_aa64mmfr0_el1", out(reg) m);
        let tcr: u64 = 25 | (0b01 << 8) | (0b01 << 10) | (0b11 << 12) | ((m & 0xF) << 32);
        core::arch::asm!("msr tcr_el1, {}", in(reg) tcr);
        // Arranca con la tabla del kernel.
        core::arch::asm!("msr ttbr0_el1, {}", in(reg) addr_of!(L1_K) as u64);
        core::arch::asm!("dsb ish; isb");
        let mut sctlr: u64;
        core::arch::asm!("mrs {}, sctlr_el1", out(reg) sctlr);
        sctlr |= (1 << 0) | (1 << 2) | (1 << 12);
        core::arch::asm!("msr sctlr_el1, {}; isb", in(reg) sctlr);
    }
}

/// Hook de espacio de direcciones: conmuta `TTBR0_EL1` por tarea y vacía el TLB.
/// `stack_base` discrimina (pila de A en bloque 1, de B en bloque 2).
fn addr_space(is_user: bool, stack_base: u32) {
    let l1 = if !is_user {
        addr_of!(L1_K) as u64
    } else if (stack_base as u64) < PHYS_B {
        addr_of!(L1_A) as u64
    } else {
        addr_of!(L1_B) as u64
    };
    // SAFETY: cambia la tabla de traducción de bajo nivel + flush de TLB.
    unsafe {
        core::arch::asm!(
            "msr ttbr0_el1, {t}",
            "tlbi vmalle1",
            "dsb nsh",
            "isb",
            t = in(reg) l1,
        );
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

// ===================== Tarea EL0 (en .user) =====================
/// Contador privado de la tarea: vive en `.user.data`, así A lo lee/escribe en
/// su bloque y B (copia) en el suyo — instancias independientes.
#[link_section = ".user.data"]
#[no_mangle]
static mut MARKER: u8 = b'A';

#[inline(always)]
fn sys_putchar(c: u8) {
    // SAFETY: syscall 1. x0 es clobbered: el handler escribe en él el valor de
    // retorno, así que NO se puede declarar como `in` (el compilador creería que
    // se conserva y reusaría el valor → corrupción).
    unsafe { core::arch::asm!("svc #0", in("x8") 1u64, inout("x0") c as u64 => _) };
}
#[inline(always)]
fn sys_yield() {
    // SAFETY: syscall 2.
    unsafe { core::arch::asm!("svc #0", in("x8") 2u64, lateout("x0") _) };
}

/// Bucle: imprime su MARKER privado, lo incrementa (A..Z / a..z) y cede. Cada
/// tarea toca SOLO su propia copia de MARKER (en su bloque) → secuencias
/// independientes = aislamiento.
#[link_section = ".user.text"]
fn el0_task() -> ! {
    loop {
        // SAFETY: MARKER está en la región .user mapeada EL0 de esta tarea.
        let m = unsafe { read_volatile(addr_of!(MARKER)) };
        sys_putchar(m);
        let nx = if m < b'a' {
            if m >= b'Z' {
                b'A'
            } else {
                m + 1
            }
        } else if m >= b'z' {
            b'a'
        } else {
            m + 1
        };
        // SAFETY: escritura en MARKER privado de esta tarea.
        unsafe { write_volatile(addr_of_mut!(MARKER), nx) };
        sys_yield();
    }
}

// ===================== Scheduler + syscalls (EL1) =====================
static mut SCHED: Option<Scheduler<CortexA>> = None;

fn cpu_yield() {
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

#[no_mangle]
extern "C" fn kernel_main() -> ! {
    uart_init();
    uart_puts("\n=== RUGUS @ RPi 3B+ — G6.3c: aislamiento EL0<->EL0 por TTBR0 ===\n");
    uart_puts("[1] EL1 + FP/SIMD + VBAR ok\n");
    uart_puts("[2] MMU: 3 tablas (kernel, A->bloque1, B->bloque2)...\n");
    unsafe { mmu_init() };
    uart_puts("[2] MMU ON\n");

    // El código/datos de A ya están en PHYS_A (0x200000) por la imagen. Copia la
    // región .user a PHYS_B (0x400000) para la tarea B, y resiembra su MARKER.
    let us = addr_of!(__user_start) as u64;
    let ue = addr_of!(__user_end) as u64;
    let len = (ue - us) as usize;
    uart_puts("[3] copiando .user a 0x400000 para B (");
    uart_hex(len as u64);
    uart_puts(" bytes) y resembrando su MARKER='a'...\n");
    // SAFETY: PHYS_B (bloque 2) es RAM libre, mapeada EL1 en la tabla del kernel.
    unsafe {
        let src = us as *const u8;
        let dst = PHYS_B as *mut u8;
        for i in 0..len {
            write_volatile(dst.add(i), read_volatile(src.add(i)));
        }
        // MARKER de B (misma posición relativa, en el bloque de B).
        let marker_off = addr_of!(MARKER) as u64 - PHYS_A;
        write_volatile((PHYS_B + marker_off) as *mut u8, b'a');
        core::arch::asm!("dsb ish; ic iallu; dsb ish; isb");
    }

    uart_puts("[4] hooks (syscall + espacio de direcciones) + spawn A,B\n");
    uart_puts("    esperado: A a B b C c ...  (dos contadores PRIVADOS, aislados)\n\n");
    vectors::set_syscall_hook(syscall);
    vectors::set_addr_space_hook(addr_space);
    vectors::install();

    // SAFETY: arranque single-thread.
    unsafe {
        SCHED = Some(Scheduler::default());
        let s = (*addr_of_mut!(SCHED)).as_mut().unwrap();
        // Tarea A: código en VA 0x200000 (.user), pila en bloque 1.
        let ustack_a =
            core::slice::from_raw_parts_mut(A_STACK_BASE as *mut u8, (A_STACK_TOP - A_STACK_BASE) as usize);
        s.spawn_user(ustack_a, el0_task, Priority::App).ok();
        // Tarea B: código en VA 0x400000 (copia), pila en bloque 2.
        let ustack_b =
            core::slice::from_raw_parts_mut(B_STACK_BASE as *mut u8, (B_STACK_TOP - B_STACK_BASE) as usize);
        let entry_b: fn() -> ! = core::mem::transmute(PHYS_B as usize);
        s.spawn_user(ustack_b, entry_b, Priority::App).ok();
        s.start();
    }
}

extern "C" {
    static __user_start: u8;
    static __user_end: u8;
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    uart_puts("\n!! PANIC\n");
    loop {
        core::hint::spin_loop();
    }
}
