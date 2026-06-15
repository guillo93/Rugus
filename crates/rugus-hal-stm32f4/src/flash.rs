//! Driver mínimo de la flash interna F4 (interfaz FPEC, RM0090 §3) sobre la
//! ventana reservada para secretos de personalidad (F6.4c).
//!
//! El F407 tiene la flash en sectores asimétricos (0–3 de 16 KiB, 4 de 64 KiB,
//! 5+ de 128 KiB). Este driver expone **el último sector** como [`BlockDevice`]
//! con direcciones relativas a la ventana: en el **VG** (1 MiB) es el sector 11
//! (`0x080E_0000`, [`FlashWindow::new`]); en el **VE** (512 KiB) es el sector 7
//! (`0x0806_0000`, [`FlashWindow::new_ve512k`]). El `memory.x` del binario DEBE
//! excluir ese sector del rango del linker (FLASH `LENGTH` recortado) para que
//! ni código ni constantes lo pisen.
//!
//! ## Secuencia FPEC desde RAM (obligatoria)
//!
//! Durante program/erase el bus de flash stalla TODO acceso, incluido el fetch
//! de instrucciones; por eso el bucle de espera de `BSY` se ejecuta desde RAM
//! (ver más abajo). Sin ello, una IRQ o el simple re-fetch del bucle durante el
//! stall cuelgan el núcleo (le pasa al F407VE bajo el scheduler).
//!
//! ## Por qué una ventana dedicada y no RFN
//!
//! Mismo principio que la página FPEC del F103 lite y el subsector QSPI del
//! F769: las claves RFN se enumeran/leen con `schema`/`scribe` del ABI, un
//! secreto ahí sería exfiltrable. La PSK de la autenticación de canal necesita
//! un almacén que la consola **no** sepa leer; un sector dedicado, escrito solo
//! por el módulo `psk` de la personalidad y jamás devuelto por ningún hook,
//! cumple ese requisito.
//!
//! ## Semántica
//!
//! - **read**: la flash interna es memory-mapped; lectura directa.
//! - **program**: byte a byte con `PSIZE = x8`, válido a cualquier tensión de
//!   alimentación (RM0090 tabla 8). Solo transiciones `1→0`; el llamante borra
//!   antes.
//! - **erase_sector**: borra el sector 11 completo (la ventana entera).
//!
//! Durante program/erase el controlador de flash **stalla cualquier fetch**
//! desde flash (el CPU se congela hasta terminar). Es aceptable: el único
//! consumidor es el `enroll` único de fábrica de la PSK, fuera del camino
//! caliente. El driver serializa cada operación esperando `BSY`.

use rugus_hal::BlockDevice;

/// Base del bloque de registros de la interfaz de flash (RM0090 §3.8).
const FLASH_REG_BASE: u32 = 0x4002_3C00;

// Offsets de registro.
const KEYR: u32 = 0x04;
const SR: u32 = 0x0C;
const CR: u32 = 0x10;

/// Claves de desbloqueo de FLASH_CR (RM0090 §3.5.2).
const KEY1: u32 = 0x4567_0123;
const KEY2: u32 = 0xCDEF_89AB;

// --- Bits de FLASH_CR ---
const CR_PG: u32 = 1 << 0;
const CR_SER: u32 = 1 << 1;
const CR_SNB_SHIFT: u32 = 3;
/// `PSIZE = x8` (00): programación por byte, segura a cualquier VDD.
const CR_PSIZE_MASK: u32 = 0b11 << 8;
const CR_STRT: u32 = 1 << 16;
const CR_LOCK: u32 = 1 << 31;

// --- Bits de FLASH_SR (write-1-to-clear los de error) ---
const SR_EOP: u32 = 1 << 0;
const SR_OPERR: u32 = 1 << 1;
const SR_WRPERR: u32 = 1 << 4;
const SR_PGAERR: u32 = 1 << 5;
const SR_PGPERR: u32 = 1 << 6;
const SR_PGSERR: u32 = 1 << 7;
const SR_BSY: u32 = 1 << 16;
const SR_ERR_MASK: u32 = SR_OPERR | SR_WRPERR | SR_PGAERR | SR_PGPERR | SR_PGSERR;

/// Sector/base de la ventana en el F407VG (1 MiB): último sector, el 11.
pub const WINDOW_BASE: u32 = 0x080E_0000;
const SECTOR_VG: u32 = 11;
/// Sector/base de la ventana en el F407VE (512 KiB): último sector, el 7.
pub const WINDOW_BASE_VE: u32 = 0x0806_0000;
const SECTOR_VE: u32 = 7;
/// Tamaño de la ventana (= tamaño del último sector, 128 KiB en ambos).
pub const WINDOW_SIZE: u32 = 128 * 1024;

// ===================== Secuencia FPEC desde RAM =====================
//
// Mientras la flash está en program/erase, TODO acceso al bus de flash se stalla
// (RM0090 §3.5), incluido el fetch de instrucciones. Si el bucle de espera de
// `BSY` vive en flash, su re-fetch durante el stall cuelga el núcleo sin retorno
// (le pasa al F407VE bajo el scheduler; el F407VG se salvaba por azar del
// prefetch). La cura robusta para toda la familia: ejecutar la secuencia desde
// **RAM**. Estas funciones van en `.data` —que `cortex-m-rt` copia a RAM en el
// arranque— y solo hacen MMIO inline (sin llamadas a flash). La MPU marca la RAM
// como exec-never (W^X, F4.7), así que el llamante la deshabilita brevemente
// alrededor de la llamada (ver `mpu_off`/`mpu_on`), con las IRQs ya enmascaradas.

/// Registro `MPU->CTRL` (ARMv7-M, B3.5.8).
const MPU_CTRL: u32 = 0xE000_ED94;

/// Deshabilita la MPU y devuelve el `CTRL` previo para restaurarlo. `dsb`/`isb`
/// garantizan que el cambio rija antes del primer fetch desde RAM.
#[inline(always)]
unsafe fn mpu_off() -> u32 {
    // SAFETY: registro MPU_CTRL estándar ARMv7-M; el llamante serializa.
    unsafe {
        let prev = core::ptr::read_volatile(MPU_CTRL as *const u32);
        core::ptr::write_volatile(MPU_CTRL as *mut u32, 0);
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
        prev
    }
}

/// Restaura `MPU->CTRL` al valor previo.
#[inline(always)]
unsafe fn mpu_on(prev: u32) {
    // SAFETY: registro MPU_CTRL estándar ARMv7-M; el llamante serializa.
    unsafe {
        cortex_m::asm::dsb();
        core::ptr::write_volatile(MPU_CTRL as *mut u32, prev);
        cortex_m::asm::isb();
    }
}

/// Borra el sector `sector` y devuelve `FLASH_SR` final. **Reside en RAM**: el
/// bucle de espera de `BSY` no debe re-fetchearse de la flash en erase.
///
/// # Safety
/// MPU deshabilitada e IRQs enmascaradas por el llamante; FPEC de uso exclusivo.
#[link_section = ".data.ramfunc"]
#[inline(never)]
unsafe fn erase_ramfunc(sector: u32) -> u32 {
    // SAFETY: registros FPEC; MPU off + IRQs enmascaradas por el llamante.
    unsafe {
        let cr = (FLASH_REG_BASE + CR) as *mut u32;
        let sr = (FLASH_REG_BASE + SR) as *mut u32;
        let keyr = (FLASH_REG_BASE + KEYR) as *mut u32;
        while core::ptr::read_volatile(sr) & SR_BSY != 0 {}
        core::ptr::write_volatile(sr, SR_ERR_MASK | SR_EOP); // limpia flags rancios
        if core::ptr::read_volatile(cr) & CR_LOCK != 0 {
            core::ptr::write_volatile(keyr, KEY1);
            core::ptr::write_volatile(keyr, KEY2);
        }
        let base =
            core::ptr::read_volatile(cr) & !(CR_PSIZE_MASK | CR_PG | (0b1111 << CR_SNB_SHIFT));
        core::ptr::write_volatile(cr, base | CR_SER | (sector << CR_SNB_SHIFT));
        core::ptr::write_volatile(cr, core::ptr::read_volatile(cr) | CR_STRT);
        while core::ptr::read_volatile(sr) & SR_BSY != 0 {}
        let st = core::ptr::read_volatile(sr);
        core::ptr::write_volatile(cr, CR_LOCK);
        st
    }
}

/// Programa `len` bytes de `src` (RAM) en `dst` (flash) con `PSIZE x8`, esperando
/// `BSY` tras cada byte. Devuelve `FLASH_SR` final (0 si OK). **Reside en RAM.**
///
/// # Safety
/// `dst..dst+len` dentro de la ventana; MPU off + IRQs enmascaradas; FPEC propio.
#[link_section = ".data.ramfunc"]
#[inline(never)]
unsafe fn program_ramfunc(dst: *mut u8, src: *const u8, len: usize) -> u32 {
    // SAFETY: registros FPEC + punteros validados; MPU off + IRQs enmascaradas.
    unsafe {
        let cr = (FLASH_REG_BASE + CR) as *mut u32;
        let sr = (FLASH_REG_BASE + SR) as *mut u32;
        let keyr = (FLASH_REG_BASE + KEYR) as *mut u32;
        while core::ptr::read_volatile(sr) & SR_BSY != 0 {}
        core::ptr::write_volatile(sr, SR_ERR_MASK | SR_EOP);
        if core::ptr::read_volatile(cr) & CR_LOCK != 0 {
            core::ptr::write_volatile(keyr, KEY1);
            core::ptr::write_volatile(keyr, KEY2);
        }
        let base = core::ptr::read_volatile(cr) & !(CR_PSIZE_MASK | CR_SER);
        core::ptr::write_volatile(cr, base | CR_PG); // PSIZE x8 + PG
        let mut st = 0u32;
        let mut i = 0usize;
        while i < len {
            core::ptr::write_volatile(dst.add(i), core::ptr::read_volatile(src.add(i)));
            while core::ptr::read_volatile(sr) & SR_BSY != 0 {}
            st = core::ptr::read_volatile(sr);
            if st & SR_ERR_MASK != 0 {
                break;
            }
            i += 1;
        }
        core::ptr::write_volatile(cr, CR_LOCK);
        st & SR_ERR_MASK
    }
}

/// Errores del driver de flash interna F4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlashError {
    /// Dirección o longitud fuera de la ventana reservada.
    OutOfRange,
    /// El hardware reportó error (bits de FLASH_SR capturados).
    Hw(u32),
    /// La verificación post-escritura no coincide (celda gastada o no borrada).
    Verify,
}

/// Ventana de flash interna reservada (sector 11) como dispositivo de bloques.
///
/// La construcción es de **propiedad lógica**: quien crea el handle se declara
/// dueño único de la secuencia FPEC. El bring-up de la placa debe crear como
/// máximo uno y entregarlo al almacén de secretos.
pub struct FlashWindow {
    /// Nº de sector a borrar (SNB en FLASH_CR).
    sector: u32,
    /// Base absoluta de la ventana en el espacio de flash.
    base: u32,
}

impl FlashWindow {
    /// Ventana del **F407VG** (1 MiB): sector 11 en `0x080E_0000`.
    ///
    /// # Safety
    ///
    /// Una sola instancia viva; el llamante garantiza que ningún otro código
    /// toca los registros de la interfaz de flash mientras exista, y que el
    /// linker excluye la ventana del rango (memory.x).
    pub unsafe fn new() -> Self {
        Self {
            sector: SECTOR_VG,
            base: WINDOW_BASE,
        }
    }

    /// Ventana del **F407VE** (512 KiB): sector 7 en `0x0806_0000`. Para placas
    /// clon tipo FK407M3-VET6, cuyo último sector es el 7 (el 11 no existe).
    ///
    /// # Safety
    ///
    /// Igual que [`Self::new`]: instancia única y ventana excluida del linker.
    pub unsafe fn new_ve512k() -> Self {
        Self {
            sector: SECTOR_VE,
            base: WINDOW_BASE_VE,
        }
    }

    /// `true` si `[addr, addr+len)` cabe dentro de la ventana (relativa).
    fn in_window(addr: u32, len: usize) -> bool {
        (addr as u64) + (len as u64) <= WINDOW_SIZE as u64
    }
}

impl BlockDevice for FlashWindow {
    type Error = FlashError;

    fn capacity(&self) -> u64 {
        WINDOW_SIZE as u64
    }

    fn prog_size(&self) -> usize {
        // PSIZE x8: programación por byte sin restricción de página.
        1
    }

    fn erase_size(&self) -> usize {
        // La granularidad de borrado es el sector completo (la ventana entera).
        WINDOW_SIZE as usize
    }

    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), FlashError> {
        if !Self::in_window(addr, buf.len()) {
            return Err(FlashError::OutOfRange);
        }
        let base = (self.base + addr) as *const u8;
        for (i, byte) in buf.iter_mut().enumerate() {
            // SAFETY: flash memory-mapped dentro de la ventana validada;
            // volatile porque el contenido cambia con program/erase.
            *byte = unsafe { core::ptr::read_volatile(base.add(i)) };
        }
        Ok(())
    }

    fn program(&mut self, addr: u32, data: &[u8]) -> Result<(), FlashError> {
        if !Self::in_window(addr, data.len()) {
            return Err(FlashError::OutOfRange);
        }
        if data.is_empty() {
            return Ok(());
        }
        // La secuencia FPEC corre desde RAM (`program_ramfunc`) con la MPU
        // deshabilitada e IRQs enmascaradas; ver la nota de la sección RAM y de
        // `erase_sector`. `data` vive en RAM (pila del llamante), legible durante
        // el stall de flash; solo el fetch de instrucciones era el problema.
        let dst = (self.base + addr) as *mut u8;
        let sr = cortex_m::interrupt::free(|_| {
            // SAFETY: MPU off acotado a la llamada ramfunc; ventana validada.
            unsafe {
                let prev = mpu_off();
                let st = program_ramfunc(dst, data.as_ptr(), data.len());
                mpu_on(prev);
                st
            }
        });
        if sr != 0 {
            return Err(FlashError::Hw(sr));
        }
        // Verificación: relee y compara (detecta celdas no borradas/gastadas).
        let base = (self.base + addr) as *const u8;
        for (i, &b) in data.iter().enumerate() {
            // SAFETY: lectura memory-mapped dentro de la ventana validada.
            if unsafe { core::ptr::read_volatile(base.add(i)) } != b {
                return Err(FlashError::Verify);
            }
        }
        Ok(())
    }

    fn erase_sector(&mut self, addr: u32) -> Result<(), FlashError> {
        if addr >= WINDOW_SIZE {
            return Err(FlashError::OutOfRange);
        }
        // La secuencia de borrado (~1-2 s) corre desde RAM (`erase_ramfunc`) con
        // la MPU deshabilitada e IRQs enmascaradas: durante el erase el bus de
        // flash stalla todo fetch, así que el bucle de espera NO puede vivir en
        // flash o el núcleo se cuelga (le pasa al F407VE bajo el scheduler). El
        // coste —UART/relojes parados ~1-2 s— es aceptable para el `enroll` único
        // de fábrica.
        let sr = cortex_m::interrupt::free(|_| {
            // SAFETY: MPU off acotado a la llamada ramfunc; FPEC de uso exclusivo.
            unsafe {
                let prev = mpu_off();
                let st = erase_ramfunc(self.sector);
                mpu_on(prev);
                st
            }
        });
        if sr & SR_ERR_MASK != 0 {
            Err(FlashError::Hw(sr & SR_ERR_MASK))
        } else {
            Ok(())
        }
    }
}
