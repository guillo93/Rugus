//! Banner/logo de arranque de Rugus para las consolas `rush`.
//!
//! Identidad: una **espada de doble filo** (la hoja `║` = los dos filos, guarda
//! en oro, pomo engastado con el núcleo verde = el kernel) junto al wordmark.
//! Refuerza el carácter del proyecto —kernel robusto, canal gateado— y un único
//! léxico para toda la flota.
//!
//! Dos versiones, misma silueta:
//! - [`write_banner`]`(out, true)` → **rico**: Unicode + color ANSI 256. Lo que
//!   ve cualquier terminal moderno (minicom/screen/`rugus-cli`/TTY gráfico).
//! - [`write_banner`]`(out, false)` → **ASCII-safe**: 7-bit sin color, byte a
//!   byte idéntico en cualquier transporte (captura de logs, 7-bit, teletipo).
//!
//! La elección (`color`) la fija el llamador desde las capacidades de la sesión:
//! rico por defecto; plano si se negocia/fuerza (p. ej. `NO_COLOR`).

use crate::ansi::Write;

// Paleta ANSI 256-color usada abajo (los códigos van inline en `concat!`, que
// exige literales): acero degradado punta→base `255 253 251 249 247`, guarda
// oro `1;220`, empuñadura bronce `136`, pomo/núcleo verde `1;84`, wordmark
// `1;253`, tagline `248`, reset `0`.

/// Logo rico (Unicode + color). Espada de hoja larga con la guarda abajo,
/// wordmark en bloque a media hoja. Cada `\n` se expande a `\r\n` en la salida.
const RICH: &str = concat!(
    "\r\n",
    "      ",
    "\x1b[38;5;255m",
    "▲",
    "\x1b[0m",
    "\r\n",
    "      ",
    "\x1b[38;5;255m",
    "║",
    "\x1b[0m",
    "\r\n",
    "      ",
    "\x1b[38;5;253m",
    "║",
    "\x1b[0m",
    "      ",
    "\x1b[1;38;5;253m",
    "█▀█ █ █ █▀▀ █ █ █▀",
    "\x1b[0m",
    "\r\n",
    "      ",
    "\x1b[38;5;251m",
    "║",
    "\x1b[0m",
    "      ",
    "\x1b[1;38;5;253m",
    "█▀▄ █▄█ █▄█ █▄█ ▄█",
    "\x1b[0m",
    "\r\n",
    "      ",
    "\x1b[38;5;249m",
    "║",
    "\x1b[0m",
    "      ",
    "\x1b[38;5;248m",
    "kernel · multipersonalidad",
    "\x1b[0m",
    "\r\n",
    "      ",
    "\x1b[38;5;247m",
    "║",
    "\x1b[0m",
    "      ",
    "\x1b[38;5;248m",
    "multi-arquitectura · RTOS",
    "\x1b[0m",
    "\r\n",
    "   ",
    "\x1b[1;38;5;220m",
    "═══╬═══",
    "\x1b[0m",
    "\r\n",
    "      ",
    "\x1b[38;5;136m",
    "║",
    "\x1b[0m",
    "\r\n",
    "      ",
    "\x1b[1;38;5;84m",
    "◆",
    "\x1b[0m",
    "\r\n",
);

/// Logo ASCII-safe (7-bit, sin color). Misma silueta con `|` y el wordmark
/// FIGlet; idéntico en cualquier terminal.
const PLAIN: &str = concat!(
    "\r\n",
    "      A\r\n",
    "      |\r\n",
    "      |      ____  _   _  ____ _   _ ____\r\n",
    "      |     |  _ \\| | | |/ ___| | | / ___|\r\n",
    "      |     | |_) | | | | |  _| | | \\___ \\\r\n",
    "   ===+===  |  _ <| |_| | |_| | |_| |___) |\r\n",
    "      |     |_| \\_\\___/ \\____|\\___/|____/\r\n",
    "      o     kernel multipersonalidad . multi-arch RTOS\r\n",
);

/// Escribe el banner de arranque. `color = true` ⇒ rico (Unicode + ANSI);
/// `false` ⇒ ASCII-safe. Tras el logo deja una línea en blanco.
pub fn write_banner(out: &mut dyn Write, color: bool) {
    let _ = out.write_str(if color { RICH } else { PLAIN });
    let _ = out.write_str("\r\n");
    // Evita que los códigos de color "manchen" lo que siga.
    if color {
        let _ = out.write_str("\x1b[0m");
    }
}
