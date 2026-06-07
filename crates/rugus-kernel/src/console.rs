//! Consola de operador interactiva sobre un puerto serie (F4.5).
//!
//! ## Qué resuelve
//!
//! Hasta ahora el operador solo podía *observar* el kernel por RTT (defmt). Esta
//! consola añade **interacción**: una línea de comandos sobre UART que inspecciona
//! el estado vivo del kernel (tareas, memoria, faults) y actúa sobre él (respawn,
//! reboot) sin depender del debugger. Es la herramienta de operación de campo de
//! un equipo embebido industrial.
//!
//! ## Diseño
//!
//! - **Sin bloqueo, dirigido por bytes**: la placa recibe cada byte por IRQ de
//!   RX y lo entrega a [`Console::feed`]. La consola acumula la línea, hace eco y,
//!   al recibir Enter, parsea y ejecuta. Nada bloquea el supervisor.
//! - **Salida invertida por trait** ([`ConsoleOut`]): la consola escribe por un
//!   `write_str` infalible que la placa implementa sobre su UART; igual patrón que
//!   [`crate::status::StatusLeds`] y [`crate::FaultObserver`]. El kernel no se
//!   acopla a un periférico concreto.
//! - **Sin `alloc` ni `defmt`**: el formato de números va por un buffer en pila;
//!   la consola es utilizable aunque el heap no esté inicializado.
//!
//! ## Comandos
//!
//! - `help` — lista de comandos.
//! - `ps` — tabla de tareas (idx, prioridad, modo, estado, stack usado/total).
//! - `mem` — uso del heap (usado/libre/total).
//! - `faults` — telemetría persistente (arranques, faults totales, por tarea,
//!   último post-mortem, safe-mode, causa del último reset).
//! - `respawn <n>` — revive la tarea `n` si está `KILL`.
//! - `reboot` — reset del sistema (`SCB.AIRCR.SYSRESETREQ`).

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use rugus_core::sched::MAX_TASKS;

/// Capacidad del anillo de recepción (potencia de 2 para enmascarar el índice).
const RX_RING_CAP: usize = 128;

/// Anillo SPSC sin bloqueo entre el handler de IRQ de RX (único productor) y la
/// tarea supervisora (único consumidor).
///
/// El productor escribe en `head` y el consumidor lee en `tail`; cada lado
/// publica su avance con `Release` y observa el del otro con `Acquire`. Si el
/// anillo está lleno se descarta el byte más reciente (un overrun de consola no
/// debe corromper la cola). Pensado para vivir en un `static` y ser compartido
/// entre el `#[interrupt]` y el bucle del supervisor.
pub struct RxRing {
    buf: [AtomicU8; RX_RING_CAP],
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl Default for RxRing {
    fn default() -> Self {
        Self::new()
    }
}

impl RxRing {
    /// Crea un anillo vacío (usable en contexto `const` para un `static`).
    pub const fn new() -> Self {
        Self {
            buf: [const { AtomicU8::new(0) }; RX_RING_CAP],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Encola un byte desde el productor (ISR). Devuelve `false` si estaba lleno
    /// (el byte se descarta sin corromper la cola).
    pub fn push(&self, b: u8) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % RX_RING_CAP;
        if next == self.tail.load(Ordering::Acquire) {
            return false; // lleno
        }
        self.buf[head].store(b, Ordering::Relaxed);
        self.head.store(next, Ordering::Release);
        true
    }

    /// Saca un byte desde el consumidor (supervisor). `None` si está vacío.
    pub fn pop(&self) -> Option<u8> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None; // vacío
        }
        let b = self.buf[tail].load(Ordering::Relaxed);
        self.tail.store((tail + 1) % RX_RING_CAP, Ordering::Release);
        Some(b)
    }
}

/// Sumidero de salida de la consola: un `write_str` infalible que la placa
/// implementa sobre su UART (traga el error de transmisión; un byte perdido en la
/// consola no debe abortar el supervisor).
pub trait ConsoleOut {
    /// Escribe la cadena completa al puerto (bloqueante a nivel de byte está bien;
    /// las cadenas son cortas).
    fn write_str(&mut self, s: &str);
}

/// Capacidad de la línea de edición. Un comando de consola realista cabe de
/// sobra; los bytes que excedan se descartan (sin desbordar).
const LINE_CAP: usize = 64;

/// Consola de línea: acumula bytes hasta Enter, luego parsea y ejecuta.
///
/// No tiene estado de hardware; la placa la conduce con los bytes que llegan por
/// IRQ de RX. Reutilizable entre placas.
pub struct Console {
    buf: [u8; LINE_CAP],
    len: usize,
    /// `true` una vez emitida la bienvenida/prompt inicial.
    greeted: bool,
}

impl Default for Console {
    fn default() -> Self {
        Self::new()
    }
}

impl Console {
    /// Crea una consola con la línea vacía.
    pub const fn new() -> Self {
        Self {
            buf: [0; LINE_CAP],
            len: 0,
            greeted: false,
        }
    }

    /// Emite el banner de bienvenida y el primer prompt (una sola vez).
    pub fn greet(&mut self, out: &mut impl ConsoleOut) {
        if !self.greeted {
            out.write_str("\r\nRugus console. Escribe 'help'.\r\n> ");
            self.greeted = true;
        }
    }

    /// Procesa un byte recibido. Hace eco, edita la línea y, al recibir Enter
    /// (`\r` o `\n`), ejecuta el comando acumulado y reimprime el prompt.
    pub fn feed(&mut self, byte: u8, out: &mut impl ConsoleOut) {
        match byte {
            b'\r' | b'\n' => {
                out.write_str("\r\n");
                self.execute(out);
                self.len = 0;
                out.write_str("> ");
            }
            // Backspace / DEL: borra el último carácter si lo hay.
            0x08 | 0x7F if self.len > 0 => {
                self.len -= 1;
                // Borra visualmente: retrocede, espacio, retrocede.
                out.write_str("\x08 \x08");
            }
            // Imprimibles ASCII: acumula y hace eco (ignora si la línea está llena).
            0x20..=0x7E if self.len < LINE_CAP => {
                self.buf[self.len] = byte;
                self.len += 1;
                // Eco del carácter (un byte como str).
                let one = [byte];
                // SAFETY: byte está en 0x20..=0x7E, ASCII imprimible => UTF-8 válido.
                out.write_str(unsafe { core::str::from_utf8_unchecked(&one) });
            }
            // Control no manejado, o byte imprimible con la línea llena: se ignora.
            _ => {}
        }
    }

    /// Parsea y ejecuta la línea acumulada.
    fn execute(&mut self, out: &mut impl ConsoleOut) {
        // SAFETY: solo acumulamos ASCII imprimible en `buf`, así que es UTF-8.
        let line = unsafe { core::str::from_utf8_unchecked(&self.buf[..self.len]) };
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        // Separa comando y argumento (un solo argumento basta para `respawn`).
        let mut parts = line.split_ascii_whitespace();
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next();
        match cmd {
            "help" => cmd_help(out),
            "ps" => cmd_ps(out),
            "mem" => cmd_mem(out),
            "faults" => cmd_faults(out),
            "power" => cmd_power(out),
            "respawn" => cmd_respawn(out, arg),
            "reboot" => cmd_reboot(out),
            _ => {
                out.write_str("comando desconocido: ");
                out.write_str(cmd);
                out.write_str(" (prueba 'help')\r\n");
            }
        }
    }
}

fn cmd_help(out: &mut impl ConsoleOut) {
    out.write_str(
        "comandos:\r\n\
         \x20 help          esta ayuda\r\n\
         \x20 ps            tareas (idx pri modo estado stack)\r\n\
         \x20 mem           uso de heap\r\n\
         \x20 faults        telemetria de faults + safe-mode\r\n\
         \x20 power         energia: uptime, idle %, systick, stop\r\n\
         \x20 respawn <n>   revive la tarea n si esta KILL\r\n\
         \x20 reboot        reset del sistema\r\n",
    );
}

fn cmd_ps(out: &mut impl ConsoleOut) {
    out.write_str("idx pri modo  estado stack\r\n");
    let n = crate::task_count();
    let mut buf = NumBuf::new();
    for idx in 0..n {
        // idx
        out.write_str(buf.fmt(idx as u32));
        out.write_str("   ");
        // prioridad
        out.write_str(buf.fmt(crate::task_priority(idx) as u32));
        out.write_str("   ");
        // modo
        out.write_str(if crate::is_user_task(idx) {
            "user "
        } else {
            "kern "
        });
        out.write_str(" ");
        // estado
        let st = crate::task_state_name(idx);
        out.write_str(st);
        pad(out, st.len(), 6);
        out.write_str(" ");
        // stack usado/total
        out.write_str(buf.fmt(crate::stack_high_water(idx)));
        out.write_str("/");
        out.write_str(buf.fmt(crate::stack_len(idx)));
        out.write_str("\r\n");
    }
}

fn cmd_mem(out: &mut impl ConsoleOut) {
    let mut buf = NumBuf::new();
    out.write_str("heap usado=");
    out.write_str(buf.fmt(crate::heap_used() as u32));
    out.write_str(" libre=");
    out.write_str(buf.fmt(crate::heap_free() as u32));
    out.write_str(" total=");
    out.write_str(buf.fmt(crate::heap_size() as u32));
    out.write_str(" bytes\r\n");
}

fn cmd_faults(out: &mut impl ConsoleOut) {
    let mut buf = NumBuf::new();
    out.write_str("arranques=");
    out.write_str(buf.fmt(crate::boot_count()));
    out.write_str(" faults_total=");
    out.write_str(buf.fmt(crate::total_faults()));
    out.write_str(if crate::safe_mode() {
        " [SAFE-MODE]"
    } else {
        ""
    });
    out.write_str("\r\n");
    // Causa del último reset (F4.6): power-on / iwdg / software / pin / brownout.
    out.write_str("ultimo reset: ");
    out.write_str(crate::reset_cause());
    out.write_str("\r\n");
    for idx in 0..crate::task_count() {
        let c = crate::faults_for(idx);
        if c > 0 {
            out.write_str("  task ");
            out.write_str(buf.fmt(idx as u32));
            out.write_str(": ");
            out.write_str(buf.fmt(c));
            out.write_str(" faults\r\n");
        }
    }
    if let Some((kind, task, pc, addr)) = crate::last_fault() {
        out.write_str("ultimo: kind=");
        out.write_str(buf.fmt(kind as u32));
        out.write_str(" task=");
        out.write_str(buf.fmt(task as u32));
        out.write_str(" pc=0x");
        out.write_str(buf.fmt_hex(pc));
        out.write_str(" addr=0x");
        out.write_str(buf.fmt_hex(addr));
        out.write_str("\r\n");
    } else {
        out.write_str("sin faults registrados\r\n");
    }
}

fn cmd_power(out: &mut impl ConsoleOut) {
    let Some(p) = crate::power_stats() else {
        out.write_str("energia no disponible (sin proveedor)\r\n");
        return;
    };
    let mut buf = NumBuf::new();
    out.write_str("uptime=");
    out.write_str(buf.fmt(p.uptime_ms));
    out.write_str(" ms  idle=");
    out.write_str(buf.fmt(p.idle_ms));
    out.write_str(" ms (");
    out.write_str(buf.fmt(p.idle_percent()));
    out.write_str("%)\r\n");
    out.write_str("systick_irqs=");
    out.write_str(buf.fmt(p.systick_irqs));
    out.write_str("  stop_entries=");
    out.write_str(buf.fmt(p.stop_entries));
    out.write_str("\r\n");
}

fn cmd_respawn(out: &mut impl ConsoleOut, arg: Option<&str>) {
    let Some(idx) = arg.and_then(|a| a.parse::<usize>().ok()) else {
        out.write_str("uso: respawn <n>\r\n");
        return;
    };
    if idx >= crate::task_count() {
        out.write_str("indice fuera de rango\r\n");
        return;
    }
    if crate::respawn(idx) {
        out.write_str("tarea revivida\r\n");
    } else {
        out.write_str("no revivible (no estaba KILL)\r\n");
    }
}

fn cmd_reboot(out: &mut impl ConsoleOut) {
    out.write_str("reiniciando...\r\n");
    crate::reboot();
}

/// Rellena con espacios hasta `width` columnas tras una cadena de `len` chars.
fn pad(out: &mut impl ConsoleOut, len: usize, width: usize) {
    let mut rem = width.saturating_sub(len);
    while rem > 0 {
        out.write_str(" ");
        rem -= 1;
    }
}

/// Buffer en pila para formatear enteros sin `alloc`.
struct NumBuf {
    bytes: [u8; 10],
}

impl NumBuf {
    fn new() -> Self {
        Self { bytes: [0; 10] }
    }

    /// Formatea `v` en decimal y devuelve el `&str` (válido hasta el siguiente uso).
    fn fmt(&mut self, mut v: u32) -> &str {
        let mut i = self.bytes.len();
        if v == 0 {
            i -= 1;
            self.bytes[i] = b'0';
        } else {
            while v > 0 && i > 0 {
                i -= 1;
                self.bytes[i] = b'0' + (v % 10) as u8;
                v /= 10;
            }
        }
        // SAFETY: solo dígitos ASCII escritos en [i, len).
        unsafe { core::str::from_utf8_unchecked(&self.bytes[i..]) }
    }

    /// Formatea `v` en hexadecimal (sin prefijo) y devuelve el `&str`.
    fn fmt_hex(&mut self, mut v: u32) -> &str {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut i = self.bytes.len();
        if v == 0 {
            i -= 1;
            self.bytes[i] = b'0';
        } else {
            while v > 0 && i > 0 {
                i -= 1;
                self.bytes[i] = HEX[(v & 0xF) as usize];
                v >>= 4;
            }
        }
        // SAFETY: solo dígitos hex ASCII escritos en [i, len).
        unsafe { core::str::from_utf8_unchecked(&self.bytes[i..]) }
    }
}

// Aserto de compilación: el formato decimal de u32 (máx. 10 dígitos) cabe.
const _: () = assert!(u32::MAX.ilog10() < 10);
// MAX_TASKS debe caber en un dígito para que la tabla `ps` quede alineada.
const _: () = assert!(MAX_TASKS <= 9);
