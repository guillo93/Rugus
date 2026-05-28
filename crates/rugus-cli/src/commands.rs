//! Comandos CLI v1 — léxico Rugus (ver docs/RUGUS-KERNEL-VISION.md).

use crate::ansi::{self, Write};
use heapless::String;
use rugus_core::syscall::lite::user;

/// Comando reconocido tras parseo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// `cosmos` → sys_info
    Cosmos,
    /// `orbit` → help
    Orbit,
    /// `ecosystem` → sys_status
    Ecosystem,
    /// `moor P N role` → gpio_bind
    Moor {
        /// Puerto
        port: u8,
        /// Pin
        pin: u8,
        /// Índice del rol en la línea original.
        role_off: usize,
        /// Longitud del rol en bytes.
        role_len: usize,
    },
    /// `pulso P N` → gpio_read
    Pulso {
        /// Puerto
        port: u8,
        /// Pin
        pin: u8,
    },
    /// `spark P N` → gpio_write high
    Spark {
        /// Puerto
        port: u8,
        /// Pin
        pin: u8,
    },
    /// `mute P N` → gpio_write low
    Mute {
        /// Porto
        port: u8,
        /// Pin
        pin: u8,
    },
    /// `ripple P N` → gpio_toggle
    Ripple {
        /// Puerto
        port: u8,
        /// Pin
        pin: u8,
    },
    /// `scout [bus]` → bus_scan
    Scout {
        /// 0=I2C1
        bus: u8,
    },
    /// `sonar N` → module_read
    Sonar {
        /// Slot módulo
        slot: u8,
    },
    /// `schema key` → config_get
    Schema {
        /// Longitud clave en line buffer
        /// Longitud de la clave en la línea original.
        key_len: usize,
    },
    /// `scribe key val` → config_set
    Scribe {
        /// Longitud de la clave.
        key_len: usize,
        /// Offset del valor en la línea original.
        val_off: usize,
        /// Longitud del valor.
        val_len: usize,
    },
    /// `seal` → config_commit
    Seal,
    /// `nest` → module_list
    Nest,
    /// `hatch name` → app_reload
    Hatch {
        /// Offset del nombre en la línea original.
        name_off: usize,
        /// Longitud del nombre.
        name_len: usize,
    },
    /// `coil` → task_list
    Coil,
    /// `anchor` → sys_failsafe
    Anchor,
    /// `ward [kick]` → wdt
    Ward {
        /// 0=status, 1=kick
        action: u8,
    },
    /// Línea vacía o desconocida.
    Unknown,
}

/// Parsea una línea de entrada (sin `\r\n`). `line` se usa para slices de args.
pub fn parse(line: &str) -> Command {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Command::Unknown;
    }

    let mut parts = trimmed.split_whitespace();
    let verb = parts.next().unwrap_or("");

    match verb {
        "cosmos" => Command::Cosmos,
        "orbit" => Command::Orbit,
        "ecosystem" => Command::Ecosystem,
        "pulso" => parse_gpio_cmd(parts, GpioKind::Pulso),
        "spark" => parse_gpio_cmd(parts, GpioKind::Spark),
        "mute" => parse_gpio_cmd(parts, GpioKind::Mute),
        "ripple" => parse_gpio_cmd(parts, GpioKind::Ripple),
        "moor" => parse_moor(trimmed),
        "scout" => {
            let bus = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            Command::Scout { bus }
        }
        "sonar" => {
            let slot = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            Command::Sonar { slot }
        }
        "schema" => {
            let key = parts.next().unwrap_or("");
            if key.is_empty() {
                Command::Unknown
            } else {
                let key_start = trimmed.find(key).unwrap_or(0);
                Command::Schema {
                    key_len: key.len().min(trimmed.len() - key_start),
                }
            }
        }
        "scribe" => parse_scribe(trimmed),
        "seal" => Command::Seal,
        "nest" => Command::Nest,
        "hatch" => parse_hatch(trimmed),
        "coil" => Command::Coil,
        "anchor" => Command::Anchor,
        "ward" => {
            let action = match parts.next() {
                Some("kick") => 1,
                _ => 0,
            };
            Command::Ward { action }
        }
        _ => Command::Unknown,
    }
}

fn parse_moor(line: &str) -> Command {
    let mut parts = line.split_whitespace();
    let _ = parts.next();
    let port_s = parts.next();
    let pin_s = parts.next();
    let role = parts.next();
    let (Some(port_s), Some(pin_s), Some(role)) = (port_s, pin_s, role) else {
        return Command::Unknown;
    };
    let Some(port) = parse_port(port_s) else {
        return Command::Unknown;
    };
    let Some(pin) = parse_pin(pin_s) else {
        return Command::Unknown;
    };
    let role_start = line.find(role).unwrap_or(0);
    Command::Moor {
        port,
        pin,
        role_off: role_start,
        role_len: role.len(),
    }
}

fn parse_scribe(line: &str) -> Command {
    let mut parts = line.split_whitespace();
    let _ = parts.next();
    let key = parts.next().unwrap_or("");
    let val = parts.next().unwrap_or("");
    if key.is_empty() || val.is_empty() {
        return Command::Unknown;
    }
    let val_start = line.find(val).unwrap_or(0);
    Command::Scribe {
        key_len: key.len(),
        val_off: val_start,
        val_len: val.len(),
    }
}

fn parse_hatch(line: &str) -> Command {
    let mut parts = line.split_whitespace();
    let _ = parts.next();
    let name = parts.next().unwrap_or("");
    if name.is_empty() {
        return Command::Unknown;
    }
    let off = line.find(name).unwrap_or(0);
    Command::Hatch {
        name_off: off,
        name_len: name.len(),
    }
}

enum GpioKind {
    Pulso,
    Spark,
    Mute,
    Ripple,
}

fn parse_gpio_cmd<'a, I>(mut parts: I, kind: GpioKind) -> Command
where
    I: Iterator<Item = &'a str>,
{
    let port_s = parts.next();
    let pin_s = parts.next();
    let (Some(port_s), Some(pin_s)) = (port_s, pin_s) else {
        return Command::Unknown;
    };
    let Some(port) = parse_port(port_s) else {
        return Command::Unknown;
    };
    let Some(pin) = parse_pin(pin_s) else {
        return Command::Unknown;
    };
    match kind {
        GpioKind::Pulso => Command::Pulso { port, pin },
        GpioKind::Spark => Command::Spark { port, pin },
        GpioKind::Mute => Command::Mute { port, pin },
        GpioKind::Ripple => Command::Ripple { port, pin },
    }
}

fn parse_port(s: &str) -> Option<u8> {
    let s = s.trim();
    if s.len() == 1 {
        let c = s.as_bytes()[0];
        if c.is_ascii_alphabetic() {
            return Some(c.to_ascii_uppercase());
        }
    }
    None
}

fn parse_pin(s: &str) -> Option<u8> {
    let n: u32 = s.parse().ok()?;
    if n <= 15 {
        Some(n as u8)
    } else {
        None
    }
}

/// Ejecuta un comando. `line` es la línea original para args compuestos.
pub fn execute(cmd: Command, line: &str, out: &mut dyn Write) {
    match cmd {
        Command::Cosmos => exec_cosmos(out),
        Command::Orbit => exec_orbit(out),
        Command::Ecosystem => exec_ecosystem(out),
        Command::Pulso { port, pin } => exec_pulso(out, port, pin),
        Command::Spark { port, pin } => exec_gpio_write(out, port, pin, true, "spark"),
        Command::Mute { port, pin } => exec_gpio_write(out, port, pin, false, "mute"),
        Command::Ripple { port, pin } => exec_ripple(out, port, pin),
        Command::Moor {
            port,
            pin,
            role_off,
            role_len,
        } => {
            let role = &line.as_bytes()[role_off..role_off + role_len];
            let ret = user::gpio_bind(port, pin, role);
            if ret == 0 {
                let _ = out.write_str("moor OK\r\n");
            } else {
                let _ = out.write_str("moor: error\r\n");
            }
        }
        Command::Scout { bus } => exec_scout(out, bus),
        Command::Sonar { slot } => exec_sonar(out, slot),
        Command::Schema { key_len } => {
            let key = extract_key(line, key_len);
            exec_schema(out, key);
        }
        Command::Scribe {
            key_len,
            val_off,
            val_len,
        } => {
            let key = extract_key(line, key_len);
            let val = &line.as_bytes()[val_off..val_off + val_len];
            exec_scribe(out, key, val);
        }
        Command::Seal => exec_seal(out),
        Command::Nest => exec_nest(out),
        Command::Hatch {
            name_off,
            name_len,
        } => {
            let name = &line.as_bytes()[name_off..name_off + name_len];
            exec_hatch(out, name);
        }
        Command::Coil => exec_coil(out),
        Command::Anchor => exec_anchor(out),
        Command::Ward { action } => exec_ward(out, action),
        Command::Unknown => {
            let _ = out.write_str("?\r\n");
        }
    }
}

fn extract_key(line: &str, key_len: usize) -> &[u8] {
    let trimmed = line.trim();
    let start = trimmed
        .find(' ')
        .map(|i| trimmed[i + 1..].find(' ').map(|j| i + 1 + j).unwrap_or(i + 1))
        .unwrap_or(0);
    &trimmed.as_bytes()[start..start + key_len.min(trimmed.len() - start)]
}

fn exec_cosmos(out: &mut dyn Write) {
    ansi::cosmos_banner(out);
    write_syscall_buf(out, user::sys_info);
}

fn exec_ecosystem(out: &mut dyn Write) {
    write_syscall_buf(out, user::sys_status);
}

fn exec_orbit(out: &mut dyn Write) {
    ansi::orbit_banner(out);
    let _ = out.write_str("cosmos orbit ecosystem moor pulso spark mute ripple\r\n");
    let _ = out.write_str("scout sonar schema scribe seal nest hatch coil anchor ward\r\n");
}

fn exec_pulso(out: &mut dyn Write, port: u8, pin: u8) {
    let ret = user::gpio_read(port, pin);
    if ret == 0 || ret == 1 {
        let _ = write_port_pin(out, port, pin);
        let _ = out.write_str(": ");
        let _ = out.write_str(if ret == 1 { "high\r\n" } else { "low\r\n" });
    } else {
        let _ = out.write_str("pulso: error\r\n");
    }
}

fn exec_gpio_write(out: &mut dyn Write, port: u8, pin: u8, high: bool, label: &str) {
    let ret = user::gpio_write(port, pin, high);
    if ret == 0 {
        let _ = out.write_str(label);
        let _ = write_port_pin(out, port, pin);
        let _ = out.write_str(if high { " → high\r\n" } else { " → low\r\n" });
    } else {
        let _ = out.write_str(label);
        let _ = out.write_str(": error\r\n");
    }
}

fn exec_ripple(out: &mut dyn Write, port: u8, pin: u8) {
    if user::gpio_toggle(port, pin) == 0 {
        let _ = out.write_str("ripple ");
        let _ = write_port_pin(out, port, pin);
        let _ = out.write_str(" OK\r\n");
    } else {
        let _ = out.write_str("ripple: error\r\n");
    }
}

fn exec_scout(out: &mut dyn Write, bus: u8) {
    let mut buf = [0u8; 128];
    let n = user::bus_scan(bus, &mut buf);
    if n > 0 {
        let text = core::str::from_utf8(&buf[..n as usize]).unwrap_or("(invalid)");
        let _ = out.write_str("scout: ");
        let _ = out.write_str(text);
        if !text.ends_with("\r\n") {
            let _ = out.write_str("\r\n");
        }
    } else {
        let _ = out.write_str("scout: (none)\r\n");
    }
}

fn exec_sonar(out: &mut dyn Write, slot: u8) {
    let mut buf = [0u8; 128];
    let n = user::module_read(slot, &mut buf);
    if n > 0 {
        let text = core::str::from_utf8(&buf[..n as usize]).unwrap_or("(invalid)");
        let _ = out.write_str(text);
        if !text.ends_with("\r\n") {
            let _ = out.write_str("\r\n");
        }
    } else {
        let _ = out.write_str("sonar: error\r\n");
    }
}

fn exec_schema(out: &mut dyn Write, key: &[u8]) {
    let mut buf = [0u8; 128];
    let n = user::config_get(key, &mut buf);
    if n > 0 {
        let text = core::str::from_utf8(&buf[..n as usize]).unwrap_or("(invalid)");
        let _ = out.write_str(text);
        if !text.ends_with("\r\n") {
            let _ = out.write_str("\r\n");
        }
    } else {
        let _ = out.write_str("schema: (missing)\r\n");
    }
}

fn exec_scribe(out: &mut dyn Write, key: &[u8], val: &[u8]) {
    if user::config_set(key, val) == 0 {
        let _ = out.write_str("scribe OK\r\n");
    } else {
        let _ = out.write_str("scribe: error\r\n");
    }
}

fn exec_seal(out: &mut dyn Write) {
    if user::config_commit() == 0 {
        let _ = out.write_str("seal OK\r\n");
    } else {
        let _ = out.write_str("seal: error\r\n");
    }
}

fn exec_nest(out: &mut dyn Write) {
    let mut buf = [0u8; 128];
    let n = user::module_list(&mut buf);
    if n > 0 {
        let text = core::str::from_utf8(&buf[..n as usize]).unwrap_or("(invalid)");
        let _ = out.write_str(text);
        if !text.ends_with("\r\n") {
            let _ = out.write_str("\r\n");
        }
    } else {
        let _ = out.write_str("nest: (empty)\r\n");
    }
}

fn exec_hatch(out: &mut dyn Write, name: &[u8]) {
    if user::app_reload(name) == 0 {
        let _ = out.write_str("hatch OK\r\n");
    } else {
        let _ = out.write_str("hatch: error\r\n");
    }
}

fn exec_coil(out: &mut dyn Write) {
    let mut buf = [0u8; 256];
    let n = user::task_list(&mut buf);
    if n > 0 {
        let text = core::str::from_utf8(&buf[..n as usize]).unwrap_or("(invalid)");
        let _ = out.write_str(text);
        if !text.ends_with("\r\n") {
            let _ = out.write_str("\r\n");
        }
    } else {
        let _ = out.write_str("coil: (none)\r\n");
    }
}

fn exec_anchor(out: &mut dyn Write) {
    if user::sys_failsafe() == 0 {
        let _ = out.write_str("anchor: fail-safe ACTIVE\r\n");
    } else {
        let _ = out.write_str("anchor: error\r\n");
    }
}

fn exec_ward(out: &mut dyn Write, action: u8) {
    let ret = user::wdt(action);
    if action == 0 {
        let _ = out.write_str(if ret == 1 {
            "ward: armed\r\n"
        } else {
            "ward: disarmed\r\n"
        });
    } else if ret == 0 {
        let _ = out.write_str("ward: kick OK\r\n");
    } else {
        let _ = out.write_str("ward: error\r\n");
    }
}

fn write_syscall_buf(out: &mut dyn Write, f: fn(&mut [u8]) -> i32) {
    let mut buf = [0u8; 256];
    let n = f(&mut buf);
    if n > 0 {
        let text = core::str::from_utf8(&buf[..n as usize]).unwrap_or("(invalid utf8)");
        let _ = out.write_str(text);
        if !text.ends_with("\r\n") {
            let _ = out.write_str("\r\n");
        }
    } else {
        let _ = out.write_str("error\r\n");
    }
}

fn write_port_pin(out: &mut dyn Write, port: u8, pin: u8) -> Result<(), ()> {
    let _ = out.write_str("P");
    let _ = out.write_str(core::str::from_utf8(&[port]).unwrap_or("?"));
    let _ = out.write_str(" ");
    let mut s: String<4> = String::new();
    let _ = s.push_str(u32::from(pin).to_string().as_str());
    out.write_str(s.as_str())
}

trait ToString {
    fn to_string(self) -> heapless::String<16>;
}

impl ToString for u32 {
    fn to_string(self) -> heapless::String<16> {
        let mut s: heapless::String<16> = heapless::String::new();
        if self == 0 {
            let _ = s.push('0');
            return s;
        }
        let mut n = self;
        let mut digits = [0u8; 10];
        let mut i = 0;
        while n > 0 {
            digits[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
        while i > 0 {
            i -= 1;
            let _ = s.push(digits[i] as char);
        }
        s
    }
}
