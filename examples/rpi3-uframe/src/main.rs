//! Rugus G6.2-proto — **frame de excepción unificado EL0/EL1** en RPi 3B+.
//!
//! Prototipo aislado del §4 de `docs/AARCH64-USERLAND-DESIGN.md`: de-risquea la
//! integración de userland en el `Scheduler<CortexA>` antes de tocar
//! `rugus-core`. Demuestra que un **único formato de frame** y un **único
//! epílogo `restore_eret`** sirven para conmutar entre una tarea EL1 (kernel) y
//! una tarea EL0 (userland), reanudando **siempre con `eret`**.
//!
//! Qué prueba, por mini-UART:
//! - Mini-scheduler round-robin de 2 tareas: `[0]` supervisor EL1, `[1]` user EL0.
//! - Toda conmutación pasa por una **trampa síncrona** (`SVC`): el supervisor
//!   cede con `svc` (yield), la tarea EL0 hace syscalls (`putchar`, `yield`).
//! - El handler salva el **frame uniforme** (x0–x30 + SP_EL0 + ELR_EL1 +
//!   SPSR_EL1), elige la siguiente tarea y restaura su frame con `eret` — el
//!   `SPSR` lleva a EL1h o EL0t según corresponda.
//! - La tarea EL0, tras 3 syscalls, lee memoria del kernel → **fault contenido**.
//!
//! Si esto funciona, el modelo del diseño es correcto y G6.2 puede integrarlo en
//! el backend `rugus-arch-cortex-a`.

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

// ===================== Frame uniforme + epílogo compartido =====================
// Frame de 288 B: x0..x30 [0..248], SP_EL0 [248], ELR_EL1 [256], SPSR_EL1 [264].
global_asm!(
    r#"
.macro SAVE_FRAME
    sub     sp, sp, #288
    stp     x0,  x1,  [sp, #0]
    stp     x2,  x3,  [sp, #16]
    stp     x4,  x5,  [sp, #32]
    stp     x6,  x7,  [sp, #48]
    stp     x8,  x9,  [sp, #64]
    stp     x10, x11, [sp, #80]
    stp     x12, x13, [sp, #96]
    stp     x14, x15, [sp, #112]
    stp     x16, x17, [sp, #128]
    stp     x18, x19, [sp, #144]
    stp     x20, x21, [sp, #160]
    stp     x22, x23, [sp, #176]
    stp     x24, x25, [sp, #192]
    stp     x26, x27, [sp, #208]
    stp     x28, x29, [sp, #224]
    str     x30,      [sp, #240]
    mrs     x9,  sp_el0
    str     x9,  [sp, #248]
    mrs     x9,  elr_el1
    str     x9,  [sp, #256]
    mrs     x9,  spsr_el1
    str     x9,  [sp, #264]
.endm

.align 11
.global vector_table
vector_table:
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    // Current EL SPx: Sync (SVC del supervisor EL1 / fault de kernel) e IRQ.
    .align 7
    b       el1_sync
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    .align 7
    b       hang_exc
    // Lower EL AArch64: Sync (syscall o abort de EL0).
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

el1_sync:
    SAVE_FRAME
    mov     x12, #0                  // origen = EL1
    b       sync_dispatch
el0_sync:
    SAVE_FRAME
    mov     x12, #1                  // origen = EL0
    b       sync_dispatch

// Despacho común: SVC → syscall; resto → fault. (sp = puntero al frame)
sync_dispatch:
    mrs     x10, esr_el1
    lsr     x11, x10, #26            // EC
    cmp     x11, #0x15               // SVC64
    b.ne    fault_path
    ldr     x8,  [sp, #64]           // nº de syscall (x8 guardado en el frame)
    cmp     x8,  #1                  // PUTCHAR
    b.eq    do_putchar
    cmp     x8,  #2                  // YIELD
    b.eq    do_yield
    b       restore_eret            // syscall desconocida: vuelve

do_putchar:
    ldr     x0,  [sp, #0]            // arg (carácter) guardado en el frame
    bl      rust_putchar
    b       restore_eret

do_yield:
    mov     x0,  sp                  // frame actual
    bl      rust_yield_next          // devuelve el frame de la siguiente tarea
    mov     sp,  x0
    b       restore_eret

fault_path:
    mov     x0,  x12                 // origen (0=EL1, 1=EL0)
    mrs     x1,  esr_el1
    mrs     x2,  far_el1
    mrs     x3,  elr_el1
    bl      rust_fault
1:  wfe
    b       1b

// Arranca la primera tarea: SP = su frame, restaura y eret. (x0 = frame ptr)
.global start_first
start_first:
    mov     sp, x0
restore_eret:
    ldr     x9,  [sp, #264]
    msr     spsr_el1, x9
    ldr     x9,  [sp, #256]
    msr     elr_el1,  x9
    ldr     x9,  [sp, #248]
    msr     sp_el0,   x9
    ldp     x0,  x1,  [sp, #0]
    ldp     x2,  x3,  [sp, #16]
    ldp     x4,  x5,  [sp, #32]
    ldp     x6,  x7,  [sp, #48]
    ldp     x8,  x9,  [sp, #64]
    ldp     x10, x11, [sp, #80]
    ldp     x12, x13, [sp, #96]
    ldp     x14, x15, [sp, #112]
    ldp     x16, x17, [sp, #128]
    ldp     x18, x19, [sp, #144]
    ldp     x20, x21, [sp, #160]
    ldp     x22, x23, [sp, #176]
    ldp     x24, x25, [sp, #192]
    ldp     x26, x27, [sp, #208]
    ldp     x28, x29, [sp, #224]
    ldr     x30,      [sp, #240]
    add     sp, sp, #288
    eret

hang_exc:
    bl      rust_unexpected
1:  wfe
    b       1b

// --- Rutina userland EL0 (se copia al bloque user 0x200000 y se ejecuta en EL0) ---
// Position-independent. Hace 3 ciclos putchar+yield y luego un acceso prohibido.
.global user_stub_start
user_stub_start:
    mov     x19, #0
1:
    mov     x8,  #1                  // SYS_PUTCHAR
    mov     x0,  #0x55               // 'U'
    svc     #0
    add     x19, x19, #1
    cmp     x19, #3
    b.ge    3f
    mov     x8,  #2                  // SYS_YIELD
    svc     #0
    b       1b
3:
    movz    x1,  #0x8, lsl #16       // x1 = 0x80000 (kernel, EL1-only)
    ldr     x2,  [x1]                // LECTURA prohibida desde EL0 → abort
    b       .
.global user_stub_end
user_stub_end:
"#
);

extern "C" {
    fn start_first(frame: u64) -> !;
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
const USER_BASE: u64 = (USER_BLOCK_IDX as u64) << 21; // 0x200000
const USER_STACK_TOP: u64 = USER_BASE + 0x10_0000; // 0x300000

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

// ===================== Mini-scheduler (frame uniforme) =====================
const NTASKS: usize = 2;
const FRAME: u64 = 288;
const KSTACK_WORDS: usize = 1024; // 8 KiB de pila kernel por tarea
#[repr(C, align(16))]
struct KStack([u8; KSTACK_WORDS]);
static mut KSTACK0: KStack = KStack([0; KSTACK_WORDS]);
static mut KSTACK1: KStack = KStack([0; KSTACK_WORDS]);

/// Bloque de control: puntero al frame guardado de cada tarea + tarea actual.
static mut TCB: [u64; NTASKS] = [0; NTASKS];
static mut CURRENT: usize = 0;

/// Construye el frame inicial de una tarea en el tope de su pila kernel.
/// `spsr`: 0x5 = EL1h (kernel), 0x0 = EL0t (userland). `sp_el0` solo en EL0.
unsafe fn init_frame(kstack_top: u64, entry: u64, spsr: u64, sp_el0: u64) -> u64 {
    let sp = kstack_top - FRAME;
    let f = sp as *mut u64;
    unsafe {
        for i in 0..(FRAME as usize / 8) {
            write_volatile(f.add(i), 0);
        }
        write_volatile(f.add(248 / 8), sp_el0); // SP_EL0
        write_volatile(f.add(256 / 8), entry); // ELR_EL1
        write_volatile(f.add(264 / 8), spsr); // SPSR_EL1
    }
    sp
}

/// Syscall del handler: imprime un carácter (no conmuta).
#[no_mangle]
extern "C" fn rust_putchar(c: u64) {
    uart_send((c & 0xFF) as u8);
}

/// Syscall YIELD: guarda el frame actual, avanza round-robin y devuelve el de la
/// siguiente tarea (el handler hace `mov sp, x0; eret`).
#[no_mangle]
extern "C" fn rust_yield_next(cur_frame: u64) -> u64 {
    // SAFETY: scheduler single-core; solo se toca desde el handler de excepción.
    unsafe {
        let cur = read_volatile(addr_of!(CURRENT));
        write_volatile((addr_of_mut!(TCB) as *mut u64).add(cur), cur_frame);
        let next = (cur + 1) % NTASKS;
        write_volatile(addr_of_mut!(CURRENT), next);
        read_volatile((addr_of!(TCB) as *const u64).add(next))
    }
}

#[no_mangle]
extern "C" fn rust_fault(origin: u64, esr: u64, far: u64, elr: u64) {
    let ec = (esr >> 26) & 0x3F;
    if origin == 1 {
        uart_puts("\n\n[EL1] *** abort de EL0 CONTENIDO (frame uniforme OK) ***\n");
    } else {
        uart_puts("\n\n!! FAULT de KERNEL (EL1)\n");
    }
    uart_puts("  EC=");
    uart_hex(ec);
    uart_puts("  ESR=");
    uart_hex(esr);
    uart_puts("\n  FAR=");
    uart_hex(far);
    uart_puts("  ELR=");
    uart_hex(elr);
    uart_puts("\n[OK] conmutamos EL1<->EL0 con un solo frame y eret; el fault de EL0 quedo contenido.\n");
}

#[no_mangle]
extern "C" fn rust_unexpected() {
    uart_puts("\n!! excepcion inesperada\n");
}

// ===================== Tareas =====================
/// Tarea supervisora (EL1): imprime su marca y cede con un syscall YIELD.
extern "C" fn supervisor() -> ! {
    loop {
        uart_puts("[sup] ");
        // SAFETY: SVC con x8=YIELD; el handler conmuta y vuelve preservando todo.
        unsafe { core::arch::asm!("mov x8, #2", "svc #0", out("x8") _) };
    }
}

#[no_mangle]
extern "C" fn kernel_main() -> ! {
    uart_init();
    uart_puts("\n=== RUGUS @ RPi 3B+ — G6.2-proto: frame unificado EL0/EL1 (AArch64) ===\n");
    uart_puts("[1] EL1 + FP/SIMD + VBAR ok\n");
    uart_puts("[2] MMU con bits AP (kernel EL1-only / user EL0 en 0x200000)...\n");
    unsafe { mmu_init() };
    uart_puts("[2] MMU ON\n");

    // Copia la rutina userland al bloque EL0.
    let src = addr_of!(user_stub_start);
    let len = addr_of!(user_stub_end) as usize - src as usize;
    uart_puts("[3] copiando rutina userland a 0x200000 (");
    uart_hex(len as u64);
    uart_puts(" bytes)...\n");
    // SAFETY: destino en el bloque user (RAM libre, EL0+EL1 RW).
    unsafe {
        let dst = USER_BASE as *mut u8;
        for i in 0..len {
            write_volatile(dst.add(i), read_volatile(src.add(i)));
        }
        core::arch::asm!("dsb ish; ic iallu; dsb ish; isb");
    }

    // Inicializa los frames de las dos tareas.
    // SAFETY: arranque single-thread; pilas estáticas vivas para todo el kernel.
    let frame0 = unsafe {
        let top = addr_of!(KSTACK0) as u64 + KSTACK_WORDS as u64;
        init_frame(top, supervisor as *const () as u64, 0x5, 0) // EL1h
    };
    let frame1 = unsafe {
        let top = addr_of!(KSTACK1) as u64 + KSTACK_WORDS as u64;
        init_frame(top, USER_BASE, 0x0, USER_STACK_TOP) // EL0t
    };
    unsafe {
        write_volatile(addr_of_mut!(TCB) as *mut u64, frame0);
        write_volatile((addr_of_mut!(TCB) as *mut u64).add(1), frame1);
        write_volatile(addr_of_mut!(CURRENT), 0);
    }

    uart_puts("[4] arrancando: [0]=supervisor EL1, [1]=user EL0 (round-robin por SVC)\n");
    uart_puts("    esperado: [sup] U [sup] U [sup] U  y luego abort de EL0 contenido\n\n");
    // SAFETY: frame0 válido; entra en la tarea 0 con eret.
    unsafe { start_first(frame0) }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    uart_puts("\n!! PANIC\n");
    loop {
        core::hint::spin_loop();
    }
}
