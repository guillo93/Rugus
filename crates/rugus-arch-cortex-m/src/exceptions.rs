//! Exception vectors — MemoryManagement / BusFault / UsageFault / HardFault.

use cortex_m::peripheral::scb::Exception;
use cortex_m_rt::{exception, ExceptionFrame};
use rugus_core::fault::FaultKind;

use crate::fault::{fault_from_thread, handle_task_fault};

#[exception]
unsafe fn MemoryManagement() {
    if fault_from_thread() {
        handle_task_fault(FaultKind::MemManage);
    }
    kernel_fault_panic("MemoryManagement");
}

#[exception]
unsafe fn BusFault() {
    if fault_from_thread() {
        handle_task_fault(FaultKind::BusFault);
    }
    kernel_fault_panic("BusFault");
}

#[exception]
unsafe fn UsageFault() {
    if fault_from_thread() {
        handle_task_fault(FaultKind::UsageFault);
    }
    kernel_fault_panic("UsageFault");
}

#[exception]
unsafe fn HardFault(_ef: &ExceptionFrame) -> ! {
    if fault_from_thread() {
        handle_task_fault(FaultKind::HardFault);
    }
    kernel_fault_panic("HardFault");
}

fn kernel_fault_panic(label: &str) -> ! {
    #[cfg(feature = "defmt")]
    defmt::panic!("{} in handler mode", label);
    #[cfg(not(feature = "defmt"))]
    {
        let _ = label;
        core::panic!("kernel fault");
    }
}

/// Habilita MemManage / BusFault / UsageFault como handlers dedicados.
pub fn enable_fault_handlers(scb: &mut cortex_m::peripheral::SCB) {
    scb.enable(Exception::MemoryManagement);
    scb.enable(Exception::BusFault);
    scb.enable(Exception::UsageFault);
}
