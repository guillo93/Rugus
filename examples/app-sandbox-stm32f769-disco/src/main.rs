//! Rugus app-sandbox — G2: MPU + syscalls + fault kill en STM32F769I-DISCO.
//!
//! - Tarea kernel (priv): parpadea LD1, supervisa el sistema.
//! - App A (user): parpadea LD2 vía SVC yield.
//! - App B (user): tras unos ciclos accede a periféricos → MemManage; kernel mata la tarea.

#![no_std]
#![no_main]

use rugus_arch_cortex_m::{platform_init, set_fault_hook, CortexM};
use rugus_core::fault::FaultReport;
use rugus_core::sched::{Priority, Scheduler};
use rugus_core::syscall::{self, user as svc_user, Hooks};
use rugus_hal_stm32f7::cache;
use rugus_hal_stm32f7::fmc::{self, SDRAM_BASE};
use rugus_hal_stm32f7::gpio::{DiscoLed, LedPin};
use rugus_hal_stm32f7::pac;
use rugus_hal_stm32f7::rcc;
use rugus_runtime::entry;

type Sched = Scheduler<CortexM>;

#[repr(C, align(4096))]
struct Stack4k([u8; 4096]);

static mut SCHEDULER: Sched = Sched::new();
static mut STACK_KERNEL: Stack4k = Stack4k([0; 4096]);
static mut STACK_GOOD: Stack4k = Stack4k([0; 4096]);
static mut STACK_BAD: Stack4k = Stack4k([0; 4096]);

const TICKS_HALF_SEC: u32 = 216_000_000 / 2;

fn kernel_task() -> ! {
    defmt::info!("kernel task (LD1) started");
    loop {
        toggle_led(DiscoLed::Red);
        defmt::debug!("kernel toggle LD1");
        delay(TICKS_HALF_SEC);
        yield_cpu();
    }
}

fn good_app() -> ! {
    loop {
        spin_delay();
        let _ = svc_user::yield_now();
    }
}

fn bad_app() -> ! {
    let mut rounds = 0u32;
    loop {
        rounds += 1;
        let _ = svc_user::yield_now();
        if rounds >= 3 {
            // Acceso prohibido a dominio Drivers — MemManage en user mode.
            unsafe {
                let _ = core::ptr::read_volatile(0x4000_0000 as *const u32);
            }
        }
        spin_delay();
    }
}

#[entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("cortex-m peripherals");
    let dp = pac::Peripherals::take().expect("device peripherals");

    let clocks = rcc::init(&dp);
    rugus_runtime::enable_cycle_counter(&mut cp);

    defmt::info!(
        "rugus app-sandbox @ STM32F769I-DISCO, SYSCLK {} MHz, ABI {=u16}",
        clocks.sysclk_mhz(),
        rugus_core::syscall::ABI_VERSION
    );

    static mut HEAP_FALLBACK: [u8; 64 * 1024] = [0; 64 * 1024];
    match fmc::init(&dp, &mut cp.SCB, &mut cp.CPUID) {
        Ok(()) => {
            defmt::info!("SDRAM OK @ {=u32}", SDRAM_BASE as u32);
            const HEAP_SIZE: usize = 256 * 1024;
            unsafe {
                rugus_core::heap::init(SDRAM_BASE as *mut u8, HEAP_SIZE);
            }
        }
        Err(_e) => {
            defmt::warn!("SDRAM init failed; heap fallback");
            unsafe {
                rugus_core::heap::init(core::ptr::addr_of_mut!(HEAP_FALLBACK).cast(), 64 * 1024);
            }
        }
    }
    cache::enable(&mut cp.SCB, &mut cp.CPUID);

    platform_init(&mut cp);

    let _ = LedPin::new(&dp.RCC, DiscoLed::Red);
    let _ = LedPin::new(&dp.RCC, DiscoLed::Green);

    unsafe {
        set_fault_hook(on_fault);
        syscall::register(Hooks {
            yield_now: yield_cpu,
            current_task_id,
            current_domain,
        });

        let sched = &mut *core::ptr::addr_of_mut!(SCHEDULER);
        sched
            .spawn(
                &mut (*core::ptr::addr_of_mut!(STACK_KERNEL)).0,
                kernel_task,
                Priority::Kernel,
            )
            .expect("spawn kernel");
        sched
            .spawn_user(
                &mut (*core::ptr::addr_of_mut!(STACK_GOOD)).0,
                good_app,
                Priority::App,
            )
            .expect("spawn good app");
        sched
            .spawn_user(
                &mut (*core::ptr::addr_of_mut!(STACK_BAD)).0,
                bad_app,
                Priority::App,
            )
            .expect("spawn bad app");

        defmt::info!("scheduler: 3 tasks (1 kernel + 2 userland), starting");
        sched.start();
    }
}

fn on_fault(report: FaultReport) -> ! {
    // SAFETY: scheduler activo; hook solo desde fault handler.
    unsafe {
        (&mut *core::ptr::addr_of_mut!(SCHEDULER)).kill_current_and_resume(report);
    }
}

fn yield_cpu() {
    unsafe {
        (&mut *core::ptr::addr_of_mut!(SCHEDULER)).yield_now();
    }
}

fn current_task_id() -> rugus_core::sched::TaskId {
    unsafe { (*core::ptr::addr_of!(SCHEDULER)).current_id() }
}

fn current_domain() -> rugus_core::Domain {
    unsafe { (*core::ptr::addr_of!(SCHEDULER)).current_domain() }
}

fn delay(ticks: u32) {
    cortex_m::asm::delay(ticks);
}

fn spin_delay() {
    for _ in 0..500_000 {
        core::hint::spin_loop();
    }
}

fn toggle_led(led: DiscoLed) {
    unsafe {
        match led {
            DiscoLed::Red => {
                let g = &*pac::GPIOJ::ptr();
                g.odr.modify(|r, w| w.bits(r.bits() ^ (1 << 13)));
            }
            DiscoLed::Green => {
                let g = &*pac::GPIOJ::ptr();
                g.odr.modify(|r, w| w.bits(r.bits() ^ (1 << 5)));
            }
            _ => {}
        }
    }
}
