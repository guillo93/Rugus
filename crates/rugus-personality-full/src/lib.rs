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
use rugus_ui::{Painter, Role};

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
    let mut p = Painter::new(buf);
    p.header("cosmos");
    // Identidad de la placa (oro = foco) + insignias de personalidad/tier.
    // SAFETY: BOARD_NAME se fija una vez en `hooks` durante el arranque.
    p.text(Role::Focus, unsafe { BOARD_NAME }).raw("  ");
    p.badge(Role::Core, " full ").raw(" ");
    p.badge(Role::Data, " tier:full ").raw("\r\n");
    // Datos vivos del kernel, alineados clave/valor.
    p.kvn("arranques", Role::Data, rugus_kernel::boot_count())
        .raw("   ");
    p.kvn("tareas", Role::Data, rugus_kernel::task_count() as u32);
    if rugus_kernel::safe_mode() {
        p.raw("   ").badge(Role::Warn, " SAFE-MODE ");
    }
    p.raw("\r\n");
    p.len()
}

/// `ecosystem` → estado global: tareas, faults, causa del último reset.
fn hook_sys_status(buf: &mut [u8]) -> usize {
    let mut p = Painter::new(buf);
    p.header("ecosystem");
    let faults = rugus_kernel::total_faults();
    let safe = rugus_kernel::safe_mode();
    // Veredicto de salud de un vistazo: verde si todo limpio, si no ámbar/rojo.
    if safe {
        p.badge(Role::Fault, " SAFE-MODE ").raw("  ");
    } else if faults == 0 {
        p.badge(Role::Core, " sano ").raw("  ");
    } else {
        p.badge(Role::Warn, " degradado ").raw("  ");
    }
    p.raw("\r\n");
    p.kvn("tareas", Role::Data, rugus_kernel::task_count() as u32)
        .raw("   ");
    p.kvn(
        "faults",
        if faults == 0 { Role::Core } else { Role::Fault },
        faults,
    );
    p.raw("\r\n");
    p.kv("reset", Role::Data, rugus_kernel::reset_cause())
        .raw("\r\n");
    p.len()
}

/// `letargo` → métricas de energía/ocio del proveedor del kernel.
fn hook_sys_power(buf: &mut [u8]) -> usize {
    let mut p = Painter::new(buf);
    p.header("letargo");
    match rugus_kernel::power_stats() {
        Some(s) => {
            // El idle es lo importante: barra de ocio (más = mejor en RTOS
            // durmiente). El medidor colorea por umbral; aquí alto idle es sano,
            // así que invertimos la lectura usando 100-idle para el color.
            let idle = s.idle_percent().min(100);
            p.kvn("uptime", Role::Data, s.uptime_ms)
                .text(Role::Chrome, " ms\r\n");
            p.on(Role::Chrome).raw("idle   ").off();
            p.meter(100 - idle, 16).raw(" ");
            p.num(Role::Core, idle).text(Role::Chrome, "% ocio\r\n");
            p.kvn("systick", Role::Data, s.systick_irqs).raw("   ");
            p.kvn("stop", Role::Data, s.stop_entries).raw("\r\n");
        }
        None => {
            p.text(Role::Warn, "energia no disponible (sin proveedor)\r\n");
        }
    }
    p.len()
}

/// `coil` → tabla de tareas (idx, prioridad, modo, estado, stack usado/total).
fn hook_task_list(out: &mut [u8]) -> i32 {
    let mut p = Painter::new(out);
    p.header("coil");
    // Cabecera de columnas en gris (cromo): no es dato, es estructura.
    p.text(Role::Chrome, "  # pri  modo  estado    pila\r\n");
    for idx in 0..rugus_kernel::task_count() {
        let used = rugus_kernel::stack_high_water(idx);
        let total = rugus_kernel::stack_len(idx).max(1);
        let pct = (used * 100 / total).min(100);
        // # y prioridad.
        p.raw("  ").num(Role::Data, idx as u32).raw("  ");
        p.num(Role::Data, rugus_kernel::task_priority(idx) as u32)
            .raw("  ");
        // Modo: kernel en oro (privilegio), user en plata.
        if rugus_kernel::is_user_task(idx) {
            p.text(Role::Text, "user");
        } else {
            p.text(Role::Focus, "kern");
        }
        p.raw("  ");
        // Estado: verde si corre/listo, ámbar si bloqueado/durmiente, rojo si
        // muerto. Heurística por nombre para no acoplar al enum del kernel.
        let st = rugus_kernel::task_state_name(idx);
        let st_role = state_role(st);
        p.on(st_role).raw(st).off();
        // Relleno a 9 columnas para alinear el medidor.
        for _ in st.len()..9 {
            p.raw(" ");
        }
        // Mini-medidor de pila + porcentaje.
        p.meter(pct, 8)
            .raw(" ")
            .num(stack_role(pct), pct)
            .text(Role::Chrome, "%\r\n");
    }
    p.len() as i32
}

/// Color del estado de una tarea (verde sano / ámbar en espera / rojo caído).
fn state_role(name: &str) -> Role {
    match name {
        "run" | "ready" | "Run" | "Ready" => Role::Core,
        "dead" | "killed" | "Dead" | "Killed" => Role::Fault,
        _ => Role::Warn, // blocked/sleeping/waiting
    }
}

/// Color del uso de pila por umbral (verde <70 % / ámbar / rojo ≥90 %).
fn stack_role(pct: u32) -> Role {
    if pct >= 90 {
        Role::Fault
    } else if pct >= 70 {
        Role::Warn
    } else {
        Role::Core
    }
}

/// `scar` → post-mortem del último fault contenido. action 0=leer, 1=borrar.
/// En el tier full la cicatriz persistente se consume al arrancar; aquí se
/// reporta la telemetría viva del kernel. `clear` es un no-op (retorna 0).
fn hook_scar(action: u8, out: &mut [u8]) -> i32 {
    if action == 1 {
        return 0; // nada que borrar: la copia persistente ya se consumió al boot.
    }
    let mut p = Painter::new(out);
    p.header("scar");
    let total = rugus_kernel::total_faults();
    p.kvn("arranques", Role::Data, rugus_kernel::boot_count())
        .raw("   ");
    p.kvn(
        "faults",
        if total == 0 { Role::Core } else { Role::Fault },
        total,
    );
    if rugus_kernel::safe_mode() {
        p.raw("   ").badge(Role::Fault, " SAFE-MODE ");
    }
    p.raw("\r\n");
    for idx in 0..rugus_kernel::task_count() {
        let c = rugus_kernel::faults_for(idx);
        if c > 0 {
            p.on(Role::Chrome).raw("  task ").off();
            p.num(Role::Data, idx as u32).raw(": ");
            p.num(Role::Fault, c).text(Role::Chrome, " faults\r\n");
        }
    }
    match rugus_kernel::last_fault() {
        Some((kind, task, pc, addr)) => {
            // Cicatriz del último fault contenido, en rojo (es la herida).
            p.on(Role::Fault).raw("\u{2717} ultimo  ").off();
            p.kvn("kind", Role::Fault, kind as u32).raw("  ");
            p.kvn("task", Role::Data, task as u32).raw("  ");
            p.on(Role::Chrome).raw("pc=").off().text(Role::Data, "0x");
            p.on(Role::Data).hex(pc).off().raw("  ");
            p.on(Role::Chrome).raw("addr=").off().text(Role::Data, "0x");
            p.on(Role::Data).hex(addr).off().raw("\r\n");
        }
        None => {
            p.ok("sin faults registrados\r\n");
        }
    }
    p.len() as i32
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
