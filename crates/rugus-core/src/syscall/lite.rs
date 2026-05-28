//! Syscalls lite — appliance tier (F103, sin MPU).
//!
//! Hooks registrados por el firmware; la capa CLI nunca toca hardware directo.

use crate::Errno;

/// Identificadores de syscall lite (extensión v0.1 appliance).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Id {
    SysInfo = 0x50,
    SysStatus = 0x51,
    GpioRead = 0x52,
    GpioWrite = 0x53,
    GpioToggle = 0x54,
    GpioBind = 0x55,
    BusScan = 0x56,
    ConfigGet = 0x57,
    ConfigSet = 0x58,
    ConfigCommit = 0x59,
    ModuleList = 0x5A,
    ModuleRead = 0x5B,
    TaskList = 0x5C,
    AppReload = 0x5D,
    SysFailsafe = 0x5E,
    Wdt = 0x5F,
}

impl Id {
    /// Decodifica raw imm8.
    pub const fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            0x50 => Some(Self::SysInfo),
            0x51 => Some(Self::SysStatus),
            0x52 => Some(Self::GpioRead),
            0x53 => Some(Self::GpioWrite),
            0x54 => Some(Self::GpioToggle),
            0x55 => Some(Self::GpioBind),
            0x56 => Some(Self::BusScan),
            0x57 => Some(Self::ConfigGet),
            0x58 => Some(Self::ConfigSet),
            0x59 => Some(Self::ConfigCommit),
            0x5A => Some(Self::ModuleList),
            0x5B => Some(Self::ModuleRead),
            0x5C => Some(Self::TaskList),
            0x5D => Some(Self::AppReload),
            0x5E => Some(Self::SysFailsafe),
            0x5F => Some(Self::Wdt),
            _ => None,
        }
    }
}

/// Nivel GPIO para [`Hooks::gpio_write`].
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpioLevel {
    /// Nivel bajo.
    Low = 0,
    /// Nivel alto.
    High = 1,
}

/// Callbacks del appliance registrados por el firmware.
#[derive(Clone, Copy)]
pub struct Hooks {
    /// Escribe info del sistema en `buf`; retorna bytes escritos.
    pub sys_info: fn(&mut [u8]) -> usize,
    /// Escribe estado global en `buf`; retorna bytes escritos.
    pub sys_status: fn(&mut [u8]) -> usize,
    /// Lee GPIO. Retorna 0/1 o errno negativo.
    pub gpio_read: fn(port: u8, pin: u8) -> i32,
    /// Escribe GPIO.
    pub gpio_write: fn(port: u8, pin: u8, level: GpioLevel) -> i32,
    /// Invierte GPIO.
    pub gpio_toggle: fn(port: u8, pin: u8) -> i32,
    /// Asocia pin a rol lógico (`moor`).
    pub gpio_bind: fn(port: u8, pin: u8, role: &[u8]) -> i32,
    /// Escanea bus I2C/UART (`scout`). `bus`: 0=I2C1.
    pub bus_scan: fn(bus: u8, out: &mut [u8]) -> i32,
    /// Lee clave RFN staging (`schema`).
    pub config_get: fn(key: &[u8], out: &mut [u8]) -> i32,
    /// Escribe clave RFN staging (`scribe`).
    pub config_set: fn(key: &[u8], val: &[u8]) -> i32,
    /// Persiste config validada (`seal`).
    pub config_commit: fn() -> i32,
    /// Lista módulos detectados (`nest`).
    pub module_list: fn(out: &mut [u8]) -> i32,
    /// Lee módulo serie (`sonar`).
    pub module_read: fn(slot: u8, out: &mut [u8]) -> i32,
    /// Lista tareas scheduler (`coil`).
    pub task_list: fn(out: &mut [u8]) -> i32,
    /// Recarga app `.afr` (`hatch`).
    pub app_reload: fn(name: &[u8]) -> i32,
    /// Modo fail-safe (`anchor`).
    pub sys_failsafe: fn() -> i32,
    /// Watchdog status/kick (`ward`). action: 0=status, 1=kick.
    pub wdt: fn(action: u8) -> i32,
}

static mut LITE_HOOKS: Option<Hooks> = None;

/// Registra hooks lite. Llamar una vez desde `main` antes del loop CLI.
///
/// # Safety
///
/// Solo desde main, antes de cualquier uso concurrente.
pub unsafe fn register(hooks: Hooks) {
    unsafe {
        LITE_HOOKS = Some(hooks);
    }
}

fn hooks() -> Option<Hooks> {
    // SAFETY: lectura de static; hooks inmutables tras init.
    unsafe { LITE_HOOKS }
}

fn slice_from_args(ptr: u32, len: u32) -> Option<&'static [u8]> {
    if ptr == 0 || len == 0 {
        return None;
    }
    // SAFETY: puntero del caller en lite (mismo dominio).
    Some(unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) })
}

fn mut_slice_from_args(ptr: u32, len: u32) -> Option<&'static mut [u8]> {
    if ptr == 0 || len == 0 {
        return None;
    }
    // SAFETY: puntero del caller en lite (mismo dominio).
    Some(unsafe { core::slice::from_raw_parts_mut(ptr as *mut u8, len as usize) })
}

/// Dispatch lite invocado desde la capa user.
pub fn dispatch(id: Id, args: [u32; 4]) -> i32 {
    let Some(h) = hooks() else {
        return Errno::Einval as i32;
    };

    match id {
        Id::SysInfo | Id::SysStatus => {
            let Some(buf) = mut_slice_from_args(args[0], args[1]) else {
                return Errno::Einval as i32;
            };
            let n = match id {
                Id::SysInfo => (h.sys_info)(buf),
                Id::SysStatus => (h.sys_status)(buf),
                _ => 0,
            };
            n as i32
        }
        Id::GpioRead => (h.gpio_read)(args[0] as u8, args[1] as u8),
        Id::GpioWrite => {
            let level = match args[2] {
                0 => GpioLevel::Low,
                1 => GpioLevel::High,
                _ => return Errno::Einval as i32,
            };
            (h.gpio_write)(args[0] as u8, args[1] as u8, level)
        }
        Id::GpioToggle => (h.gpio_toggle)(args[0] as u8, args[1] as u8),
        Id::GpioBind => {
            let Some(role) = slice_from_args(args[2], args[3]) else {
                return Errno::Einval as i32;
            };
            (h.gpio_bind)(args[0] as u8, args[1] as u8, role)
        }
        Id::BusScan => {
            let Some(out) = mut_slice_from_args(args[1], args[2]) else {
                return Errno::Einval as i32;
            };
            (h.bus_scan)(args[0] as u8, out)
        }
        Id::ConfigGet => {
            let Some(key) = slice_from_args(args[0], args[1]) else {
                return Errno::Einval as i32;
            };
            let Some(out) = mut_slice_from_args(args[2], args[3]) else {
                return Errno::Einval as i32;
            };
            (h.config_get)(key, out)
        }
        Id::ConfigSet => {
            let Some(key) = slice_from_args(args[0], args[1]) else {
                return Errno::Einval as i32;
            };
            let Some(val) = slice_from_args(args[2], args[3]) else {
                return Errno::Einval as i32;
            };
            (h.config_set)(key, val)
        }
        Id::ConfigCommit => (h.config_commit)(),
        Id::ModuleList => {
            let Some(out) = mut_slice_from_args(args[0], args[1]) else {
                return Errno::Einval as i32;
            };
            (h.module_list)(out)
        }
        Id::ModuleRead => {
            let Some(out) = mut_slice_from_args(args[1], args[2]) else {
                return Errno::Einval as i32;
            };
            (h.module_read)(args[0] as u8, out)
        }
        Id::TaskList => {
            let Some(out) = mut_slice_from_args(args[0], args[1]) else {
                return Errno::Einval as i32;
            };
            (h.task_list)(out)
        }
        Id::AppReload => {
            let Some(name) = slice_from_args(args[0], args[1]) else {
                return Errno::Einval as i32;
            };
            (h.app_reload)(name)
        }
        Id::SysFailsafe => (h.sys_failsafe)(),
        Id::Wdt => (h.wdt)(args[0] as u8),
    }
}

/// API userland lite — llama dispatch directo (cooperativo en F103).
pub mod user {
    use super::{dispatch, Id};

    /// Información del sistema (`cosmos`).
    pub fn sys_info(buf: &mut [u8]) -> i32 {
        dispatch(
            Id::SysInfo,
            [buf.as_mut_ptr() as u32, buf.len() as u32, 0, 0],
        )
    }

    /// Estado global (`ecosystem`).
    pub fn sys_status(buf: &mut [u8]) -> i32 {
        dispatch(
            Id::SysStatus,
            [buf.as_mut_ptr() as u32, buf.len() as u32, 0, 0],
        )
    }

    /// Lee GPIO (`pulso`).
    pub fn gpio_read(port: u8, pin: u8) -> i32 {
        dispatch(Id::GpioRead, [port as u32, pin as u32, 0, 0])
    }

    /// Escribe GPIO (`spark`/`mute`).
    pub fn gpio_write(port: u8, pin: u8, high: bool) -> i32 {
        dispatch(Id::GpioWrite, [port as u32, pin as u32, u32::from(high), 0])
    }

    /// Invierte GPIO (`ripple`).
    pub fn gpio_toggle(port: u8, pin: u8) -> i32 {
        dispatch(Id::GpioToggle, [port as u32, pin as u32, 0, 0])
    }

    /// Asocia pin a rol (`moor`).
    pub fn gpio_bind(port: u8, pin: u8, role: &[u8]) -> i32 {
        dispatch(
            Id::GpioBind,
            [
                port as u32,
                pin as u32,
                role.as_ptr() as u32,
                role.len() as u32,
            ],
        )
    }

    /// Escanea bus (`scout`).
    pub fn bus_scan(bus: u8, out: &mut [u8]) -> i32 {
        dispatch(
            Id::BusScan,
            [bus as u32, out.as_mut_ptr() as u32, out.len() as u32, 0],
        )
    }

    /// Lee config staging (`schema`).
    pub fn config_get(key: &[u8], out: &mut [u8]) -> i32 {
        dispatch(
            Id::ConfigGet,
            [
                key.as_ptr() as u32,
                key.len() as u32,
                out.as_mut_ptr() as u32,
                out.len() as u32,
            ],
        )
    }

    /// Escribe config staging (`scribe`).
    pub fn config_set(key: &[u8], val: &[u8]) -> i32 {
        dispatch(
            Id::ConfigSet,
            [
                key.as_ptr() as u32,
                key.len() as u32,
                val.as_ptr() as u32,
                val.len() as u32,
            ],
        )
    }

    /// Persiste config (`seal`).
    pub fn config_commit() -> i32 {
        dispatch(Id::ConfigCommit, [0; 4])
    }

    /// Lista módulos (`nest`).
    pub fn module_list(out: &mut [u8]) -> i32 {
        dispatch(
            Id::ModuleList,
            [out.as_mut_ptr() as u32, out.len() as u32, 0, 0],
        )
    }

    /// Lee módulo (`sonar`).
    pub fn module_read(slot: u8, out: &mut [u8]) -> i32 {
        dispatch(
            Id::ModuleRead,
            [slot as u32, out.as_mut_ptr() as u32, out.len() as u32, 0],
        )
    }

    /// Lista tareas (`coil`).
    pub fn task_list(out: &mut [u8]) -> i32 {
        dispatch(
            Id::TaskList,
            [out.as_mut_ptr() as u32, out.len() as u32, 0, 0],
        )
    }

    /// Recarga app (`hatch`).
    pub fn app_reload(name: &[u8]) -> i32 {
        dispatch(
            Id::AppReload,
            [name.as_ptr() as u32, name.len() as u32, 0, 0],
        )
    }

    /// Fail-safe (`anchor`).
    pub fn sys_failsafe() -> i32 {
        dispatch(Id::SysFailsafe, [0; 4])
    }

    /// Watchdog (`ward`).
    pub fn wdt(action: u8) -> i32 {
        dispatch(Id::Wdt, [action as u32, 0, 0, 0])
    }
}
