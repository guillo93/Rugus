//! Independent watchdog (IWDG) para STM32F7 — raw-MMIO, gemelo del de F4.
//!
//! El IWDG corre de un reloj LSI propio (~32 kHz), independiente del SYSCLK: si
//! el kernel se cuelga y deja de alimentarlo, dispara un reset de sistema. Es la
//! última red de seguridad del failsafe (el WFI terminal, con todas las tareas
//! muertas, deja de alimentarlo → reset → arranque limpio).
//!
//! El bloque IWDG es idéntico en F4/F7 (mismos offsets KR/PR/RLR/SR en
//! 0x4000_3000), por eso este driver es un gemelo exacto del de F4. Acceso por
//! MMIO directo en la misma línea que [`crate::gpio`]/[`crate::usart`].
//!
//! Secuencia: `start` desbloquea (KR=0x5555), fija prescaler y reload, luego lo
//! arranca (KR=0xCCCC) — esto también enciende el LSI automáticamente. A partir
//! de ahí hay que `kick` (KR=0xAAAA) antes de que venza el reload.

use core::ptr::write_volatile;

/// Base del IWDG (idéntica en F4/F7).
const IWDG_BASE: u32 = 0x4000_3000;

// Offsets de registro.
const KR: u32 = 0x00;
const PR: u32 = 0x04;
const RLR: u32 = 0x08;
const SR: u32 = 0x0C;
const WINR: u32 = 0x10;

// Flags de "update in progress" del status register (SR). Tras escribir PR/RLR/
// WINR hay que esperar a que el flag correspondiente vuelva a 0 antes del
// siguiente write, o el hardware lo ignora silenciosamente.
const SR_PVU: u32 = 1 << 0; // prescaler update
const SR_RVU: u32 = 1 << 1; // reload update
const SR_WVU: u32 = 1 << 2; // window update

// Llaves del key register.
const KEY_RELOAD: u32 = 0xAAAA;
const KEY_ENABLE_WRITE: u32 = 0x5555;
const KEY_START: u32 = 0xCCCC;

/// Prescaler /128 (PR=0b101): con LSI ~32 kHz da ~250 Hz (tick ~4 ms).
const PR_DIV128: u32 = 0b101;
/// Reload ~4 s nominal: 250 Hz * 4 s = 1000 ticks. El LSI NO está calibrado (rango
/// de hoja ~17-47 kHz, ±50 % sobre 32 kHz nominal), así que el periodo real varía
/// de ~2.7 s (LSI rápido) a ~7.5 s (LSI lento). Una ventana ancha es lo que hace
/// fiable el kick a ~1.5 s del supervisor en AMBAS placas (ver `WINR_OPEN`).
const RLR_4S: u32 = 1000;
/// Apertura de ventana temprana (~0.5 s nominal): con modo windowed solo se puede
/// alimentar cuando el contador ha bajado de este valor; alimentar antes resetea.
/// Se abre tras 125 ticks (RLR_4S - 875), ~0.5 s nominal. Con el kick fijo a ~1.5 s
/// (reloj SysTick preciso), la ventana real [~0.5 s, ~4 s] nominal tolera todo el
/// rango del LSI sin disparar ni el límite inferior (kick demasiado pronto) ni el
/// superior (cuelgue): el bug de bucle de reset en F769 venía de una ventana
/// [~1 s, ~2 s] demasiado estrecha frente a un LSI rápido (F4.6).
const WINR_OPEN: u32 = 875;

/// Handle del watchdog independiente.
pub struct Iwdg {
    armed: bool,
}

impl Iwdg {
    /// Configura prescaler /128 y reload ~4 s nominal, y arranca el IWDG. Tras
    /// esto hay que [`Self::kick`] periódicamente o el chip se resetea.
    pub fn start() -> Self {
        // SAFETY: registros MMIO del IWDG; arranque single-thread.
        //
        // No se sondea SR.PVU/RVU: esos flags solo se actualizan con el LSI en
        // marcha, y el LSI no arranca hasta KEY_START (0xCCCC). Sondearlos antes
        // de arrancar cuelga (LSI parado → nunca se limpian). Habilitamos
        // escritura, programamos PR/RLR, arrancamos (esto enciende el LSI) y
        // recargamos; el hardware aplica PR/RLR antes del primer timeout.
        unsafe {
            write_reg(KR, KEY_ENABLE_WRITE);
            write_reg(PR, PR_DIV128);
            write_reg(RLR, RLR_4S);
            write_reg(KR, KEY_START);
            write_reg(KR, KEY_RELOAD);
        }
        Self { armed: true }
    }

    /// Como [`Self::start`] pero en **modo windowed**: además del límite superior
    /// (~4 s nominal sin kick → reset por cuelgue), fija una ventana inferior
    /// (`WINR_OPEN`) de modo que alimentar demasiado pronto (< ~0.5 s nominal tras
    /// la recarga) también resetea. Detecta un supervisor en bucle desbocado, no
    /// solo uno parado.
    ///
    /// El supervisor debe espaciar su kick para caer en la ventana [~0.5 s, ~4 s]
    /// nominal; con kick a ~1.5 s (reloj SysTick preciso) hay margen amplio frente
    /// a la tolerancia del LSI. Escribir WINR recarga el contador automáticamente.
    pub fn start_windowed() -> Self {
        // SAFETY: registros MMIO del IWDG; arranque single-thread.
        //
        // Secuencia canónica de modo windowed (RM0410 §IWDG): arrancar PRIMERO el
        // IWDG (KEY_START enciende el LSI), luego habilitar escritura y programar
        // PR/RLR/WINR esperando a que cada flag de "update" (SR.PVU/RVU/WVU) vuelva
        // a 0 antes del siguiente write. Esta espera es OBLIGATORIA: el bug del
        // bucle de reset en F769 era que, sin esperar, los writes de PR/RLR se
        // perdían y el watchdog corría con sus defaults (PR=/4, RLR=0xFFF → ~0.5 s
        // nominal, ~1 s con LSI lento) y reseteaba antes del primer kick (~1.5 s).
        // Aquí poder sondear SR es seguro porque el LSI YA gira (post KEY_START).
        // Escribir WINR recarga el contador por hardware (no hace falta KEY_RELOAD).
        unsafe {
            write_reg(KR, KEY_START);
            write_reg(KR, KEY_ENABLE_WRITE);
            write_reg(PR, PR_DIV128);
            wait_sr_clear(SR_PVU);
            write_reg(RLR, RLR_4S);
            wait_sr_clear(SR_RVU);
            write_reg(WINR, WINR_OPEN);
            wait_sr_clear(SR_WVU);
        }
        Self { armed: true }
    }

    /// Alimenta el watchdog (recarga el contador). No-op si no está armado.
    pub fn kick(&self) {
        if self.armed {
            // SAFETY: escribir la llave de reload es atómico y siempre seguro.
            unsafe { write_reg(KR, KEY_RELOAD) }
        }
    }
}

#[inline]
unsafe fn write_reg(off: u32, val: u32) {
    unsafe { write_volatile((IWDG_BASE + off) as *mut u32, val) }
}

#[inline]
unsafe fn read_reg(off: u32) -> u32 {
    unsafe { core::ptr::read_volatile((IWDG_BASE + off) as *const u32) }
}

/// Espera a que el flag de "update" indicado en SR vuelva a 0 (el hardware acaba
/// de aplicar el write a PR/RLR/WINR). Acotado: tras ~100k iteraciones desiste
/// para no colgar el arranque si el LSI fallara (el IWDG aún protege con defaults).
#[inline]
unsafe fn wait_sr_clear(flag: u32) {
    let mut spins = 0u32;
    // SAFETY: SR es de solo lectura; el LSI ya gira (post KEY_START) así que el
    // flag se actualiza y termina por limpiarse.
    while unsafe { read_reg(SR) } & flag != 0 {
        spins += 1;
        if spins >= 100_000 {
            break;
        }
        core::hint::spin_loop();
    }
}
