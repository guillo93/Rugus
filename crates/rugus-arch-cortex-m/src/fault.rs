//! Handlers HardFault / MemManage / BusFault / UsageFault — report domain + PC.

use core::ptr;
use rugus_core::domain::Domain;
use rugus_core::fault::{FaultKind, FaultReport};

use crate::mpu;

/// Callback registrado por el kernel — mata la tarea y continúa (no panic global).
static mut FAULT_HOOK: Option<fn(FaultReport) -> !> = None;

/// Registra el hook de faults del kernel. Llamar una vez antes de `sched.start()`.
///
/// # Safety
///
/// Debe invocarse desde main antes de arrancar tareas; no concurrente.
pub unsafe fn set_fault_hook(hook: fn(FaultReport) -> !) {
    unsafe {
        FAULT_HOOK = Some(hook);
    }
}

/// Extrae PC del frame en PSP (tarea userland / thread mode).
#[inline]
pub fn stacked_pc(psp: u32) -> u32 {
    // Frame estándar sin FPU: r0,r1,r2,r3,r12,lr,pc,xpsr → PC en offset 24.
    // SAFETY: psp apunta al frame válido del exception entry.
    unsafe { ptr::read_volatile((psp as *const u32).add(6)) }
}

/// Dominio lógico inferido del PC (reservado para diagnóstico futuro).
#[allow(dead_code)]
pub fn domain_for_pc(pc: u32) -> Domain {
    if (mpu::layout::PERIPH_BASE..0x6000_0000).contains(&pc) {
        Domain::Drivers
    } else if (mpu::layout::SDRAM_BASE..0xC100_0000).contains(&pc) {
        Domain::Services
    } else if (mpu::layout::FLASH_BASE..0x0820_0000).contains(&pc) {
        Domain::App
    } else {
        Domain::Kernel
    }
}

/// Handler común para MemManage / Bus / Usage / HardFault en contexto de tarea.
pub fn handle_task_fault(kind: FaultKind) -> ! {
    // El registro de dirección (MMFAR/BFAR) solo es fiable si su bit de validez
    // sigue en alto, así que se lee ANTES de limpiar el estado de fault.
    let addr = fault_address(kind);
    clear_fault_status(kind);
    let psp = cortex_m::register::psp::read();
    let pc = stacked_pc(psp);
    let domain = rugus_core::syscall::current_domain();
    let report = FaultReport {
        kind,
        pc,
        domain,
        task_id: rugus_core::syscall::current_task_id(),
        addr,
    };

    // SAFETY: hook registrado en main; no reentrante desde otra tarea.
    unsafe {
        match FAULT_HOOK {
            Some(hook) => hook(report),
            None => fault_panic(kind, domain, pc),
        }
    }
}

fn fault_panic(kind: FaultKind, domain: Domain, pc: u32) -> ! {
    #[cfg(feature = "defmt")]
    defmt::panic!(
        "fault {} domain={} pc={=u32} (no hook)",
        kind.name(),
        domain.name(),
        pc
    );
    #[cfg(not(feature = "defmt"))]
    {
        let _ = (kind, domain, pc);
        core::panic!("rugus fault");
    }
}

/// Lee la dirección del fault desde MMFAR/BFAR cuando el bit de validez en CFSR
/// está activo. Devuelve `None` para faults sin dirección asociada
/// (UsageFault, HardFault sin escalado de MemManage/BusFault) o si el HW no
/// marcó la dirección como válida.
fn fault_address(kind: FaultKind) -> Option<u32> {
    const CFSR: *const u32 = 0xE000_ED28 as *const u32;
    const MMFAR: *const u32 = 0xE000_ED34 as *const u32;
    const BFAR: *const u32 = 0xE000_ED38 as *const u32;
    const MMARVALID: u32 = 1 << 7;
    const BFARVALID: u32 = 1 << 15;
    // SAFETY: registros SCB estándar de solo lectura.
    unsafe {
        let cfsr = ptr::read_volatile(CFSR);
        match kind {
            FaultKind::MemManage if cfsr & MMARVALID != 0 => Some(ptr::read_volatile(MMFAR)),
            FaultKind::BusFault if cfsr & BFARVALID != 0 => Some(ptr::read_volatile(BFAR)),
            _ => None,
        }
    }
}

fn clear_fault_status(kind: FaultKind) {
    const CFSR: *mut u32 = 0xE000_ED28 as *mut u32;
    const HFSR: *mut u32 = 0xE000_ED2C as *mut u32;
    // SAFETY: registros SCB estándar.
    unsafe {
        match kind {
            FaultKind::MemManage => {
                ptr::write_volatile(CFSR, ptr::read_volatile(CFSR) & 0xFF);
            }
            FaultKind::BusFault => {
                ptr::write_volatile(CFSR, ptr::read_volatile(CFSR) & 0xFF00);
            }
            FaultKind::UsageFault => {
                ptr::write_volatile(CFSR, ptr::read_volatile(CFSR) & 0xFFFF_0000);
            }
            FaultKind::HardFault => {
                ptr::write_volatile(CFSR, 0xFFFF_FFFF);
                ptr::write_volatile(HFSR, ptr::read_volatile(HFSR) | (1 << 1));
            }
        }
    }
}

/// True si el fault ocurrió en thread mode con PSP (tarea, no kernel handler).
pub fn fault_from_thread() -> bool {
    let exc_return = cortex_m::register::lr::read() as u32;
    // EXC_RETURN bits [3:0]: 0xD/0x9/0x1 → return to Thread mode using PSP.
    matches!(exc_return & 0xF, 0xD | 0x9 | 0x1)
}
