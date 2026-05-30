//! Estado y hooks del appliance — capa servicio (no CLI).

use crate::heartbeat;
use core::sync::atomic::{AtomicBool, Ordering};

use heapless::String;
use rugus_core::syscall::lite::{GpioLevel, Hooks};
use rugus_core::Errno;
use rugus_hal::SerialPort;
use rugus_hal_stm32f1::gpio_raw;
use rugus_hal_stm32f1::hm20::{self, Hm20Config, InitResult};
use rugus_hal_stm32f1::i2c::I2c1;
use rugus_hal_stm32f1::pac;
use rugus_hal_stm32f1::spi_sd::{SdStatus, Spi1Sd};
use rugus_hal_stm32f1::uart2::Usart2;
use rugus_hal_stm32f1::wdt::Watchdog;
use rugus_rfn::{parse_afr_header, parse_rfn, ConfigMap, MAX_FIELD};
use rush::{identify, Write};

/// Config RFN embebida por defecto (sin SD).
const DEFAULT_RFN: &str =
    "# Rugus lite appliance default\nboard = bluepill\npersonality = lite\nled = C13\n";

static FAILSAFE: AtomicBool = AtomicBool::new(false);
static mut CONFIG: Option<ConfigMap> = None;
static mut I2C: Option<I2c1> = None;
static mut SD: Option<Spi1Sd> = None;
static mut MODULES: Option<Usart2> = None;
static mut WDT: Option<Watchdog> = None;
static mut SCHED_TASK_COUNT: u32 = 0;
static mut MODULE_ECO: Option<&'static str> = None;
static mut MODULE_STATUS: ModuleStatus = ModuleStatus::Idle;
static mut APP_NAME: Option<String<{ MAX_FIELD }>> = None;
static mut IDENT_LINE: [u8; 16] = [0; 16];
static mut IDENT_LEN: usize = 0;

/// Estado de init HM-20 en USART2 (diagnóstico `ecosystem`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ModuleStatus {
    Idle,
    NoAtResponse,
    Hm20AtWarn,
    Hm20Ready,
}

impl ModuleStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::NoAtResponse => "no-at-response",
            Self::Hm20AtWarn => "hm20-at-warn",
            Self::Hm20Ready => "hm20-ready",
        }
    }
}

/// Escritor IDENTIFY sobre USART2 (bus de módulos).
struct ModuleWriter;

impl Write for ModuleWriter {
    fn write_str(&mut self, s: &str) -> Result<(), ()> {
        // SAFETY: solo la tarea CLI cooperativa toca USART2 fuera de los hooks.
        unsafe {
            if let Some(u) = MODULES.as_mut() {
                for &b in s.as_bytes() {
                    u.write_byte(b);
                }
            }
        }
        Ok(())
    }
}

/// Poll no bloqueante del bus de módulos (USART2) para el protocolo IDENTIFY.
///
/// Permite que un host conectado por serie/BLE a través del puente de módulos
/// descubra el dispositivo. Responde a `IDENTIFY\r\n` y al byte de control ENQ.
pub fn poll_identify_usart2() {
    // SAFETY: invocado solo desde la tarea CLI cooperativa.
    let byte = unsafe { MODULES.as_mut().and_then(|u| u.try_read_byte()) };
    let Some(b) = byte else {
        return;
    };

    if b == identify::ENQ {
        identify::write_signature(&mut ModuleWriter, identify::TIER, identify::CHIP);
        return;
    }

    unsafe {
        if b == b'\r' || b == b'\n' {
            if IDENT_LEN > 0 {
                if &IDENT_LINE[..IDENT_LEN] == b"IDENTIFY" {
                    identify::write_signature(&mut ModuleWriter, identify::TIER, identify::CHIP);
                }
                IDENT_LEN = 0;
            }
        } else if IDENT_LEN < IDENT_LINE.len() {
            IDENT_LINE[IDENT_LEN] = b;
            IDENT_LEN += 1;
        } else {
            // Línea sobredimensionada: descartar para evitar falsos positivos.
            IDENT_LEN = 0;
        }
    }
}

/// Inicializa servicios y config staging.
pub fn init(rcc: &pac::RCC, i2c: I2c1, sd: Spi1Sd, modules: Usart2, wdt: Watchdog) {
    let _ = gpio_raw::configure_output(rcc, b'C', 13);
    unsafe {
        I2C = Some(i2c);
        SD = Some(sd);
        MODULES = Some(modules);
        WDT = Some(wdt);
        SCHED_TASK_COUNT = 0;
        CONFIG = Some(ConfigMap::new());
        if let Some(cfg) = CONFIG.as_mut() {
            parse_rfn(DEFAULT_RFN, cfg);
            if let Some(sd) = SD.as_mut() {
                if sd.status() == SdStatus::Ready {
                    heartbeat::note(heartbeat::SD);
                    let mut sector = [0u8; 512];
                    let n = sd.read_boot_sector(&mut sector);
                    if n > 0 {
                        heartbeat::note(heartbeat::SD);
                        if let Ok(text) = core::str::from_utf8(&sector[..n]) {
                            parse_rfn(text, cfg);
                        }
                    }
                }
            }
        }
        if let Some(u) = MODULES.as_mut() {
            match hm20::init_with_kick(u, Hm20Config::default(), kick_wdt) {
                InitResult::Ready => {
                    MODULE_ECO = Some("hm20-ble");
                    MODULE_STATUS = ModuleStatus::Hm20Ready;
                    defmt::info!("hm20 init: ready");
                }
                InitResult::NoResponse => {
                    MODULE_ECO = None;
                    MODULE_STATUS = ModuleStatus::NoAtResponse;
                    defmt::warn!("hm20 init: no-at-response");
                }
                InitResult::AtError => {
                    MODULE_ECO = Some("hm20-ble (AT warn)");
                    MODULE_STATUS = ModuleStatus::Hm20AtWarn;
                    defmt::warn!("hm20 init: at-error");
                }
            }
        }
    }
    defmt::info!("services ok");
}

pub fn set_task_count(n: u32) {
    unsafe {
        SCHED_TASK_COUNT = n;
    }
}

pub fn set_wdt(wdt: Watchdog) {
    unsafe {
        WDT = Some(wdt);
    }
}

pub fn hooks() -> Hooks {
    Hooks {
        sys_info: hook_sys_info,
        sys_status: hook_sys_status,
        gpio_read: hook_gpio_read,
        gpio_write: hook_gpio_write,
        gpio_toggle: hook_gpio_toggle,
        gpio_bind: hook_gpio_bind,
        bus_scan: hook_bus_scan,
        config_get: hook_config_get,
        config_set: hook_config_set,
        config_commit: hook_config_commit,
        module_list: hook_module_list,
        module_read: hook_module_read,
        module_renew: hook_module_renew,
        task_list: hook_task_list,
        app_reload: hook_app_reload,
        sys_failsafe: hook_sys_failsafe,
        wdt: hook_wdt,
    }
}

fn hook_sys_info(out: &mut [u8]) -> usize {
    let msg = "Rugus lite v0.1\r\nboard: F103 Blue Pill\r\n";
    write_bytes(out, msg.as_bytes())
}

fn hook_sys_status(out: &mut [u8]) -> usize {
    let fs = FAILSAFE.load(Ordering::Relaxed);
    let sd_ok = unsafe {
        SD.as_ref()
            .map(|s| s.status() == SdStatus::Ready)
            .unwrap_or(false)
    };
    let mod_status = unsafe { MODULE_STATUS.as_str() };
    let tasks = unsafe { SCHED_TASK_COUNT };
    let mut line: String<128> = String::new();
    let _ = line.push_str("uptime: (cycle counter)\r\n");
    let _ = line.push_str("failsafe: ");
    let _ = line.push_str(if fs { "ON\r\n" } else { "OFF\r\n" });
    let _ = line.push_str("sd: ");
    let _ = line.push_str(if sd_ok { "ready\r\n" } else { "absent\r\n" });
    let _ = line.push_str("usart2: ");
    let _ = line.push_str(mod_status);
    let _ = line.push_str("\r\n");
    let _ = line.push_str("tasks: ");
    push_u32(&mut line, tasks);
    let _ = line.push_str("\r\n");
    write_bytes(out, line.as_bytes())
}

fn hook_gpio_read(port: u8, pin: u8) -> i32 {
    if FAILSAFE.load(Ordering::Relaxed) {
        return Errno::Edenied as i32;
    }
    match gpio_raw::read(port, pin) {
        Some(v) => i32::from(v),
        None => Errno::Einval as i32,
    }
}

fn hook_gpio_write(port: u8, pin: u8, level: GpioLevel) -> i32 {
    if FAILSAFE.load(Ordering::Relaxed) {
        return Errno::Edenied as i32;
    }
    match gpio_raw::write(port, pin, level == GpioLevel::High) {
        Some(()) => 0,
        None => Errno::Einval as i32,
    }
}

fn hook_gpio_toggle(port: u8, pin: u8) -> i32 {
    if FAILSAFE.load(Ordering::Relaxed) {
        return Errno::Edenied as i32;
    }
    match gpio_raw::toggle(port, pin) {
        Some(()) => 0,
        None => Errno::Einval as i32,
    }
}

fn hook_gpio_bind(port: u8, pin: u8, role: &[u8]) -> i32 {
    if FAILSAFE.load(Ordering::Relaxed) {
        return Errno::Edenied as i32;
    }
    let Ok(role_s) = core::str::from_utf8(role) else {
        return Errno::Einval as i32;
    };
    let Ok(key) = heapless::String::<MAX_FIELD>::try_from("bind.pin") else {
        return Errno::Enomem as i32;
    };
    let mut val: heapless::String<{ MAX_FIELD }> = heapless::String::new();
    let _ = val.push(port as char);
    let _ = val.push('.');
    push_u32_str(&mut val, u32::from(pin));
    let _ = val.push('=');
    let _ = val.push_str(role_s);
    unsafe {
        if let Some(cfg) = CONFIG.as_mut() {
            let _ = cfg.insert(key, val);
        }
    }
    0
}

fn hook_bus_scan(bus: u8, out: &mut [u8]) -> i32 {
    if bus != 0 {
        return write_bytes(out, b"I2C bus 0 only\r\n") as i32;
    }
    unsafe {
        if let Some(i2c) = I2C.as_mut() {
            let mut addrs = [0u8; 16];
            let n = i2c.scan(&mut addrs);
            heartbeat::note(heartbeat::I2C);
            let mut pos = 0;
            for addr in &addrs[..n] {
                let line = format_addr(*addr);
                let b = line.as_bytes();
                if pos + b.len() >= out.len() {
                    break;
                }
                out[pos..pos + b.len()].copy_from_slice(b);
                pos += b.len();
            }
            if pos == 0 {
                return write_bytes(out, b"(none)\r\n") as i32;
            }
            return pos as i32;
        }
    }
    Errno::Ebusy as i32
}

fn hook_config_get(key: &[u8], out: &mut [u8]) -> i32 {
    let Ok(k) = core::str::from_utf8(key) else {
        return Errno::Einval as i32;
    };
    unsafe {
        if let Some(cfg) = CONFIG.as_ref() {
            if let Some(v) = cfg.get(k) {
                return write_bytes(out, v.as_bytes()) as i32;
            }
        }
    }
    0
}

fn hook_config_set(key: &[u8], val: &[u8]) -> i32 {
    if FAILSAFE.load(Ordering::Relaxed) {
        return Errno::Edenied as i32;
    }
    let Ok(k) = core::str::from_utf8(key) else {
        return Errno::Einval as i32;
    };
    let Ok(vs) = core::str::from_utf8(val) else {
        return Errno::Einval as i32;
    };
    let Ok(kk) = heapless::String::<MAX_FIELD>::try_from(k) else {
        return Errno::Einval as i32;
    };
    let Ok(vv) = heapless::String::<MAX_FIELD>::try_from(vs) else {
        return Errno::Einval as i32;
    };
    unsafe {
        if let Some(cfg) = CONFIG.as_mut() {
            let _ = cfg.insert(kk, vv);
        }
    }
    0
}

fn hook_config_commit() -> i32 {
    if FAILSAFE.load(Ordering::Relaxed) {
        return Errno::Edenied as i32;
    }
    // Phase 3: persist to SD when ready; staging validated in RAM.
    0
}

fn hook_module_list(out: &mut [u8]) -> i32 {
    unsafe {
        if let Some(eco) = MODULE_ECO {
            let mut line = heapless::String::<64>::new();
            let _ = line.push_str("slot0: usart2 (");
            let _ = line.push_str(eco);
            let _ = line.push_str(")\r\n");
            return write_bytes(out, line.as_bytes()) as i32;
        }
    }
    write_bytes(out, b"(no modules)\r\n") as i32
}

fn hook_module_renew() -> i32 {
    unsafe {
        if let Some(u) = MODULES.as_mut() {
            let result = hm20::factory_renew(u, Hm20Config::default(), kick_wdt);
            match result {
                InitResult::Ready => {
                    MODULE_ECO = Some("hm20-ble");
                    MODULE_STATUS = ModuleStatus::Hm20Ready;
                    return 0;
                }
                InitResult::NoResponse => {
                    MODULE_ECO = None;
                    MODULE_STATUS = ModuleStatus::NoAtResponse;
                    return Errno::Ebusy as i32;
                }
                InitResult::AtError => {
                    MODULE_ECO = Some("hm20-ble (AT warn)");
                    MODULE_STATUS = ModuleStatus::Hm20AtWarn;
                    return Errno::Einval as i32;
                }
            }
        }
    }
    Errno::Ebusy as i32
}

fn hook_module_read(slot: u8, out: &mut [u8]) -> i32 {
    if slot != 0 {
        return Errno::Einval as i32;
    }
    unsafe {
        if let Some(u) = MODULES.as_mut() {
            let _ = u.write(b"AT");
            let mut pos = 0;
            for _ in 0..500 {
                kick_wdt();
                if let Some(b) = u.try_read_byte() {
                    if pos < out.len() {
                        out[pos] = b;
                        pos += 1;
                    }
                } else {
                    cortex_m::asm::delay(500);
                }
            }
            if pos == 0 {
                return write_bytes(out, b"(no response)\r\n") as i32;
            }
            return pos as i32;
        }
    }
    Errno::Ebusy as i32
}

fn kick_wdt() {
    unsafe {
        let ptr = crate::IWDG_PTR;
        if !ptr.is_null() {
            (&(*ptr)).kr.write(|w| w.key().bits(0xAAAA));
        }
    }
}

fn hook_task_list(out: &mut [u8]) -> i32 {
    let n = unsafe { SCHED_TASK_COUNT };
    let mut line: String<128> = String::new();
    let _ = line.push_str("id name\r\n");
    if n >= 1 {
        let _ = line.push_str("1 cli\r\n");
    }
    if n >= 2 {
        let _ = line.push_str("2 heartbeat\r\n");
    }
    write_bytes(out, line.as_bytes()) as i32
}

fn hook_app_reload(name: &[u8]) -> i32 {
    if FAILSAFE.load(Ordering::Relaxed) {
        return Errno::Edenied as i32;
    }
    let Ok(name_s) = core::str::from_utf8(name) else {
        return Errno::Einval as i32;
    };
    // Try embedded AFR header stub
    let stub = "app.name = demo\napp.version = 0.1.0\n";
    if let Some(hdr) = parse_afr_header(stub) {
        if hdr.name.as_str() == name_s || name_s == "demo" {
            unsafe {
                APP_NAME = Some(hdr.name);
            }
            return 0;
        }
    }
    Errno::Einval as i32
}

fn hook_sys_failsafe(action: u8) -> i32 {
    if action == 0 {
        FAILSAFE.store(true, Ordering::Relaxed);
        let _ = gpio_raw::write(b'C', 13, true);
    } else {
        FAILSAFE.store(false, Ordering::Relaxed);
        let _ = gpio_raw::write(b'C', 13, false);
    }
    0
}

fn hook_wdt(action: u8) -> i32 {
    if action == 0 {
        return unsafe {
            if WDT.as_ref().map(|w| w.is_armed()).unwrap_or(false) {
                1
            } else {
                0
            }
        };
    }
    unsafe {
        let ptr = crate::IWDG_PTR;
        if !ptr.is_null() {
            (&(*ptr)).kr.write(|w| w.key().bits(0xAAAA));
            return 0;
        }
    }
    Errno::Einval as i32
}

fn write_bytes(out: &mut [u8], data: &[u8]) -> usize {
    let n = data.len().min(out.len());
    out[..n].copy_from_slice(&data[..n]);
    n
}

fn format_addr(addr: u8) -> heapless::String<16> {
    let mut s: heapless::String<16> = heapless::String::new();
    let _ = s.push_str("0x");
    push_hex_byte(&mut s, addr);
    let _ = s.push_str("\r\n");
    s
}

fn push_hex_byte(s: &mut heapless::String<16>, b: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let _ = s.push(HEX[(b >> 4) as usize] as char);
    let _ = s.push(HEX[(b & 0xF) as usize] as char);
}

fn push_u32(s: &mut heapless::String<128>, n: u32) {
    let mut buf = [0u8; 10];
    let mut i = 0;
    let mut v = n;
    if v == 0 {
        let _ = s.push('0');
        return;
    }
    while v > 0 {
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        let _ = s.push(buf[i] as char);
    }
}

fn push_u32_str(s: &mut heapless::String<{ MAX_FIELD }>, n: u32) {
    let mut v = n;
    if v == 0 {
        let _ = s.push('0');
        return;
    }
    let mut buf = [0u8; 4];
    let mut i = 0;
    while v > 0 {
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        let _ = s.push(buf[i] as char);
    }
}
