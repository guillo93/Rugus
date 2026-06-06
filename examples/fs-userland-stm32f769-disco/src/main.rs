//! Rugus F5.C.3 — API de ficheros userland por syscall + IPC bajo MPU sobre
//! `rugus-fs` en la QSPI NOR (Macronix MX25L51245G) de la STM32F769I-DISCO.
//!
//! Cierra la línea F5.C exponiendo el almacén persistente a una tarea
//! **userland** (nPRIV, dominio App, sandboxeada por la MPU) con el mismo diseño
//! **híbrido** que la red (F5.B.2):
//!
//! - **Plano de control + orden de E/S** → syscalls finas `fs_open`/`fs_read`/
//!   `fs_write`/`fs_close` (SVC 0x50..0x53). El dispatch del kernel las rutea a
//!   los `FsHooks` que registra esta placa; el *servicio de ficheros* (el código
//!   privilegiado que posee la `Rufs` sobre la flash) es el ÚNICO que toca la FS.
//! - **Plano de datos** (el contenido del fichero) → un **pool de buffers
//!   compartido** mapeado App-RW por la región MPU 5 (`SERVICES`). La app escribe
//!   el payload en un slot del pool y pasa su índice por valor; el hook (en
//!   contexto de syscall, privilegiado) lee el slot y lo persiste. Ningún puntero
//!   cruza la frontera de confianza: todo son `u32` en registros.
//!
//! A diferencia de la red, la E/S de flash es **síncrona y acotada**, así que el
//! hook resuelve la operación en el propio contexto del SVC contra la `Rufs`
//! poseída por el kernel: no hace falta un canal asíncrono ni una tarea de
//! servicio drenándolo. La exclusión mutua es trivial — durante un SVC corre solo
//! la tarea atrapada y el kernel toca la FS únicamente en el arranque
//! (mantenimiento del log de faults), antes de `start()`.
//!
//! Demostración:
//!   1. **Arranque**: monta la FS, abre el **log circular de faults** persistente
//!      (`rugus_fs::faultlog`), registra un evento de arranque, vuelca los últimos
//!      por RTT (sobreviven a power-cycles, a diferencia de la telemetría
//!      `.uninit` volátil) e incrementa un `boot_count` persistente.
//!   2. **Userland**: la app `open`-ea dos ficheros de config, **escribe** sus
//!      valores por el pool+syscall, los **relee** y verifica, y `close`-a. Repite
//!      en bucle con un latido de liveness.
//!
//! LED rojo (LD1/PJ13): parpadeo = vivo. Detalle por SWD/RTT (defmt).

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use core::ptr::addr_of_mut;

use rugus_runtime as _; // panic-probe + defmt-rtt
use rugus_runtime::entry;

use rugus_arch_cortex_m::{mpu_app_region_for, mpu_region, platform_init, time, MpuLayout};
use rugus_core::sched::Priority;
use rugus_core::syscall::user as svc_user;
use rugus_core::syscall::{self, FsHooks};
use rugus_fs::faultlog::FaultLog;
use rugus_fs::Rufs;
use rugus_hal::GpioPin;
use rugus_hal_stm32f7::cache;
use rugus_hal_stm32f7::gpio::{DiscoLed, LedPin};
use rugus_hal_stm32f7::pac;
use rugus_hal_stm32f7::qspi::Qspi;
use rugus_hal_stm32f7::rcc;

/// Capacidad del índice en RAM de la FS (claves vivas + tombstones).
const FS_KEYS: usize = 32;
/// Ranuras del log circular de faults persistente.
const FAULT_CAP: usize = 8;

/// Clave del contador de arranques persistente.
const KEY_BOOTS: &[u8] = b"boot_count";

/// Códigos `kind` del log de faults (definidos por esta placa).
const FK_BOOT: u32 = 0x0001;

// ---------------------------------------------------------------------------
// Tabla de claves de config que la app puede direccionar por `key_id`. El ABI
// userland NO maneja nombres (sin punteros): la app pide un fichero por su índice
// lógico y el servicio conoce la clave real. Acota la superficie de confianza.
// ---------------------------------------------------------------------------
const CFG_KEYS: [&[u8]; 2] = [b"cfg_hostname", b"cfg_role"];

// ---------------------------------------------------------------------------
// Pool de E/S compartido (plano de datos). Vive en SRAM y se mapea App-RW por la
// región MPU 5 (SERVICES) para que la app userland lea/escriba el contenido sin
// pasar por el kernel. El servicio (hooks, privilegiado) lo accede para persistir
// o rellenar. Power-of-two (256 B), alineado a su tamaño (reglas de región MPU).
// ---------------------------------------------------------------------------
const POOL_SLOTS: usize = 4;
const SLOT_DATA: usize = 60;

#[repr(C)]
#[derive(Clone, Copy)]
struct Slot {
    len: u32,
    data: [u8; SLOT_DATA],
}

impl Slot {
    const EMPTY: Self = Self {
        len: 0,
        data: [0; SLOT_DATA],
    };
}

/// Pool + área de estado, alineado a 512 B (= su tamaño tras el padding) para la
/// región MPU. El bloque de estado (`ok`/`err`/`iter`) lo escribe la app userland
/// (region 5, App-RW) y lo lee/loguea el `kernel_task` (privilegiado): así la app
/// reporta resultados SIN llamar a defmt (el bloque de control RTT vive en RAM
/// del kernel, priv-only, y un acceso userland dispararía MemManage).
#[repr(C, align(512))]
struct Pool {
    slots: [Slot; POOL_SLOTS],
    ok: u32,
    err: u32,
    iter: u32,
}

static mut IO_POOL: Pool = Pool {
    slots: [Slot::EMPTY; POOL_SLOTS],
    ok: 0,
    err: 0,
    iter: 0,
};

#[repr(C, align(4096))]
struct Stack4k([u8; 4096]);

static mut STACK_KERNEL: Stack4k = Stack4k([0; 4096]);
static mut STACK_APP: Stack4k = Stack4k([0; 4096]);

/// FS poseída por el servicio. La inicializa `main`; tras `start()` SOLO la tocan
/// los `FsHooks` en contexto de syscall (un único llamante a la vez).
static mut FS: Option<Rufs<Qspi, FS_KEYS>> = None;

/// Tabla de ficheros abiertos. La indexa el handle que devuelve `fs_open`.
const MAX_FILES: usize = 4;

#[derive(Clone, Copy)]
struct FileSlot {
    used: bool,
    /// Índice en [`CFG_KEYS`] de la clave que respalda este fichero.
    key_id: u32,
}

static mut FILES: [Option<FileSlot>; MAX_FILES] = [None; MAX_FILES];

/// LED de latido (LD Red).
static mut HB_LED: Option<LedPin> = None;

#[entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("cortex-m peripherals");
    let dp = pac::Peripherals::take().expect("device peripherals");

    let clocks = rcc::init(&dp);
    cache::enable(&mut cp.SCB, &mut cp.CPUID);
    rugus_runtime::enable_cycle_counter(&mut cp);

    defmt::info!(
        "rugus fs-userland @ STM32F769I-DISCO, SYSCLK {} MHz, ABI {=u16}",
        clocks.sysclk_mhz(),
        syscall::ABI_VERSION
    );

    // FPU-context + fault handlers + layout MPU de sandbox.
    platform_init(&mut cp, &MpuLayout::STM32F769);

    // Región MPU 5 (SERVICES, libre): mapea el pool de E/S como App-RW. No la toca
    // el context switch, así que persiste entre cambios de tarea; por número alto
    // gana el solapamiento con KERNEL_RAM (region 2, priv-only), de modo que la
    // app accede SOLO a este pool, no al resto de la RAM del kernel.
    // SAFETY: arranque single-thread; programación única de la región MPU 5.
    unsafe {
        let base = addr_of_mut!(IO_POOL) as u32;
        let (rbar, rasr) = mpu_app_region_for(base, core::mem::size_of::<Pool>() as u32);
        cp.MPU.rnr.write(mpu_region::SERVICES as u32);
        cp.MPU.rbar.write(rbar);
        cp.MPU.rasr.write(rasr);
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
    }
    defmt::info!(
        "pool de E/S compartido mapeado App-RW (region MPU 5, {} slots)",
        POOL_SLOTS
    );

    // Base de tiempo del kernel (SysTick 1 ms): preempción + relojes.
    time::init(&mut cp.SYST, clocks.hclk);

    // Bring-up de la QSPI NOR y montaje de la FS.
    let flash = Qspi::new(dp.QUADSPI, &dp.RCC).expect("qspi init");
    let mut fs = Rufs::<_, FS_KEYS>::mount(flash).expect("fs mount");
    defmt::info!("FS montada: {} claves vivas", fs.len());

    // --- Log circular de faults persistente (sobre la misma FS) -------------
    // Vuelca lo retenido del/los arranque(s) previo(s) ANTES de añadir el de
    // ahora: demuestra que el post-mortem sobrevive al power-off (la telemetría
    // .uninit no lo hace). Todo el acceso a la FS aquí ocurre antes de start().
    let mut flog = FaultLog::<FAULT_CAP>::open(&mut fs).expect("faultlog open");
    defmt::info!(
        "log de faults: {} eventos totales, {} retenidos",
        flog.total(),
        flog.stored()
    );
    flog.for_each(&mut fs, |r| {
        defmt::info!(
            "  fault #{=u32}: kind={=u32:#06x} arg={=u32:#010x}",
            r.seq,
            r.kind,
            r.arg
        );
    })
    .expect("faultlog iterate");

    // Contador de arranques persistente.
    let mut buf = [0u8; 4];
    let prev = match fs.get(KEY_BOOTS, &mut buf) {
        Ok(4) => u32::from_le_bytes(buf),
        _ => 0,
    };
    let boots = prev + 1;
    fs.set(KEY_BOOTS, &boots.to_le_bytes()).expect("set boots");
    defmt::info!("boot_count: {} -> {}", prev, boots);

    // Registra el arranque en el log circular (kind=BOOT, arg=nº de arranque).
    let seq = flog
        .record(&mut fs, FK_BOOT, boots)
        .expect("faultlog record");
    defmt::info!("evento de arranque registrado: fault #{=u32}", seq);

    // SAFETY: arranque single-thread; estos statics solo se inicializan aquí y a
    // partir de `start()` la FS la tocan en exclusiva los hooks (contexto SVC).
    unsafe {
        FS = Some(fs);
        // Pre-declara los ficheros de config (used=false hasta `fs_open`).
        HB_LED = Some(LedPin::new(&dp.RCC, DiscoLed::Red));
    }

    // SAFETY: arranque single-thread; pilas estáticas vivas para todo el kernel.
    unsafe {
        // Cablea los hooks del scheduler (yield/sleep/chan/ipc/...).
        rugus_kernel::install(None);
        // Registra el plano de control de ficheros: ahora `fs_*` dejan de
        // devolver Enosys y rutean a estos hooks.
        syscall::register_fs(FsHooks {
            fs_open: hook_fs_open,
            fs_read: hook_fs_read,
            fs_write: hook_fs_write,
            fs_close: hook_fs_close,
        });

        rugus_kernel::spawn(
            &mut (*addr_of_mut!(STACK_KERNEL)).0,
            kernel_task,
            Priority::Kernel,
        )
        .expect("spawn kernel");
        rugus_kernel::spawn_user(&mut (*addr_of_mut!(STACK_APP)).0, app_task, Priority::App)
            .expect("spawn app");
        defmt::info!("scheduler: 2 tareas (kernel + app userland), starting");
        rugus_kernel::start();
    }
}

// ===========================================================================
// Hooks del plano de control/datos de ficheros (corren en contexto de syscall,
// privilegiado, mientras la app está atrapada en el SVC). Tocan FS + FILES +
// IO_POOL; nunca concurren con otra tarea (el SVC corre a término).
// ===========================================================================

/// `fs_open(key_id)`: reclama un handle para el fichero de config `key_id`.
fn hook_fs_open(key_id: u32) -> i32 {
    if key_id as usize >= CFG_KEYS.len() {
        return rugus_core::Errno::Einval as i32;
    }
    // SAFETY: acceso desde el dispatch del syscall (sin concurrencia).
    unsafe {
        // ¿Ya abierto? Reusa el handle (idempotente).
        for (i, slot) in FILES.iter().enumerate() {
            if let Some(f) = slot {
                if f.used && f.key_id == key_id {
                    return i as i32;
                }
            }
        }
        for (i, slot) in FILES.iter_mut().enumerate() {
            if slot.is_none() || !slot.unwrap().used {
                *slot = Some(FileSlot { used: true, key_id });
                return i as i32;
            }
        }
    }
    rugus_core::Errno::Ebusy as i32
}

/// `fs_read(handle, slot)`: lee el fichero al slot del pool; devuelve bytes (≥0).
fn hook_fs_read(handle: u32, slot: u32) -> i32 {
    let (key, pool_slot) = match resolve(handle, slot) {
        Ok(v) => v,
        Err(e) => return e,
    };
    // SAFETY: acceso cooperativo a FS/IO_POOL desde el dispatch del syscall.
    unsafe {
        let fs = match (*addr_of_mut!(FS)).as_mut() {
            Some(f) => f,
            None => return rugus_core::Errno::Enosys as i32,
        };
        let dst = &mut (*addr_of_mut!(IO_POOL)).slots[pool_slot];
        match fs.get(key, &mut dst.data) {
            Ok(n) => {
                dst.len = n as u32;
                n as i32
            }
            Err(rugus_fs::Error::NotFound) => rugus_core::Errno::Enoent as i32,
            Err(_) => rugus_core::Errno::Einval as i32,
        }
    }
}

/// `fs_write(handle, slot, len)`: persiste `len` bytes del slot del pool.
fn hook_fs_write(handle: u32, slot: u32, len: u32) -> i32 {
    let (key, pool_slot) = match resolve(handle, slot) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let n = (len as usize).min(SLOT_DATA);
    // SAFETY: acceso cooperativo a FS/IO_POOL desde el dispatch del syscall.
    unsafe {
        let fs = match (*addr_of_mut!(FS)).as_mut() {
            Some(f) => f,
            None => return rugus_core::Errno::Enosys as i32,
        };
        let src = &(*addr_of_mut!(IO_POOL)).slots[pool_slot];
        match fs.set(key, &src.data[..n]) {
            Ok(()) => 0,
            Err(_) => rugus_core::Errno::Einval as i32,
        }
    }
}

/// `fs_close(handle)`: libera el handle.
fn hook_fs_close(handle: u32) -> i32 {
    let idx = handle as usize;
    // SAFETY: acceso desde el dispatch del syscall.
    unsafe {
        match FILES.get_mut(idx).and_then(|s| s.as_mut()) {
            Some(f) if f.used => {
                f.used = false;
                0
            }
            _ => rugus_core::Errno::Einval as i32,
        }
    }
}

/// Valida `handle`/`slot` y devuelve (clave del fichero, índice de slot del pool).
fn resolve(handle: u32, slot: u32) -> Result<(&'static [u8], usize), i32> {
    if slot as usize >= POOL_SLOTS {
        return Err(rugus_core::Errno::Einval as i32);
    }
    let idx = handle as usize;
    // SAFETY: lectura de FILES desde el dispatch del syscall.
    let key_id = unsafe {
        match FILES.get(idx).and_then(|s| s.as_ref()) {
            Some(f) if f.used => f.key_id as usize,
            _ => return Err(rugus_core::Errno::Einval as i32),
        }
    };
    let key = CFG_KEYS
        .get(key_id)
        .ok_or(rugus_core::Errno::Einval as i32)?;
    Ok((key, slot as usize))
}

// ===========================================================================
// Tareas
// ===========================================================================

/// Tarea kernel: latido visible en LD Red ~1 Hz; duerme para ceder a la app.
/// También lee el bloque de estado del pool (que rellena la app userland) y lo
/// loguea por RTT — la app no puede hacerlo (defmt toca RAM priv-only).
fn kernel_task() -> ! {
    let mut last = time::now_ms();
    let mut last_iter = 0u32;
    loop {
        let t = time::now_ms();
        if t.wrapping_sub(last) >= 500 {
            last = t;
            // SAFETY: solo esta tarea toca el LED de latido.
            unsafe {
                if let Some(led) = (*addr_of_mut!(HB_LED)).as_mut() {
                    let _ = led.toggle();
                }
            }
            // SAFETY: bloque de estado en el pool compartido; lectura privilegiada.
            unsafe {
                let p = &*addr_of_mut!(IO_POOL);
                if p.iter != last_iter {
                    last_iter = p.iter;
                    defmt::info!(
                        "app userland: iter {=u32}, verificaciones OK={=u32} MISMATCH={=u32}",
                        p.iter,
                        p.ok,
                        p.err
                    );
                }
            }
        }
        // Duerme (no busy-loop): con prioridades preemptivas, una tarea de banda
        // alta que nunca bloquea STARVA a la app userland (banda baja).
        rugus_kernel::cpu_sleep_ms(250);
    }
}

/// Tarea USERLAND (nPRIV): solo usa syscalls `fs_*` + el pool (region MPU 5).
/// No accede a la FS ni a periféricos — la MPU dispararía MemManage.
fn app_task() -> ! {
    const VAL_HOST: &[u8] = b"rugus-f769";
    const VAL_ROLE: &[u8] = b"fs-userland";

    // Abre ambos ficheros de config.
    let host = svc_user::fs_open(0);
    let role = svc_user::fs_open(1);

    // Escribe la config inicial (plano de datos por el pool + orden por syscall).
    if host >= 0 {
        write_file(host as u32, 0, VAL_HOST);
    }
    if role >= 0 {
        write_file(role as u32, 1, VAL_ROLE);
    }

    let mut iter = 0u32;
    let mut spin = 0u32;
    // La app NO duerme: al ser la tarea de menor prioridad y permanecer siempre
    // lista, evita que el scheduler entre en WFI (que congelaría el RTT en dev).
    const SPINS_PER_CYCLE: u32 = 40_000;
    loop {
        spin = spin.wrapping_add(1);
        if spin >= SPINS_PER_CYCLE {
            spin = 0;
            iter = iter.wrapping_add(1);
            // Relee y verifica ambos ficheros de config; acumula resultado en el
            // bloque de estado del pool (la app NO puede loguear por defmt).
            if host >= 0 {
                verify_file(host as u32, 2, VAL_HOST);
            }
            if role >= 0 {
                verify_file(role as u32, 3, VAL_ROLE);
            }
            // Publica el contador de iteración (lo lee y loguea el kernel_task).
            // SAFETY: bloque de estado en el pool App-RW de esta tarea.
            unsafe {
                (*addr_of_mut!(IO_POOL)).iter = iter;
            }
            let _ = svc_user::checkin();
        }
        let _ = svc_user::yield_now();
    }
}

/// Escribe `body` en el slot `slot` del pool y ordena persistirlo en `handle`.
fn write_file(handle: u32, slot: u32, body: &[u8]) {
    // SAFETY: slot < POOL_SLOTS; el pool está mapeado App-RW para esta tarea.
    unsafe {
        let s = &mut (*addr_of_mut!(IO_POOL)).slots[slot as usize];
        let n = body.len().min(SLOT_DATA);
        s.data[..n].copy_from_slice(&body[..n]);
        s.len = n as u32;
        let _ = svc_user::fs_write(handle, slot, n as u32);
    }
}

/// Relee `handle` al slot `slot` del pool, comprueba que coincide con `expect` y
/// contabiliza el resultado en el bloque de estado del pool. Sin defmt (userland).
fn verify_file(handle: u32, slot: u32, expect: &[u8]) {
    let n = svc_user::fs_read(handle, slot);
    // SAFETY: el pool está mapeado App-RW; leemos el slot que rellenó el hook y
    // actualizamos los contadores de estado (misma región MPU 5).
    unsafe {
        let pool = &mut *addr_of_mut!(IO_POOL);
        let ok = if n < 0 {
            false
        } else {
            let len = (n as usize).min(SLOT_DATA);
            &pool.slots[slot as usize].data[..len] == expect
        };
        if ok {
            pool.ok = pool.ok.wrapping_add(1);
        } else {
            pool.err = pool.err.wrapping_add(1);
        }
    }
}
