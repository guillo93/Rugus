//! Almacén de PSK en flash (F103) para la autenticación de canal (F6.1).
//!
//! La consola universal `rush` exige challenge-response HMAC antes de aceptar
//! verbos privilegiados, pero **no** ve el secreto: lo delega en la personalidad
//! por punteros de función. En la Blue Pill, la PSK vive en la última página de
//! flash (1K en `0x0800FC00`, reservada en `memory.x`), aprovisionable **una
//! sola vez** en fábrica con `enroll`; re-provisionar exige borrar esa página
//! (factory reset físico).
//!
//! ## Por qué flash y no RFN
//!
//! Las claves RFN se leen/escriben con los verbos `schema`/`scribe`, expuestos
//! por el ABI de syscalls — un secreto ahí sería trivialmente exfiltrable. La
//! PSK necesita un almacén que la consola **no** sepa enumerar ni leer. Una
//! página de flash dedicada, escrita por el driver FPEC mínimo de este módulo y
//! jamás devuelta por ningún hook de lectura, cumple ese requisito.
//!
//! ## Formato de la página (little-endian, alineado a halfword)
//!
//! | offset | tamaño | campo                                  |
//! |--------|--------|----------------------------------------|
//! | 0      | 4      | magic `RUGS` (0x53_47_55_52)           |
//! | 4      | 2      | longitud de la PSK en bytes (1..=64)   |
//! | 6      | 2      | padding (0xFFFF)                       |
//! | 8      | len    | bytes de la PSK                        |

use rugus_hal_stm32f1::pac;

/// Dirección base de la página reservada para la PSK (última de 64K, 1K).
const PSK_PAGE: u32 = 0x0800_FC00;
/// Magic que marca una página aprovisionada (`RUGS` en little-endian).
const MAGIC: u32 = 0x5347_5552;
/// Offset de los bytes de PSK dentro de la página.
const PSK_OFF: u32 = 8;
/// Longitud máxima de PSK soportada (= tamaño de bloque HMAC-SHA256).
const PSK_MAX: usize = 64;

/// Claves de desbloqueo del FPEC (Reference Manual RM0008, §3.3.3).
const KEY1: u32 = 0x4567_0123;
const KEY2: u32 = 0xCDEF_89AB;

// --- Bits de FLASH_CR / FLASH_SR (RM0008). ---
const CR_PG: u32 = 1 << 0;
const CR_PER: u32 = 1 << 1;
const CR_STRT: u32 = 1 << 6;
const CR_LOCK: u32 = 1 << 7;
const SR_BSY: u32 = 1 << 0;
const SR_PGERR: u32 = 1 << 2;
const SR_WRPRTERR: u32 = 1 << 4;
const SR_EOP: u32 = 1 << 5;

/// `true` si la página tiene el magic: ya hay PSK aprovisionada.
pub fn provisioned() -> bool {
    read_u32(PSK_PAGE) == MAGIC
}

/// Longitud de la PSK aprovisionada, o 0 si no hay ninguna válida.
fn psk_len() -> usize {
    if !provisioned() {
        return 0;
    }
    let len = (read_u32(PSK_PAGE + 4) & 0xFFFF) as usize;
    if (1..=PSK_MAX).contains(&len) {
        len
    } else {
        0
    }
}

/// Copia la PSK aprovisionada en `buf`; devuelve su longitud (0 si no hay).
/// Solo este módulo la lee y solo para calcular el HMAC; nunca sale al cable.
pub fn read_psk(buf: &mut [u8; PSK_MAX]) -> usize {
    let len = psk_len();
    let base = PSK_PAGE + PSK_OFF;
    for (i, byte) in buf.iter_mut().enumerate().take(len) {
        *byte = read_u8(base + i as u32);
    }
    len
}

/// Aprovisiona la PSK escribiéndola en la página de flash. Falla (devuelve
/// `false`) si ya está aprovisionada o si la escritura no verifica. Una sola
/// vez: re-provisionar exige borrar la página (factory reset).
pub fn enroll(psk: &[u8]) -> bool {
    if provisioned() || psk.is_empty() || psk.len() > PSK_MAX {
        return false;
    }
    // SAFETY: secuencia FPEC serializada; solo la tarea CLI cooperativa la
    // invoca (vía hook), sin reentrada. La página está fuera del rango del
    // linker (memory.x la excluye), así que no pisamos código ni datos.
    unsafe {
        unlock();
        erase_page(PSK_PAGE);
        // Cabecera: magic + longitud (halfwords).
        program_hw(PSK_PAGE, (MAGIC & 0xFFFF) as u16);
        program_hw(PSK_PAGE + 2, (MAGIC >> 16) as u16);
        program_hw(PSK_PAGE + 4, psk.len() as u16);
        program_hw(PSK_PAGE + 6, 0xFFFF);
        // Cuerpo: PSK en halfwords (relleno con 0xFF si longitud impar).
        let mut i = 0;
        while i < psk.len() {
            let lo = psk[i] as u16;
            let hi = if i + 1 < psk.len() {
                psk[i + 1] as u16
            } else {
                0xFF
            };
            program_hw(PSK_PAGE + PSK_OFF + i as u32, lo | (hi << 8));
            i += 2;
        }
        lock();
    }
    // Verifica releyendo: magic + longitud + bytes idénticos.
    if psk_len() != psk.len() {
        return false;
    }
    let base = PSK_PAGE + PSK_OFF;
    psk.iter()
        .enumerate()
        .all(|(i, &b)| read_u8(base + i as u32) == b)
}

// --- Driver FPEC mínimo (halfword, una página). ---

/// Desbloquea el FPEC para permitir borrado/programación.
///
/// SAFETY: escribe registros FLASH; el llamador serializa el acceso.
unsafe fn unlock() {
    let f = &*pac::FLASH::ptr();
    if f.cr.read().bits() & CR_LOCK != 0 {
        f.keyr.write(|w| w.bits(KEY1));
        f.keyr.write(|w| w.bits(KEY2));
    }
}

/// Re-bloquea el FPEC tras programar.
///
/// SAFETY: escribe FLASH_CR; llamador serializa.
unsafe fn lock() {
    let f = &*pac::FLASH::ptr();
    f.cr.modify(|r, w| w.bits(r.bits() | CR_LOCK));
}

/// Borra una página de 1K. Espera BSY y limpia los flags de error.
///
/// SAFETY: borra flash; `addr` debe ser una página reservada (PSK_PAGE).
unsafe fn erase_page(addr: u32) {
    let f = &*pac::FLASH::ptr();
    wait_busy(f);
    f.cr.modify(|r, w| w.bits(r.bits() | CR_PER));
    f.ar.write(|w| w.bits(addr));
    f.cr.modify(|r, w| w.bits(r.bits() | CR_STRT));
    wait_busy(f);
    f.cr.modify(|r, w| w.bits(r.bits() & !CR_PER));
    clear_eop(f);
}

/// Programa un halfword (16 bits) en `addr` (debe ser par y estar borrado).
///
/// SAFETY: escribe flash; el llamador garantiza dirección válida y desbloqueo.
unsafe fn program_hw(addr: u32, data: u16) {
    let f = &*pac::FLASH::ptr();
    wait_busy(f);
    f.cr.modify(|r, w| w.bits(r.bits() | CR_PG));
    core::ptr::write_volatile(addr as *mut u16, data);
    wait_busy(f);
    f.cr.modify(|r, w| w.bits(r.bits() & !CR_PG));
    clear_eop(f);
}

/// Espera a que el FPEC quede libre (BSY=0).
unsafe fn wait_busy(f: &pac::flash::RegisterBlock) {
    while f.sr.read().bits() & SR_BSY != 0 {}
}

/// Limpia EOP/PGERR/WRPRTERR (escritura-1-para-limpiar).
unsafe fn clear_eop(f: &pac::flash::RegisterBlock) {
    f.sr.write(|w| w.bits(SR_EOP | SR_PGERR | SR_WRPRTERR));
}

// --- Lectura directa de flash (memoria mapeada). ---

fn read_u32(addr: u32) -> u32 {
    // SAFETY: flash mapeada en memoria, lectura alineada de 32 bits.
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

fn read_u8(addr: u32) -> u8 {
    // SAFETY: flash mapeada en memoria, lectura de byte.
    unsafe { core::ptr::read_volatile(addr as *const u8) }
}
