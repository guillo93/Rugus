//! Almacén de PSK en la ventana de flash interna (F407) para la autenticación
//! de canal.
//!
//! La consola universal `rush` exige challenge-response HMAC antes de aceptar
//! verbos privilegiados, pero **no** ve el secreto: lo delega en la personalidad
//! por punteros de función (ver [`crate::auth`]). En la F407G-DISC1 la PSK vive
//! en el **sector 11 de la flash interna** (128 KiB en `0x080E_0000`, excluido
//! del linker en `memory.x` y expuesto por
//! [`rugus_hal_stm32f4::flash::FlashWindow`]), aprovisionable **una sola vez**
//! con `enroll`; re-provisionar exige borrar el sector (factory reset). Como el
//! sector está fuera del rango que `probe-rs download` reescribe, la PSK
//! sobrevive a los reflasheos del firmware — igual que el subsector QSPI del
//! F769.
//!
//! ## Por qué una ventana dedicada y no RFN
//!
//! Las claves RFN se leen/escriben con los verbos `schema`/`scribe` del ABI de
//! syscalls — un secreto ahí sería trivialmente exfiltrable. La PSK necesita un
//! almacén que la consola **no** sepa enumerar ni leer. Un sector dedicado,
//! escrito solo por este módulo y jamás devuelto por ningún hook de lectura,
//! cumple ese requisito (mismo principio que la página FPEC del F103 lite y el
//! subsector QSPI del F769).
//!
//! ## Formato de la ventana (little-endian)
//!
//! | offset | tamaño | campo                                  |
//! |--------|--------|----------------------------------------|
//! | 0      | 4      | magic `RUGS`                           |
//! | 4      | 2      | longitud de la PSK en bytes (1..=64)   |
//! | 6      | 2      | padding (0xFFFF)                       |
//! | 8      | len    | bytes de la PSK                        |

use core::ptr::addr_of_mut;

use rugus_hal::BlockDevice;
use rugus_hal_stm32f4::flash::FlashWindow;

/// Dirección del registro de PSK dentro de la ventana (su inicio).
const PSK_ADDR: u32 = 0x0000_0000;
/// Magic que marca una ventana aprovisionada (`RUGS`).
const MAGIC: [u8; 4] = *b"RUGS";
/// Offset de los bytes de PSK dentro de la ventana.
const PSK_OFF: u32 = 8;
/// Longitud máxima de PSK soportada (= tamaño de bloque HMAC-SHA256).
const PSK_MAX: usize = 64;

/// Handle de la ventana de flash; se fija una vez en el arranque y a partir de
/// ahí solo lo usa, en exclusiva, la tarea supervisora (que posee la consola).
static mut FLASH: Option<FlashWindow> = None;

/// Entrega el handle de la ventana al almacén (llamado una vez en el bring-up).
///
/// # Safety
/// Debe invocarse en arranque single-thread antes de `start()`; a partir de ahí
/// el handle solo lo usa, en exclusiva, la tarea supervisora.
pub unsafe fn install(flash: FlashWindow) {
    unsafe { FLASH = Some(flash) };
}

/// Acceso exclusivo al handle de la ventana; `None` si aún no se instaló.
fn with_flash<R>(f: impl FnOnce(&mut FlashWindow) -> R) -> Option<R> {
    // SAFETY: la tarea supervisora es la única que llama a estas funciones tras
    // el arranque; sin reentrada (scheduler cooperativo en esa tarea).
    let w = unsafe { (*addr_of_mut!(FLASH)).as_mut()? };
    Some(f(w))
}

/// `true` si la ventana tiene el magic: ya hay PSK aprovisionada.
pub fn provisioned() -> bool {
    let mut hdr = [0u8; 4];
    matches!(
        with_flash(|w| w.read(PSK_ADDR, &mut hdr).is_ok()),
        Some(true)
    ) && hdr == MAGIC
}

/// Longitud de la PSK aprovisionada, o 0 si no hay ninguna válida.
fn psk_len() -> usize {
    let mut hdr = [0u8; 6];
    let ok = matches!(
        with_flash(|w| w.read(PSK_ADDR, &mut hdr).is_ok()),
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
    match with_flash(|w| w.read(PSK_ADDR + PSK_OFF, &mut buf[..len]).is_ok()) {
        Some(true) => len,
        _ => 0,
    }
}

/// Aprovisiona la PSK escribiéndola en la ventana. Falla (devuelve `false`) si
/// ya está aprovisionada, si la PSK es vacía/larga, o si la escritura no
/// verifica. Una sola vez: re-provisionar exige borrar el sector.
///
/// El borrado del sector (128 KiB) stalla el CPU ~1-2 s (fetch desde la misma
/// flash); aceptable para una operación única de fábrica.
pub fn enroll(psk: &[u8]) -> bool {
    if provisioned() || psk.is_empty() || psk.len() > PSK_MAX {
        return false;
    }
    // Cabecera + cuerpo en un buffer contiguo (sector borrado a 0xFF).
    let mut page = [0xFFu8; PSK_OFF as usize + PSK_MAX];
    page[..4].copy_from_slice(&MAGIC);
    page[4..6].copy_from_slice(&(psk.len() as u16).to_le_bytes());
    // page[6..8] queda en 0xFF (padding).
    page[PSK_OFF as usize..PSK_OFF as usize + psk.len()].copy_from_slice(psk);
    let total = PSK_OFF as usize + psk.len();

    let written = with_flash(|w| {
        w.erase_sector(PSK_ADDR).is_ok() && w.program(PSK_ADDR, &page[..total]).is_ok()
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
