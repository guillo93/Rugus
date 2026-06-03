//! Syscall ABI v0.1 — dispatch y trampolines userland. Ver `docs/SYSCALL_ABI.md`.

pub mod lite;

use crate::sched::TaskId;
use crate::Errno;

/// Versión actual del ABI expuesta a userspace.
pub const ABI_VERSION: u16 = 0x0001;

/// Identificadores de syscall. Los valores numéricos son parte del ABI
/// estable post-G2 — no renumerar tras 1.0.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Id {
    YieldNow = 0x00,
    SleepMs = 0x01,
    TaskId = 0x02,
    Log = 0x03,
    IpcSend = 0x10,
    IpcRecv = 0x11,
    ChanSend = 0x12,
    ChanRecv = 0x13,
    MutexLock = 0x20,
    MutexUnlock = 0x21,
    SemWait = 0x22,
    SemPost = 0x23,
    NetSocket = 0x30,
    NetConnect = 0x31,
    NetSend = 0x32,
    NetRecv = 0x33,
    CryptoSign = 0x40,
    RngFill = 0x41,
    PanicApp = 0xFE,
    Extended = 0xFF,
}

impl Id {
    /// Decodifica raw imm8 del SVC.
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            0x00 => Some(Self::YieldNow),
            0x01 => Some(Self::SleepMs),
            0x02 => Some(Self::TaskId),
            0x03 => Some(Self::Log),
            0x10 => Some(Self::IpcSend),
            0x11 => Some(Self::IpcRecv),
            0x12 => Some(Self::ChanSend),
            0x13 => Some(Self::ChanRecv),
            0x20 => Some(Self::MutexLock),
            0x21 => Some(Self::MutexUnlock),
            0x22 => Some(Self::SemWait),
            0x23 => Some(Self::SemPost),
            0x30 => Some(Self::NetSocket),
            0x31 => Some(Self::NetConnect),
            0x32 => Some(Self::NetSend),
            0x33 => Some(Self::NetRecv),
            0x40 => Some(Self::CryptoSign),
            0x41 => Some(Self::RngFill),
            0xFE => Some(Self::PanicApp),
            0xFF => Some(Self::Extended),
            _ => None,
        }
    }
}

/// Hooks registrados por el kernel / ejemplo antes de arrancar tareas.
#[derive(Clone, Copy)]
pub struct Hooks {
    /// Cede CPU (cooperativo).
    pub yield_now: fn(),
    /// Duerme la tarea actual `ms` milisegundos (sleep/wake del scheduler).
    pub sleep_ms: fn(u32),
    /// ID de la tarea en ejecución.
    pub current_task_id: fn() -> TaskId,
    /// Dominio lógico de la tarea en ejecución.
    pub current_domain: fn() -> crate::Domain,
    /// Región MPU `(base, len)` del stack de la tarea en ejecución si es
    /// userland; `None` si es privilegiada. Fuente para [`validate_user_range`].
    pub current_user_region: fn() -> Option<(u32, u32)>,
    /// Entrega un mensaje IPC por valor (`chan`, `msg`) a un buzón del kernel.
    /// Retorna `0` si encolado, [`Errno`] negativo si el canal no existe o
    /// está lleno. Es la ruta de I/O userland → driver privilegiado sin
    /// punteros: todo viaja en registros, así que no hay rango que validar.
    pub ipc_send: fn(chan: u32, msg: u32) -> i32,
    /// Toma el mutex `id` (bloquea con herencia de prioridad si está ocupado).
    pub mutex_lock: fn(id: u32) -> i32,
    /// Libera el mutex `id` (debe ser el dueño).
    pub mutex_unlock: fn(id: u32) -> i32,
    /// Consume un permiso del semáforo `id` (bloquea si no hay).
    pub sem_wait: fn(id: u32) -> i32,
    /// Devuelve un permiso al semáforo `id` (despierta a un waiter).
    pub sem_post: fn(id: u32) -> i32,
    /// Envía `msg` por el canal IPC `chan` con `timeout_ms` (bloquea con plazo
    /// si está lleno). Todo por valor en registros: sin punteros que validar.
    pub chan_send: fn(chan: u32, msg: u32, timeout_ms: u32) -> i32,
    /// Recibe del canal IPC `chan` con `timeout_ms` y escribe el mensaje en
    /// `out_ptr` (4 bytes, ya validado por [`validate_user_range`] en el
    /// dispatch). Retorna `0` al recibir, [`Errno`] negativo en error/timeout.
    pub chan_recv: fn(chan: u32, timeout_ms: u32, out_ptr: u32) -> i32,
}

static mut HOOKS: Option<Hooks> = None;

/// Registra callbacks del scheduler. Llamar una vez desde `main`.
///
/// # Safety
///
/// Solo desde main, antes de `sched.start()`.
pub unsafe fn register(hooks: Hooks) {
    unsafe {
        HOOKS = Some(hooks);
    }
}

/// ID de tarea actual (0 si no hay hooks).
pub fn current_task_id() -> TaskId {
    // SAFETY: lectura de static; hooks inmutables tras init.
    unsafe { HOOKS.map(|h| (h.current_task_id)()).unwrap_or(TaskId(0)) }
}

/// Dominio de la tarea actual.
pub fn current_domain() -> crate::Domain {
    // SAFETY: lectura de static; hooks inmutables tras init.
    unsafe {
        HOOKS
            .map(|h| (h.current_domain)())
            .unwrap_or(crate::Domain::Kernel)
    }
}

/// Valida que el rango `[ptr, ptr + len)` es accesible para el llamante actual.
///
/// Implementa el contrato de validación documentado en [`dispatch`]: es el
/// helper que todo syscall con puntero debe invocar antes de desreferenciar
/// memoria controlada por una tarea no confiable.
///
/// Reglas:
/// - `len == 0`: `Ok(())` (no hay nada que desreferenciar).
/// - Llamante **privilegiado** (sin región userland): `Ok(())` — el kernel se
///   confía a sí mismo y la MPU no restringe el modo privilegiado.
/// - Llamante **userland**: `Ok(())` solo si `[ptr, ptr+len)` no se desborda y
///   cae **completo** dentro de su región App-RW. El chequeo del rango entero
///   (no solo del puntero base) cierra el TOCTOU de un `len` que cruza el borde.
/// - Sin hooks registrados: [`Errno::Efault`] — fail-closed, no se valida a
///   ciegas.
///
/// La comprobación usa aritmética saturada (`checked_add`) para que un
/// `ptr + len` que envuelve el espacio de direcciones no falsee la contención.
pub fn validate_user_range(ptr: u32, len: u32) -> Result<(), Errno> {
    if len == 0 {
        return Ok(());
    }
    // SAFETY: lectura de static; hooks inmutables tras init.
    let region = unsafe { HOOKS.map(|h| (h.current_user_region)()) };
    match region {
        // Sin hooks: no se puede determinar la región del llamante → rechazar.
        None => Err(Errno::Efault),
        // Llamante privilegiado: confiado.
        Some(None) => Ok(()),
        // Llamante userland: el rango debe caer completo en su región App-RW.
        Some(Some((base, region_len))) => {
            let end = ptr.checked_add(len).ok_or(Errno::Efault)?;
            let region_end = base.checked_add(region_len).ok_or(Errno::Efault)?;
            if ptr >= base && end <= region_end {
                Ok(())
            } else {
                Err(Errno::Efault)
            }
        }
    }
}

/// Dispatch central invocado desde el SVC handler (arch backend).
///
/// # Contrato de validación de punteros (CRÍTICO para seguridad del kernel)
///
/// Los `args` provienen del frame de excepción de una tarea **potencialmente
/// userland** y NO son de confianza. Cualquier syscall que en el futuro reciba
/// un puntero/longitud en `args` (p. ej. `Log`, `IpcSend/Recv`, `NetSend/Recv`,
/// `CryptoSign`, `RngFill`) DEBE, antes de desreferenciarlo:
///
/// llamar a [`validate_user_range`], que:
///
/// 1. Valida que el rango `[ptr, ptr+len)` no se desborda (`checked_add`).
/// 2. Comprueba que ese rango cae **completo** dentro de la región MPU de la
///    tarea llamante (su stack App-RW) — nunca en RAM del kernel, periféricos
///    ni flash; un llamante privilegiado es confiado.
/// 3. Devuelve [`Errno::Efault`] si la comprobación falla; jamás copiar a/de un
///    puntero sin validar (TOCTOU: copiar a buffer del kernel y validar solo el
///    puntero base no basta — hay que validar el rango entero).
///
/// La frontera de confianza es ESTE punto: una vez que el SVC handler entra en
/// modo privilegiado, el MPU ya no protege contra accesos del propio kernel, así
/// que la validación es responsabilidad del dispatch, no del hardware. Las
/// syscalls actuales (`YieldNow`, `TaskId`) no toman punteros; el primer syscall
/// con puntero debe enrutar sus `args` por [`validate_user_range`].
pub fn dispatch(id: Id, args: [u32; 4]) -> i32 {
    match id {
        Id::YieldNow => {
            // SAFETY: hook registrado antes de userland.
            unsafe {
                if let Some(h) = HOOKS {
                    (h.yield_now)();
                }
            }
            0
        }
        Id::TaskId => {
            let id = current_task_id();
            id.0 as i32
        }
        Id::SleepMs => {
            // SAFETY: hook registrado antes de userland.
            unsafe {
                if let Some(h) = HOOKS {
                    (h.sleep_ms)(args[0]);
                }
            }
            0
        }
        Id::IpcSend => {
            // chan=args[0], msg=args[1]; ambos por valor (sin punteros).
            // SAFETY: hook registrado antes de userland.
            unsafe {
                match HOOKS {
                    Some(h) => (h.ipc_send)(args[0], args[1]),
                    None => Errno::Efault as i32,
                }
            }
        }
        Id::ChanSend => {
            // chan=args[0], msg=args[1], timeout_ms=args[2]; todo por valor.
            // SAFETY: hook registrado antes de userland.
            unsafe {
                match HOOKS {
                    Some(h) => (h.chan_send)(args[0], args[1], args[2]),
                    None => Errno::Efault as i32,
                }
            }
        }
        Id::ChanRecv => {
            // chan=args[0], timeout_ms=args[1], out_ptr=args[2] (4 bytes).
            // Syscall con puntero: valida el rango de salida ANTES de que el hook
            // escriba en él (contrato de [`dispatch`]).
            match validate_user_range(args[2], 4) {
                // SAFETY: hook registrado antes de userland; rango validado.
                Ok(()) => unsafe {
                    match HOOKS {
                        Some(h) => (h.chan_recv)(args[0], args[1], args[2]),
                        None => Errno::Efault as i32,
                    }
                },
                Err(e) => e as i32,
            }
        }
        Id::MutexLock => sync_call(args[0], |h| h.mutex_lock),
        Id::MutexUnlock => sync_call(args[0], |h| h.mutex_unlock),
        Id::SemWait => sync_call(args[0], |h| h.sem_wait),
        Id::SemPost => sync_call(args[0], |h| h.sem_post),
        Id::Log
        | Id::IpcRecv
        | Id::NetSocket
        | Id::NetConnect
        | Id::NetSend
        | Id::NetRecv
        | Id::CryptoSign
        | Id::RngFill
        | Id::Extended => {
            let _ = args;
            Errno::Einval as i32
        }
        Id::PanicApp => {
            let _ = args;
            Errno::Einval as i32
        }
    }
}

/// Rutea un syscall de sincronización (id de objeto en `obj`) al hook elegido
/// por `pick`. Fail-closed: [`Errno::Efault`] si no hay hooks registrados.
fn sync_call(obj: u32, pick: fn(&Hooks) -> fn(u32) -> i32) -> i32 {
    // SAFETY: lectura de static; hooks inmutables tras init.
    unsafe {
        match HOOKS {
            Some(h) => pick(&h)(obj),
            None => Errno::Efault as i32,
        }
    }
}

/// Trampolines userland — ejecutan `SVC #imm8` (ARMv7-M ABI).
///
/// Solo se compilan en targets ARM: usan ensamblador en línea con registros
/// `r0..r3`, inexistentes en el triple host donde corren los tests
/// (`rugus-host-tests`). El resto del ABI (`Id`, `dispatch`,
/// `validate_user_range`) es agnóstico y sí se prueba en host.
#[cfg(target_arch = "arm")]
pub mod user {
    use super::Id;

    /// Cede el CPU al scheduler (`Id::YieldNow`).
    #[inline(always)]
    pub fn yield_now() -> i32 {
        svc_imm(Id::YieldNow as u8)
    }

    /// Retorna el ID de la tarea actual (`Id::TaskId`).
    #[inline(always)]
    pub fn task_id() -> i32 {
        svc_imm(Id::TaskId as u8)
    }

    /// Duerme la tarea actual `ms` milisegundos (`Id::SleepMs`).
    ///
    /// `ms` viaja en `r0`, que el handler SVC recupera del frame apilado como
    /// `args[0]`. `0` equivale a ceder el CPU.
    #[inline(always)]
    pub fn sleep_ms(ms: u32) -> i32 {
        let ret: i32;
        // SAFETY: SVC con r0=ms; el dispatch lee args[0] del frame.
        unsafe {
            core::arch::asm!(
                "svc 1",
                in("r0") ms,
                lateout("r0") ret,
                options(nomem, nostack)
            );
        }
        ret
    }

    /// Envía un mensaje IPC por valor a un buzón del kernel (`Id::IpcSend`).
    ///
    /// `chan` viaja en `r0` y `msg` en `r1`; el dispatch los lee del frame como
    /// `args[0]`/`args[1]`. Sin punteros: la ruta de I/O userland → driver
    /// privilegiado no expone memoria de la app, así que no hay MemManage.
    #[inline(always)]
    pub fn ipc_send(chan: u32, msg: u32) -> i32 {
        let ret: i32;
        // SAFETY: SVC con r0=chan, r1=msg; el dispatch lee args[0..2] del frame.
        unsafe {
            core::arch::asm!(
                "svc 0x10",
                in("r0") chan,
                in("r1") msg,
                lateout("r0") ret,
                options(nomem, nostack)
            );
        }
        ret
    }

    /// Envía `msg` por el canal IPC bloqueante `chan` (`Id::ChanSend`) con
    /// `timeout_ms` (bloquea con plazo si está lleno; `0` no bloquea,
    /// `u32::MAX` indefinido). `chan`/`msg`/`timeout_ms` viajan en `r0`/`r1`/`r2`.
    #[inline(always)]
    pub fn chan_send(chan: u32, msg: u32, timeout_ms: u32) -> i32 {
        let ret: i32;
        // SAFETY: SVC con r0=chan, r1=msg, r2=timeout; dispatch lee args[0..3].
        unsafe {
            core::arch::asm!(
                "svc 0x12",
                in("r0") chan,
                in("r1") msg,
                in("r2") timeout_ms,
                lateout("r0") ret,
                options(nomem, nostack)
            );
        }
        ret
    }

    /// Recibe del canal IPC bloqueante `chan` (`Id::ChanRecv`) con `timeout_ms`,
    /// escribiendo el mensaje en `*out` si retorna `0`. `chan`/`timeout_ms`/`out`
    /// viajan en `r0`/`r1`/`r2`; el dispatch valida el rango de `out` (4 bytes)
    /// contra la región del llamante antes de que el kernel escriba.
    #[inline(always)]
    pub fn chan_recv(chan: u32, timeout_ms: u32, out: &mut u32) -> i32 {
        let ret: i32;
        let out_ptr = out as *mut u32 as u32;
        // SAFETY: SVC con r0=chan, r1=timeout, r2=out_ptr. Sin `nomem`: el kernel
        // escribe en `*out` durante el syscall, así que el compilador no puede
        // asumir que la memoria queda intacta.
        unsafe {
            core::arch::asm!(
                "svc 0x13",
                in("r0") chan,
                in("r1") timeout_ms,
                in("r2") out_ptr,
                lateout("r0") ret,
                options(nostack)
            );
        }
        ret
    }

    /// Toma el mutex `id` (`Id::MutexLock`); bloquea con herencia de prioridad
    /// si está ocupado. `id` viaja en `r0`.
    #[inline(always)]
    pub fn mutex_lock(id: u32) -> i32 {
        svc_arg(0x20, id)
    }

    /// Libera el mutex `id` (`Id::MutexUnlock`). `id` viaja en `r0`.
    #[inline(always)]
    pub fn mutex_unlock(id: u32) -> i32 {
        svc_arg(0x21, id)
    }

    /// Consume un permiso del semáforo `id` (`Id::SemWait`), bloqueando si no
    /// hay. `id` viaja en `r0`.
    #[inline(always)]
    pub fn sem_wait(id: u32) -> i32 {
        svc_arg(0x22, id)
    }

    /// Devuelve un permiso al semáforo `id` (`Id::SemPost`). `id` viaja en `r0`.
    #[inline(always)]
    pub fn sem_post(id: u32) -> i32 {
        svc_arg(0x23, id)
    }

    /// Ejecuta `SVC #imm` con un argumento en `r0` y devuelve `r0`. Los cuatro
    /// syscalls de sincronización comparten esta forma (1 arg → 1 retorno).
    #[inline(always)]
    fn svc_arg(imm: u8, arg: u32) -> i32 {
        let ret: i32;
        // SAFETY: SVC con r0=arg; el dispatch lee args[0] del frame apilado.
        // El imm8 se codifica en la instrucción, así que cada valor necesita su
        // propia instrucción (no se puede parametrizar el imm en runtime).
        match imm {
            0x20 => unsafe {
                core::arch::asm!("svc 0x20", in("r0") arg, lateout("r0") ret, options(nomem, nostack))
            },
            0x21 => unsafe {
                core::arch::asm!("svc 0x21", in("r0") arg, lateout("r0") ret, options(nomem, nostack))
            },
            0x22 => unsafe {
                core::arch::asm!("svc 0x22", in("r0") arg, lateout("r0") ret, options(nomem, nostack))
            },
            0x23 => unsafe {
                core::arch::asm!("svc 0x23", in("r0") arg, lateout("r0") ret, options(nomem, nostack))
            },
            _ => return crate::Errno::Einval as i32,
        }
        ret
    }

    #[inline(always)]
    fn svc_imm(imm: u8) -> i32 {
        match imm {
            0x00 => svc0_00(),
            0x02 => svc0_02(),
            _ => crate::Errno::Einval as i32,
        }
    }

    #[inline(always)]
    fn svc0_00() -> i32 {
        let ret: i32;
        unsafe {
            core::arch::asm!(
                "svc 0",
                lateout("r0") ret,
                options(nomem, nostack)
            );
        }
        ret
    }

    #[inline(always)]
    fn svc0_02() -> i32 {
        let ret: i32;
        unsafe {
            core::arch::asm!(
                "svc 2",
                lateout("r0") ret,
                options(nomem, nostack)
            );
        }
        ret
    }
}
