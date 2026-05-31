//! Rugus app-sandbox — G3: MPU + syscalls + fault kill en STM32F407G-DISC1.
//!
//! - Tarea kernel (priv): parpadea LD4, supervisa el sistema.
//! - App A (user): parpadea LD6 vía SVC yield.
//! - App B (user): tras unos ciclos accede a periféricos → MemManage; kernel mata la tarea.
//!
//! Sin SDRAM en F407: heap en SRAM interna. MPU reutiliza `rugus-arch-cortex-m`.

#![no_std]
#![no_main]

use rugus_arch_cortex_m::{platform_init, set_fault_hook, CortexM, MpuLayout};
use rugus_core::fault::FaultReport;
use rugus_core::sched::{Priority, Scheduler};
use rugus_core::syscall::{self, user as svc_user, Hooks};
use rugus_hal_stm32f4::gpio::{DiscoLed, LedPin};
use rugus_hal_stm32f4::pac;
use rugus_hal_stm32f4::rcc;
use rugus_runtime::entry;

type Sched = Scheduler<CortexM>;

#[repr(C, align(4096))]
struct Stack4k([u8; 4096]);

static mut SCHEDULER: Sched = Sched::new();
static mut STACK_KERNEL: Stack4k = Stack4k([0; 4096]);
static mut STACK_GOOD: Stack4k = Stack4k([0; 4096]);
static mut STACK_BAD: Stack4k = Stack4k([0; 4096]);

const TICKS_HALF_SEC: u32 = 168_000_000 / 2;

fn kernel_task() -> ! {
    defmt::info!("kernel task (LD4) started");
    loop {
        toggle_led(DiscoLed::Green);
        defmt::debug!("kernel toggle LD4");
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
        "rugus app-sandbox @ STM32F407G-DISC1, SYSCLK {} MHz, ABI {=u16}",
        clocks.sysclk_mhz(),
        rugus_core::syscall::ABI_VERSION
    );

    static mut HEAP: [u8; 32 * 1024] = [0; 32 * 1024];
    const HEAP_SIZE: usize = 32 * 1024;
    unsafe {
        rugus_core::heap::init(core::ptr::addr_of_mut!(HEAP).cast(), HEAP_SIZE);
    }
    defmt::info!(
        "heap on internal SRAM ({=u32} KiB)",
        HEAP_SIZE as u32 / 1024
    );

    platform_init(&mut cp, &MpuLayout::STM32F407);

    let _ = LedPin::new(&dp.RCC, DiscoLed::Green);
    let _ = LedPin::new(&dp.RCC, DiscoLed::Blue);

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
        // `bad_app` se lanza antes que `good_app`: el round-robin cooperativo
        // sirve a las tareas App en orden de spawn, así `bad_app` alcanza su
        // acceso ilegal, el kernel la mata, y `good_app` (+ kernel) sobreviven.
        sched
            .spawn_user(
                &mut (*core::ptr::addr_of_mut!(STACK_BAD)).0,
                bad_app,
                Priority::App,
            )
            .expect("spawn bad app");
        sched
            .spawn_user(
                &mut (*core::ptr::addr_of_mut!(STACK_GOOD)).0,
                good_app,
                Priority::App,
            )
            .expect("spawn good app");

        defmt::info!("scheduler: 3 tasks (1 kernel + 2 userland), starting");
        sched.start();
    }
}

fn on_fault(report: FaultReport) -> ! {
    // Traza del MPU en acción: confirma que el acceso ilegal de `bad_app` al
    // dominio Drivers disparó un MemManage en user mode y que el kernel lo
    // contiene matando SOLO esa tarea (LD4 sigue parpadeando).
    defmt::error!(
        "MPU fault {} domain={} pc={=u32:#x} task={=u8} -> kill+resume",
        report.kind.name(),
        report.domain.name(),
        report.pc,
        report.task_id.0
    );
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
        let g = &*pac::GPIOD::ptr();
        let bit = match led {
            DiscoLed::Green => 1 << 12,
            DiscoLed::Blue => 1 << 15,
            _ => return,
        };
        g.odr.modify(|r, w| w.bits(r.bits() ^ bit));
    }
}
