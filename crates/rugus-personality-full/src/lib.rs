//! `rugus-personality-full` — tabla `lite::Hooks` del tier full (F407/F769).
//!
//! ## Por qué este crate
//!
//! `rush` (la consola universal) mapea cada verbo a
//! [`rugus_core::syscall::lite::user`], que delega en una tabla de punteros de
//! función ([`rugus_core::syscall::lite::Hooks`]) que la **personalidad** instala
//! con `lite::register`. La personalidad lite (F103) ya lo hace; las placas full
//! (F407/F769) no tenían tabla y por eso los verbos respondían `error`.
//!
//! Este crate cierra esa brecha **sin duplicar el contrato**: aporta los cuerpos
//! genéricos del tier full —respaldados por lo que F4 y F7 comparten vía
//! [`rugus_kernel`]— y recibe **inyectadas** las piezas que dependen del silicio
//! (`gpio_*`, `bus_scan`, `wdt`, `sys_failsafe`, `sting`). Así el léxico es el
//! mismo en todas las placas (mismo verbo → misma forma de salida) mientras cada
//! personalidad escala con los recursos de su placa.
//!
//! ## Reparto de responsabilidades
//!
//! - **Genéricos del kernel (este crate):** `cosmos`/`sys_info`,
//!   `ecosystem`/`sys_status`, `letargo`/`sys_power`, `coil`/`task_list`,
//!   `scar`. Se leen de la API pública de [`rugus_kernel`].
//! - **De placa (inyectados en [`BoardOps`]):** `gpio_read/write/toggle/bind`,
//!   `bus_scan`, `wdt`, `sys_failsafe`, `sting`.
//! - **Sin respaldo en este tier (stubs honestos → `Enosys`):** `schema`/
//!   `scribe`/`seal` (config RFN), `nest`/`sonar`/`nest renew` (módulos serie),
//!   `hatch` (recarga de apps). Devuelven `Enosys` para que la consola muestre un
//!   "no soportado" claro en vez de fingir.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

use rugus_core::syscall::lite::{GpioLevel, Hooks};
use rugus_core::Errno;

/// Operaciones específicas de placa que la personalidad full inyecta. Sus firmas
/// son idénticas a los campos homónimos de [`Hooks`]: se trasladan tal cual a la
/// tabla, sin indirección. Una placa que no cablee alguna pasa el stub
/// correspondiente de [`BoardOps::unsupported`].
#[derive(Clone, Copy)]
pub struct BoardOps {
    /// Identidad de la placa para `cosmos`/`ecosystem` (p. ej. `"f407-disco"`).
    pub name: &'static str,
    /// Lee GPIO (`pulso`). Retorna 0/1 o errno negativo.
    pub gpio_read: fn(port: u8, pin: u8) -> i32,
    /// Escribe GPIO (`spark`/`mute`).
    pub gpio_write: fn(port: u8, pin: u8, level: GpioLevel) -> i32,
    /// Invierte GPIO (`ripple`).
    pub gpio_toggle: fn(port: u8, pin: u8) -> i32,
    /// Asocia pin a rol lógico (`moor`).
    pub gpio_bind: fn(port: u8, pin: u8, role: &[u8]) -> i32,
    /// Escanea bus (`scout`).
    pub bus_scan: fn(bus: u8, out: &mut [u8]) -> i32,
    /// Watchdog status/kick (`ward`). action: 0=status, 1=kick.
    pub wdt: fn(action: u8) -> i32,
    /// Modo fail-safe (`anchor`). action: 0=on, 1=off.
    pub sys_failsafe: fn(action: u8) -> i32,
    /// Provoca un fault controlado (`sting`) para validar el failsafe.
    pub sting: fn() -> i32,
}

impl BoardOps {
    /// Construye un `BoardOps` con todas las operaciones marcadas como no
    /// soportadas (`Enosys`). La placa sobrescribe solo los campos que cablea:
    ///
    /// ```ignore
    /// let ops = BoardOps { gpio_read: my_read, ..BoardOps::unsupported("f407-disco") };
    /// ```
    pub const fn unsupported(name: &'static str) -> Self {
        Self {
            name,
            gpio_read: stub_rw2,
            gpio_write: stub_gpio_write,
            gpio_toggle: stub_rw2,
            gpio_bind: stub_gpio_bind,
            bus_scan: stub_bus_scan,
            wdt: stub_action,
            sys_failsafe: stub_action,
            sting: stub_unit,
        }
    }
}

/// Identidad de placa para los verbos informativos. La fija [`hooks`] al
/// construir la tabla (una vez en el arranque single-thread).
static mut BOARD_NAME: &str = "full";

/// Construye la tabla [`Hooks`] del tier full: genéricos del kernel + las
/// operaciones de placa de `ops`. Regístrala con `lite::register(hooks(ops))`.
///
/// # Safety
/// Debe llamarse una sola vez en el arranque single-thread, antes de cualquier
/// uso de la consola: fija la identidad de placa en un `static`.
pub unsafe fn hooks(ops: BoardOps) -> Hooks {
    unsafe { BOARD_NAME = ops.name };
    Hooks {
        // Genéricos respaldados por el kernel.
        sys_info: hook_sys_info,
        sys_status: hook_sys_status,
        sys_power: hook_sys_power,
        task_list: hook_task_list,
        scar: hook_scar,
        // De placa (inyectados).
        gpio_read: ops.gpio_read,
        gpio_write: ops.gpio_write,
        gpio_toggle: ops.gpio_toggle,
        gpio_bind: ops.gpio_bind,
        bus_scan: ops.bus_scan,
        wdt: ops.wdt,
        sys_failsafe: ops.sys_failsafe,
        sting: ops.sting,
        // Sin respaldo en este tier (honestos: Enosys).
        config_get: stub_kv_out,
        config_set: stub_kv,
        config_commit: stub_unit,
        module_list: stub_out,
        module_read: stub_slot_out,
        module_renew: stub_unit,
        app_reload: stub_name,
    }
}

// ---------------------------------------------------------------------------
// Hooks genéricos respaldados por `rugus_kernel`.
// ---------------------------------------------------------------------------

/// `cosmos` → identidad de la placa + datos vivos del kernel.
fn hook_sys_info(buf: &mut [u8]) -> usize {
    let mut w = SliceWriter::new(buf);
    // SAFETY: BOARD_NAME se fija una vez en `hooks` durante el arranque.
    w.str(unsafe { BOARD_NAME });
    w.str(" · personality=full · tier=full\r\n");
    w.str("arranques=");
    w.u32(rugus_kernel::boot_count());
    w.str(" tareas=");
    w.u32(rugus_kernel::task_count() as u32);
    if rugus_kernel::safe_mode() {
        w.str(" [SAFE-MODE]");
    }
    w.str("\r\n");
    w.len()
}

/// `ecosystem` → estado global: tareas, faults, causa del último reset.
fn hook_sys_status(buf: &mut [u8]) -> usize {
    let mut w = SliceWriter::new(buf);
    w.str("tareas=");
    w.u32(rugus_kernel::task_count() as u32);
    w.str(" faults_total=");
    w.u32(rugus_kernel::total_faults());
    if rugus_kernel::safe_mode() {
        w.str(" [SAFE-MODE]");
    }
    w.str("\r\nultimo reset: ");
    w.str(rugus_kernel::reset_cause());
    w.str("\r\n");
    w.len()
}

/// `letargo` → métricas de energía/ocio del proveedor del kernel.
fn hook_sys_power(buf: &mut [u8]) -> usize {
    let mut w = SliceWriter::new(buf);
    match rugus_kernel::power_stats() {
        Some(p) => {
            w.str("uptime=");
            w.u32(p.uptime_ms);
            w.str(" ms  idle=");
            w.u32(p.idle_ms);
            w.str(" ms (");
            w.u32(p.idle_percent());
            w.str("%)\r\nsystick_irqs=");
            w.u32(p.systick_irqs);
            w.str("  stop_entries=");
            w.u32(p.stop_entries);
            w.str("\r\n");
        }
        None => {
            w.str("energia no disponible (sin proveedor)\r\n");
        }
    }
    w.len()
}

/// `coil` → tabla de tareas (idx, prioridad, modo, estado, stack usado/total).
fn hook_task_list(out: &mut [u8]) -> i32 {
    let mut w = SliceWriter::new(out);
    w.str("idx pri modo estado stack\r\n");
    for idx in 0..rugus_kernel::task_count() {
        w.u32(idx as u32);
        w.str("   ");
        w.u32(rugus_kernel::task_priority(idx) as u32);
        w.str("  ");
        w.str(if rugus_kernel::is_user_task(idx) {
            "user "
        } else {
            "kern "
        });
        w.str(rugus_kernel::task_state_name(idx));
        w.str(" ");
        w.u32(rugus_kernel::stack_high_water(idx));
        w.str("/");
        w.u32(rugus_kernel::stack_len(idx));
        w.str("\r\n");
    }
    w.len() as i32
}

/// `scar` → post-mortem del último fault contenido. action 0=leer, 1=borrar.
/// En el tier full la cicatriz persistente se consume al arrancar; aquí se
/// reporta la telemetría viva del kernel. `clear` es un no-op (retorna 0).
fn hook_scar(action: u8, out: &mut [u8]) -> i32 {
    if action == 1 {
        return 0; // nada que borrar: la copia persistente ya se consumió al boot.
    }
    let mut w = SliceWriter::new(out);
    w.str("arranques=");
    w.u32(rugus_kernel::boot_count());
    w.str(" faults_total=");
    w.u32(rugus_kernel::total_faults());
    if rugus_kernel::safe_mode() {
        w.str(" [SAFE-MODE]");
    }
    w.str("\r\n");
    for idx in 0..rugus_kernel::task_count() {
        let c = rugus_kernel::faults_for(idx);
        if c > 0 {
            w.str("  task ");
            w.u32(idx as u32);
            w.str(": ");
            w.u32(c);
            w.str(" faults\r\n");
        }
    }
    match rugus_kernel::last_fault() {
        Some((kind, task, pc, addr)) => {
            w.str("ultimo: kind=");
            w.u32(kind as u32);
            w.str(" task=");
            w.u32(task as u32);
            w.str(" pc=0x");
            w.hex(pc);
            w.str(" addr=0x");
            w.hex(addr);
            w.str("\r\n");
        }
        None => w.str("sin faults registrados\r\n"),
    }
    w.len() as i32
}

// ---------------------------------------------------------------------------
// Stubs honestos: servicios sin respaldo en el tier full (retornan Enosys).
// ---------------------------------------------------------------------------

fn stub_rw2(_a: u8, _b: u8) -> i32 {
    Errno::Enosys as i32
}
fn stub_gpio_write(_p: u8, _n: u8, _l: GpioLevel) -> i32 {
    Errno::Enosys as i32
}
fn stub_gpio_bind(_p: u8, _n: u8, _role: &[u8]) -> i32 {
    Errno::Enosys as i32
}
fn stub_bus_scan(_bus: u8, _out: &mut [u8]) -> i32 {
    Errno::Enosys as i32
}
fn stub_action(_action: u8) -> i32 {
    Errno::Enosys as i32
}
fn stub_unit() -> i32 {
    Errno::Enosys as i32
}
fn stub_kv_out(_k: &[u8], _out: &mut [u8]) -> i32 {
    Errno::Enosys as i32
}
fn stub_kv(_k: &[u8], _v: &[u8]) -> i32 {
    Errno::Enosys as i32
}
fn stub_out(_out: &mut [u8]) -> i32 {
    Errno::Enosys as i32
}
fn stub_slot_out(_slot: u8, _out: &mut [u8]) -> i32 {
    Errno::Enosys as i32
}
fn stub_name(_name: &[u8]) -> i32 {
    Errno::Enosys as i32
}

// ---------------------------------------------------------------------------
// Escritor sobre un slice de bytes, sin `alloc` ni `defmt`. Satura en capacidad
// (las salidas de consola son cortas; nunca corrompe memoria).
// ---------------------------------------------------------------------------

struct SliceWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> SliceWriter<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn len(&self) -> usize {
        self.pos
    }

    /// Copia `s` truncando si no cabe (sin desbordar).
    fn str(&mut self, s: &str) {
        let bytes = s.as_bytes();
        let n = bytes.len().min(self.buf.len() - self.pos);
        self.buf[self.pos..self.pos + n].copy_from_slice(&bytes[..n]);
        self.pos += n;
    }

    /// Formatea `v` en decimal.
    fn u32(&mut self, v: u32) {
        let mut tmp = [0u8; 10];
        let mut i = tmp.len();
        let mut n = v;
        if n == 0 {
            i -= 1;
            tmp[i] = b'0';
        } else {
            while n > 0 && i > 0 {
                i -= 1;
                tmp[i] = b'0' + (n % 10) as u8;
                n /= 10;
            }
        }
        // SAFETY: solo dígitos ASCII en [i, len).
        self.str(unsafe { core::str::from_utf8_unchecked(&tmp[i..]) });
    }

    /// Formatea `v` en hexadecimal (sin prefijo).
    fn hex(&mut self, v: u32) {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut tmp = [0u8; 8];
        let mut i = tmp.len();
        let mut n = v;
        if n == 0 {
            i -= 1;
            tmp[i] = b'0';
        } else {
            while n > 0 && i > 0 {
                i -= 1;
                tmp[i] = HEX[(n & 0xF) as usize];
                n >>= 4;
            }
        }
        // SAFETY: solo dígitos hex ASCII en [i, len).
        self.str(unsafe { core::str::from_utf8_unchecked(&tmp[i..]) });
    }
}
