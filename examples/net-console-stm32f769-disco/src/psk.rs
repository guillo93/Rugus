//! Almacén de PSK en flash QSPI externa (F769) para la autenticación de canal.
//!
//! La consola universal `rush` exige challenge-response HMAC antes de aceptar
//! verbos privilegiados, pero **no** ve el secreto: lo delega en la personalidad
//! por punteros de función (ver [`crate::auth`]). En la F769-DISCO la PSK vive
//! en el **primer subsector (4 KiB) de la NOR QSPI** (MX25L512), aprovisionable
//! **una sola vez** con `enroll`; re-provisionar exige borrar ese subsector
//! (factory reset).
//!
//! ## Por qué QSPI dedicada y no RFN
//!
//! Las claves RFN se leen/escriben con los verbos `schema`/`scribe` del ABI de
//! syscalls — un secreto ahí sería trivialmente exfiltrable. La PSK necesita un
//! almacén que la consola **no** sepa enumerar ni leer. Un subsector QSPI
//! dedicado, escrito solo por este módulo y jamás devuelto por ningún hook de
//! lectura, cumple ese requisito (mismo principio que la página de flash interna
//! reservada en la personalidad lite del F103).
//!
//! ## Formato del subsector (little-endian)
//!
//! | offset | tamaño | campo                                  |
//! |--------|--------|----------------------------------------|
//! | 0      | 4      | magic `RUGS`                           |
//! | 4      | 2      | longitud de la PSK en bytes (1..=64)   |
//! | 6      | 2      | padding (0xFFFF)                       |
//! | 8      | len    | bytes de la PSK                        |

use core::ptr::addr_of_mut;

use rugus_hal::BlockDevice;
use rugus_hal_stm32f7::qspi::Qspi;

/// Dirección del subsector reservado para la PSK (primer subsector de la NOR).
const PSK_ADDR: u32 = 0x0000_0000;
/// Magic que marca un subsector aprovisionado (`RUGS`).
const MAGIC: [u8; 4] = *b"RUGS";
/// Offset de los bytes de PSK dentro del subsector.
const PSK_OFF: u32 = 8;
/// Longitud máxima de PSK soportada (= tamaño de bloque HMAC-SHA256).
const PSK_MAX: usize = 64;

/// Handle QSPI propiedad de la tarea de red; se fija una vez en el arranque.
/// Solo la tarea de red (que posee la pila y la consola) lo toca tras `start()`.
static mut QSPI: Option<Qspi> = None;

/// Entrega el handle QSPI al almacén (llamado una vez durante el bring-up).
///
/// # Safety
/// Debe invocarse en arranque single-thread antes de `start()`; a partir de ahí
/// el handle solo lo usa, en exclusiva, la tarea de red.
pub unsafe fn install(qspi: Qspi) {
    unsafe { QSPI = Some(qspi) };
}

/// Acceso exclusivo al handle QSPI; `None` si aún no se instaló.
fn with_qspi<R>(f: impl FnOnce(&mut Qspi) -> R) -> Option<R> {
    // SAFETY: la tarea de red es la única que llama a estas funciones tras el
    // arranque; sin reentrada (scheduler cooperativo en la tarea de red).
    let q = unsafe { (*addr_of_mut!(QSPI)).as_mut()? };
    Some(f(q))
}

/// `true` si el subsector tiene el magic: ya hay PSK aprovisionada.
pub fn provisioned() -> bool {
    let mut hdr = [0u8; 4];
    matches!(
        with_qspi(|q| q.read(PSK_ADDR, &mut hdr).is_ok()),
        Some(true)
    ) && hdr == MAGIC
}

/// Longitud de la PSK aprovisionada, o 0 si no hay ninguna válida.
fn psk_len() -> usize {
    let mut hdr = [0u8; 6];
    let ok = matches!(
        with_qspi(|q| q.read(PSK_ADDR, &mut hdr).is_ok()),
        Some(true)
    );
    if !ok || hdr[..4] != MAGIC {
        return 0;
    }
    let len = u16::from_le_bytes([hdr[4], hdr[5]]) as usize;
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
    if len == 0 {
        return 0;
    }
    match with_qspi(|q| q.read(PSK_ADDR + PSK_OFF, &mut buf[..len]).is_ok()) {
        Some(true) => len,
        _ => 0,
    }
}

/// Aprovisiona la PSK escribiéndola en el subsector. Falla (devuelve `false`)
/// si ya está aprovisionada, si la PSK es vacía/larga, o si la escritura no
/// verifica. Una sola vez: re-provisionar exige borrar el subsector.
pub fn enroll(psk: &[u8]) -> bool {
    if provisioned() || psk.is_empty() || psk.len() > PSK_MAX {
        return false;
    }
    // Cabecera + cuerpo en un buffer contiguo (subsector borrado a 0xFF).
    let mut page = [0xFFu8; PSK_OFF as usize + PSK_MAX];
    page[..4].copy_from_slice(&MAGIC);
    page[4..6].copy_from_slice(&(psk.len() as u16).to_le_bytes());
    // page[6..8] queda en 0xFF (padding).
    page[PSK_OFF as usize..PSK_OFF as usize + psk.len()].copy_from_slice(psk);
    let total = PSK_OFF as usize + psk.len();

    let written = with_qspi(|q| {
        q.erase_sector(PSK_ADDR).is_ok() && q.program(PSK_ADDR, &page[..total]).is_ok()
    });
    if !matches!(written, Some(true)) {
        return false;
    }
    // Verifica releyendo: longitud + bytes idénticos.
    if psk_len() != psk.len() {
        return false;
    }
    let mut back = [0u8; PSK_MAX];
    read_psk(&mut back) == psk.len() && back[..psk.len()] == *psk
}
