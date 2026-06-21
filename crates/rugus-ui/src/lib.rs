//! `rugus-ui` — sistema visual de la consola `rush`.
//!
//! La consola nativa de Rugus no se parece a la de otros OS por diseño: paleta
//! semántica (cada color *significa* algo), componentes compactos (badges,
//! tablas, medidores) y dos fidelidades —rica (ANSI 256) y plana (7-bit)— con la
//! **misma silueta**. Todo `no_std`, sin `alloc`: la salida se compone sobre un
//! `&mut [u8]` con el [`Painter`], que solo emite secuencias de escape si el
//! color está activo.
//!
//! Capas que dependen de este crate:
//! - `rugus-personality-full` formatea los verbos (`cosmos`/`coil`/…).
//! - `rush` pinta el prompt, el banner y el feedback de autenticación.
//!
//! El tier *lite* (F103, pila de consola de ~1,5 KiB) **no** usa este sistema:
//! mantiene su formato compacto sin color por presupuesto de RAM. El sistema
//! rico es para el tier *full* (F407/F769/RPi), donde el silicio lo permite.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

use core::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Paleta semántica.
//
// Un color = un significado, no una decoración. ANSI 256 para tonos finos
// (acero, oro), que cualquier terminal moderno (xterm/minicom/screen/VS Code)
// resuelve. La marca: plata (texto/acero), oro (autoridad/foco) y verde (el
// núcleo, lo vivo y sano).
// ---------------------------------------------------------------------------

/// Reinicia todos los atributos (color y estilo).
pub const RESET: &str = "\x1b[0m";

/// Negrita / intensidad.
pub const BOLD: &str = "\x1b[1m";
/// Atenuado (chrome, detalles secundarios).
pub const DIM: &str = "\x1b[2m";

/// Núcleo / OK / lo vivo y sano — **verde** esmeralda.
pub const VERDE: &str = "\x1b[38;5;84m";
/// Autoridad / foco / sesión autenticada — **oro**.
pub const ORO: &str = "\x1b[38;5;220m";
/// Datos / valores / enlaces — **cian** sereno.
pub const CIAN: &str = "\x1b[38;5;80m";
/// Texto principal — **plata** (acero claro).
pub const PLATA: &str = "\x1b[38;5;252m";
/// Aviso / letargo / atención no fatal — **ámbar**.
pub const AMBAR: &str = "\x1b[38;5;215m";
/// Fallo / fault / peligro — **rojo** templado.
pub const ROJO: &str = "\x1b[38;5;203m";
/// Cromo / marcos / etiquetas tenues — **gris** acero.
pub const GRIS: &str = "\x1b[38;5;245m";

// ---------------------------------------------------------------------------
// Flag global de color.
//
// El color es por capacidad del terminal, negociada al abrir sesión. En un
// dispositivo embebido hay de hecho una consola activa a la vez, así que un
// flag global es suficiente y de coste cero (relaxed atomic). `rush` lo fija
// según el transporte / `NO_COLOR`; por defecto rico.
// ---------------------------------------------------------------------------

static COLOR_ON: AtomicBool = AtomicBool::new(true);

/// Activa o desactiva el color para toda la salida posterior.
pub fn set_color(on: bool) {
    COLOR_ON.store(on, Ordering::Relaxed);
}

/// ¿Está el color activo?
pub fn color() -> bool {
    COLOR_ON.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Rol semántico de un fragmento de texto.
//
// El formateador piensa en *significados*, no en códigos: `Role::Core`, no
// "verde 84". Así la paleta se puede reajustar en un único sitio.
// ---------------------------------------------------------------------------

/// Significado de un fragmento, que decide su color en modo rico.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Núcleo / OK / sano (verde).
    Core,
    /// Autoridad / foco / sesión (oro).
    Focus,
    /// Datos / valores (cian).
    Data,
    /// Texto principal (plata).
    Text,
    /// Aviso / letargo (ámbar).
    Warn,
    /// Fallo / fault (rojo).
    Fault,
    /// Cromo / etiquetas tenues (gris).
    Chrome,
}

impl Role {
    /// Código ANSI de primer plano del rol.
    pub const fn code(self) -> &'static str {
        match self {
            Role::Core => VERDE,
            Role::Focus => ORO,
            Role::Data => CIAN,
            Role::Text => PLATA,
            Role::Warn => AMBAR,
            Role::Fault => ROJO,
            Role::Chrome => GRIS,
        }
    }
}

// ---------------------------------------------------------------------------
// Painter — compositor sobre un buffer fijo.
//
// Sucesor del `SliceWriter` ad-hoc que cada formateador reinventaba: escribe
// texto y números a un `&mut [u8]` truncando con seguridad, y añade primitivas
// de presentación (color por rol, badges, kv, medidores) que respetan el flag
// global. Las secuencias ANSI no cuentan como “contenido”: en modo plano no se
// emite ni un byte de escape (los glifos UTF-8 sí permanecen), legible en
// cualquier captura de log sin terminal que interprete color.
// ---------------------------------------------------------------------------

/// Compositor de salida sobre un buffer fijo (sin `alloc`).
pub struct Painter<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Painter<'a> {
    /// Crea un pintor sobre `buf`.
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Bytes escritos hasta ahora.
    pub fn len(&self) -> usize {
        self.pos
    }

    /// ¿No se ha escrito nada todavía?
    pub fn is_empty(&self) -> bool {
        self.pos == 0
    }

    /// Escribe `s` crudo, truncando si no cabe (nunca desborda).
    pub fn raw(&mut self, s: &str) -> &mut Self {
        let bytes = s.as_bytes();
        let n = bytes.len().min(self.buf.len() - self.pos);
        self.buf[self.pos..self.pos + n].copy_from_slice(&bytes[..n]);
        self.pos += n;
        self
    }

    /// Emite una secuencia ANSI **solo** si el color está activo.
    fn esc(&mut self, code: &str) -> &mut Self {
        if color() {
            self.raw(code);
        }
        self
    }

    /// Abre el color de un rol (no-op en modo plano).
    pub fn on(&mut self, role: Role) -> &mut Self {
        self.esc(role.code())
    }

    /// Cierra todos los atributos (no-op en modo plano).
    pub fn off(&mut self) -> &mut Self {
        self.esc(RESET)
    }

    /// Escribe `s` con el color de `role`, cerrando al terminar.
    pub fn text(&mut self, role: Role, s: &str) -> &mut Self {
        self.on(role).raw(s).off()
    }

    /// Escribe `v` en decimal (sin color).
    pub fn u32(&mut self, v: u32) -> &mut Self {
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
        self.raw(unsafe { core::str::from_utf8_unchecked(&tmp[i..]) })
    }

    /// Escribe `v` en decimal con el color de `role`.
    pub fn num(&mut self, role: Role, v: u32) -> &mut Self {
        self.on(role).u32(v).off()
    }

    /// Escribe `v` en hexadecimal sin prefijo (sin color).
    pub fn hex(&mut self, v: u32) -> &mut Self {
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
                tmp[i] = HEX[(n & 0xf) as usize];
                n >>= 4;
            }
        }
        // SAFETY: solo dígitos hex ASCII en [i, len).
        self.raw(unsafe { core::str::from_utf8_unchecked(&tmp[i..]) })
    }

    // -- Componentes de alto nivel ------------------------------------------

    /// Cabecera de sección: `◆ <título> ───────────`.
    ///
    /// El rombo es el núcleo (verde); el título en oro (foco); la regla en gris.
    pub fn header(&mut self, title: &str) -> &mut Self {
        self.text(Role::Core, "\u{25c6} ")
            .on(Role::Focus)
            .raw(title)
            .off()
            .raw(" ")
            .on(Role::Chrome);
        // Regla hasta ~44 columnas, descontando "◆ " + título + espacio.
        let used = 2 + title.chars().count() + 1;
        for _ in used..44 {
            self.raw("\u{2500}"); // ─
        }
        self.off().raw("\r\n")
    }

    /// Badge enmarcado: `▐ texto ▌` con el color del rol.
    pub fn badge(&mut self, role: Role, label: &str) -> &mut Self {
        self.on(role)
            .raw("\u{2590}")
            .raw(label)
            .raw("\u{258c}")
            .off()
    }

    /// Par clave/valor alineado: `clave` en gris, `valor` en el rol dado.
    pub fn kv(&mut self, key: &str, role: Role, value: &str) -> &mut Self {
        self.on(Role::Chrome)
            .raw(key)
            .off()
            .raw(" ")
            .text(role, value)
    }

    /// Par clave/número.
    pub fn kvn(&mut self, key: &str, role: Role, value: u32) -> &mut Self {
        self.on(Role::Chrome)
            .raw(key)
            .off()
            .raw(" ")
            .num(role, value)
    }

    /// Medidor de barra de `width` celdas para `pct` (0..=100): celdas llenas
    /// en un color según el umbral (verde/ámbar/rojo), vacías en gris.
    pub fn meter(&mut self, pct: u32, width: u32) -> &mut Self {
        let pct = pct.min(100);
        let full = (pct * width + 50) / 100;
        let role = if pct >= 90 {
            Role::Fault
        } else if pct >= 70 {
            Role::Warn
        } else {
            Role::Core
        };
        self.on(role);
        for _ in 0..full {
            self.raw("\u{2588}"); // █
        }
        self.off().on(Role::Chrome);
        for _ in full..width {
            self.raw("\u{2591}"); // ░
        }
        self.off()
    }

    /// Línea de feedback positivo: `✓ <msg>` en verde.
    pub fn ok(&mut self, msg: &str) -> &mut Self {
        self.on(Role::Core).raw("\u{2713} ").raw(msg).off()
    }

    /// Línea de feedback de error: `✗ <msg>` en rojo.
    pub fn err(&mut self, msg: &str) -> &mut Self {
        self.on(Role::Fault).raw("\u{2717} ").raw(msg).off()
    }
}
