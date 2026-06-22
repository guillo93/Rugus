//! Rugus G5 — **consola `rush` por mini-UART** en la RPi 3B+ (AArch64).
//!
//! Lleva la consola nativa de Rugus —mismo léxico universal y mismo **sistema
//! visual** (`rugus-ui`: paleta semántica, banner, badges, tablas, medidores)—
//! a la segunda arquitectura, dándole paridad de consola con la flota STM32.
//!
//! A diferencia de las consolas STM32 (cuyos verbos leen `rugus-kernel`, atado a
//! Cortex-M), aquí los verbos informativos leen **directamente el
//! `Scheduler<CortexA>` de `rugus-core`** vía su API de introspección pública
//! (`task_count`/`task_state_name`/`stack_high_water`/…). Es la prueba de que el
//! léxico y el visual son arch-agnósticos: la misma consola, otro silicio.
//!
//! Paso 1 (este ejemplo): transporte mini-UART (RX por sondeo + TX) + banner +
//! eco/edición de línea + prompt + IDENTIFY + verbos `cosmos`/`ecosystem`/`coil`/
//! `letargo`. **Ungated** (sin autenticación todavía: llega en el Paso 2).
//!
//! Boot (EL2→EL1 + FP/SIMD + MMU) reutilizado de `rpi3-preempt`. Esquema
//! cooperativo: la tarea de consola y un worker ceden con `yield_now` (sin
//! `wfi`/sleep, que sin IRQ de timer no despertaría).

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use core::arch::global_asm;
use core::panic::PanicInfo;
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};

use rugus_arch_cortex_a::CortexA;
use rugus_core::arch::Arch;
use rugus_core::sched::{Priority, Scheduler};
use rugus_core::syscall::lite::{GpioLevel, Hooks};
use rugus_core::Errno;
use rugus_ui::{Painter, Role};
use rush::{execute, identify, parse, Write};

/// Identidad reportada en IDENTIFY/ENQ y en el prompt.
const TIER: &str = "full";
const CHIP: &str = "rpi3b+";
/// Nombre de placa que muestra `cosmos`.
const BOARD: &str = "Raspberry Pi 3B+ (BCM2837, Cortex-A53)";

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

// Tabla de vectores: captura faults síncronos (consola cooperativa, sin IRQ).
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

// ===================== mini-UART (TX + RX por sondeo) =====================
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
const LSR_RX_READY: u32 = 1 << 0;

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
    mw(AUX_MU_CNTL, 3); // TX + RX habilitados
}
fn uart_send(b: u8) {
    while mr(AUX_MU_LSR) & LSR_TX_EMPTY == 0 {}
    mw(AUX_MU_IO, b as u32);
}
/// Lee un byte del mini-UART si hay alguno disponible (no bloqueante).
fn uart_recv() -> Option<u8> {
    if mr(AUX_MU_LSR) & LSR_RX_READY != 0 {
        Some((mr(AUX_MU_IO) & 0xFF) as u8)
    } else {
        None
    }
}
fn uart_puts(s: &str) {
    for &b in s.as_bytes() {
        if b == b'\n' {
            uart_send(b'\r');
        }
        uart_send(b);
    }
}

/// Sumidero de salida `rush` sobre el mini-UART (TX byte a byte, sin traducción
/// de `\n`: los verbos ya emiten `\r\n`).
struct UartSink;
impl Write for UartSink {
    fn write_str(&mut self, s: &str) -> Result<(), ()> {
        for &b in s.as_bytes() {
            uart_send(b);
        }
        Ok(())
    }
}

// ===================== MMU (idéntica a rpi3-preempt) =====================
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
    uart_puts("\r\n!! FAULT ESR=0x");
    let mut buf = [0u8; 16];
    uart_puts(hex(esr, &mut buf));
    uart_puts(" ELR=0x");
    uart_puts(hex(elr, &mut buf));
    uart_puts("\r\n");
}
fn hex(mut v: u64, buf: &mut [u8; 16]) -> &str {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut i = buf.len();
    if v == 0 {
        i -= 1;
        buf[i] = b'0';
    }
    while v > 0 && i > 0 {
        i -= 1;
        buf[i] = H[(v & 0xF) as usize];
        v >>= 4;
    }
    core::str::from_utf8(&buf[i..]).unwrap_or("?")
}

// ===================== Scheduler + personalidad RPi =====================
const STACK_WORDS: usize = 4096; // 32 KiB por tarea (la consola compone en pila)
#[repr(C, align(16))]
struct Stack([u8; STACK_WORDS]);
static mut STACK_CON: Stack = Stack([0; STACK_WORDS]);
static mut STACK_WRK: Stack = Stack([0; STACK_WORDS]);

/// El scheduler arch-agnóstico de `rugus-core`. Los hooks de los verbos lo leen.
static mut SCHED: Option<Scheduler<CortexA>> = None;

/// Referencia al scheduler para los hooks (single-core, sin reentrada de ISR).
#[inline]
fn sched() -> Option<&'static Scheduler<CortexA>> {
    // SAFETY: instancia única; los hooks corren en la tarea de consola, no en ISR.
    unsafe { (*addr_of!(SCHED)).as_ref() }
}

fn cpu_yield() {
    // SAFETY: scheduler único; cooperativo (single-core, sin reentrada).
    unsafe {
        if let Some(s) = (*addr_of_mut!(SCHED)).as_mut() {
            s.yield_now();
        }
    }
}

/// Color del estado de una tarea (verde sano / ámbar en espera / rojo caído).
fn state_role(name: &str) -> Role {
    match name {
        "run" | "ready" | "Run" | "Ready" => Role::Core,
        "dead" | "killed" | "Dead" | "Killed" => Role::Fault,
        _ => Role::Warn,
    }
}
fn stack_role(pct: u32) -> Role {
    if pct >= 90 {
        Role::Fault
    } else if pct >= 70 {
        Role::Warn
    } else {
        Role::Core
    }
}

/// `cosmos` → identidad de la placa + datos vivos del scheduler de rugus-core.
fn h_sys_info(buf: &mut [u8]) -> usize {
    let mut p = Painter::new(buf);
    p.header("cosmos");
    p.text(Role::Focus, BOARD).raw("  ");
    p.badge(Role::Core, " full ").raw(" ");
    p.badge(Role::Data, " arch:aarch64 ").raw("\r\n");
    let tasks = sched().map(|s| s.task_count() as u32).unwrap_or(0);
    p.kvn("tareas", Role::Data, tasks).raw("   ");
    p.kvn("uptime_ms", Role::Data, CortexA::now_ms()).raw("\r\n");
    p.len()
}

/// `ecosystem` → salud global (en RPi aún sin telemetría de faults: sano).
fn h_sys_status(buf: &mut [u8]) -> usize {
    let mut p = Painter::new(buf);
    p.header("ecosystem");
    p.badge(Role::Core, " sano ").raw("\r\n");
    let tasks = sched().map(|s| s.task_count() as u32).unwrap_or(0);
    p.kvn("tareas", Role::Data, tasks).raw("   ");
    p.kvn("faults", Role::Core, 0).raw("\r\n");
    p.kv("reset", Role::Data, "power-on").raw("\r\n");
    p.len()
}

/// `letargo` → uptime del Generic Timer (sin contabilidad de ocio todavía).
fn h_sys_power(buf: &mut [u8]) -> usize {
    let mut p = Painter::new(buf);
    p.header("letargo");
    p.kvn("uptime", Role::Data, CortexA::now_ms())
        .text(Role::Chrome, " ms\r\n");
    p.text(Role::Chrome, "idle: n/d (cooperativo, sin tick de ocio)\r\n");
    p.len()
}

/// `coil` → tabla de tareas del scheduler con medidor de pila por fila.
fn h_task_list(out: &mut [u8]) -> i32 {
    let mut p = Painter::new(out);
    p.header("coil");
    p.text(Role::Chrome, "  # pri  modo  estado    pila\r\n");
    if let Some(s) = sched() {
        for idx in 0..s.task_count() {
            let used = s.stack_high_water(idx);
            let total = s.stack_len(idx).max(1);
            let pct = (used * 100 / total).min(100);
            p.raw("  ").num(Role::Data, idx as u32).raw("  ");
            p.num(Role::Data, s.task_priority(idx) as u32).raw("  ");
            if s.is_user_task(idx) {
                p.text(Role::Text, "user");
            } else {
                p.text(Role::Focus, "kern");
            }
            p.raw("  ");
            let st = s.task_state_name(idx);
            p.on(state_role(st)).raw(st).off();
            for _ in st.len()..9 {
                p.raw(" ");
            }
            p.meter(pct, 8)
                .raw(" ")
                .num(stack_role(pct), pct)
                .text(Role::Chrome, "%\r\n");
        }
    }
    p.len() as i32
}

// Stubs honestos: servicios sin respaldo en esta capa RPi (retornan Enosys).
fn s_rw2(_a: u8, _b: u8) -> i32 {
    Errno::Enosys as i32
}
fn s_gpio_write(_p: u8, _n: u8, _l: GpioLevel) -> i32 {
    Errno::Enosys as i32
}
fn s_gpio_bind(_p: u8, _n: u8, _r: &[u8]) -> i32 {
    Errno::Enosys as i32
}
fn s_bus_scan(_b: u8, _o: &mut [u8]) -> i32 {
    Errno::Enosys as i32
}
fn s_kv_out(_k: &[u8], _o: &mut [u8]) -> i32 {
    Errno::Enosys as i32
}
fn s_kv(_k: &[u8], _v: &[u8]) -> i32 {
    Errno::Enosys as i32
}
fn s_unit() -> i32 {
    Errno::Enosys as i32
}
fn s_out(_o: &mut [u8]) -> i32 {
    Errno::Enosys as i32
}
fn s_slot_out(_s: u8, _o: &mut [u8]) -> i32 {
    Errno::Enosys as i32
}
fn s_name(_n: &[u8]) -> i32 {
    Errno::Enosys as i32
}
fn s_action(_a: u8) -> i32 {
    Errno::Enosys as i32
}
fn s_scar(_a: u8, _o: &mut [u8]) -> i32 {
    Errno::Enosys as i32
}

fn rpi_hooks() -> Hooks {
    Hooks {
        sys_info: h_sys_info,
        sys_status: h_sys_status,
        sys_power: h_sys_power,
        gpio_read: s_rw2,
        gpio_write: s_gpio_write,
        gpio_toggle: s_rw2,
        gpio_bind: s_gpio_bind,
        bus_scan: s_bus_scan,
        config_get: s_kv_out,
        config_set: s_kv,
        config_commit: s_unit,
        module_list: s_out,
        module_read: s_slot_out,
        task_list: h_task_list,
        app_reload: s_name,
        sys_failsafe: s_action,
        wdt: s_action,
        module_renew: s_unit,
        scar: s_scar,
        sting: s_unit,
    }
}

// ===================== Tareas =====================
static mut LINE: [u8; 128] = [0; 128];
static mut LINE_LEN: usize = 0;

/// Tarea de consola: banner + prompt + bucle de edición de línea sobre el
/// mini-UART, cediendo el CPU entre sondeos. Ungated (Paso 1): `execute` directo.
fn console_task() -> ! {
    let mut sink = UartSink;
    rush::banner::write_banner(&mut sink, true);
    let _ = sink.write_str("Consola rush en RPi 3B+ (AArch64). Sin gate todavia.\r\n\r\n");
    rush::paint::prompt(&mut sink, CHIP);
    loop {
        while let Some(b) = uart_recv() {
            cli_byte(&mut sink, b);
        }
        cpu_yield();
    }
}

fn cli_byte(sink: &mut UartSink, b: u8) {
    if b == identify::ENQ {
        identify::write_signature(sink, TIER, CHIP);
        return;
    }
    // SAFETY: solo la tarea de consola edita la línea.
    unsafe {
        if b == b'\r' || b == b'\n' {
            if LINE_LEN > 0 {
                let _ = sink.write_str("\r\n");
                let line = core::str::from_utf8(&LINE[..LINE_LEN]).unwrap_or("");
                execute(parse(line), line, sink);
                LINE_LEN = 0;
            } else {
                let _ = sink.write_str("\r\n");
            }
            rush::paint::prompt(sink, CHIP);
        } else if b == 0x7F || b == 0x08 {
            if LINE_LEN > 0 {
                LINE_LEN -= 1;
                let _ = sink.write_str("\x08 \x08");
            }
        } else if LINE_LEN < LINE.len() {
            LINE[LINE_LEN] = b;
            LINE_LEN += 1;
            let ch = [b];
            if let Ok(s) = core::str::from_utf8(&ch) {
                let _ = sink.write_str(s);
            }
        }
    }
}

/// Worker de fondo: solo existe para que `coil` muestre más de una tarea. Cuenta
/// y cede; sin `sleep` (no hay IRQ de timer que despierte de un `wfi`).
fn worker_task() -> ! {
    let mut _n: u64 = 0;
    loop {
        _n = _n.wrapping_add(1);
        // Delay corto: en esquema cooperativo, la consola solo sondea el RX
        // cuando corre. El FIFO del mini-UART es de 8 bytes, así que un worker
        // que acapare el CPU lo desbordaría en ráfagas largas. Mantener la
        // ventana de no-sondeo pequeña hasta tener RX por IRQ + ring (futuro).
        delay(150_000);
        cpu_yield();
    }
}

#[no_mangle]
extern "C" fn kernel_main() -> ! {
    uart_init();
    uart_puts("\r\n[boot] RUGUS @ RPi 3B+ — consola rush (AArch64)\r\n");
    uart_puts("[boot] MMU...\r\n");
    unsafe { mmu_init() };
    uart_puts("[boot] MMU ON\r\n");

    uart_puts("[boot] registrando personalidad RPi (verbos sobre rugus-core)...\r\n");
    // SAFETY: registro único en arranque single-thread, antes de spawn/start.
    unsafe { rugus_core::syscall::lite::register(rpi_hooks()) };

    uart_puts("[boot] scheduler: consola + worker; arrancando...\r\n");
    // SAFETY: arranque single-thread; pilas estáticas vivas para todo el kernel.
    unsafe {
        SCHED = Some(Scheduler::default());
        let s = (*addr_of_mut!(SCHED)).as_mut().unwrap();
        s.spawn(&mut (*addr_of_mut!(STACK_CON)).0, console_task, Priority::Kernel)
            .ok();
        s.spawn(&mut (*addr_of_mut!(STACK_WRK)).0, worker_task, Priority::App)
            .ok();
        s.start();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    uart_puts("\r\n!! PANIC\r\n");
    loop {
        core::hint::spin_loop();
    }
}
