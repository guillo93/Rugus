//! Driver mínimo de la flash interna F4 (interfaz FPEC, RM0090 §3) sobre la
//! ventana reservada para secretos de personalidad (F6.4c).
//!
//! El STM32F407VG tiene 1 MiB de flash en sectores asimétricos: 0–3 de 16 KiB,
//! 4 de 64 KiB y 5–11 de 128 KiB. Este driver expone **solo el último sector**
//! (sector 11, `0x080E_0000..0x0810_0000`) como [`BlockDevice`], con
//! direcciones relativas al inicio de la ventana. El `memory.x` del binario
//! que lo use DEBE excluir ese sector del rango del linker (FLASH `LENGTH =
//! 896K`), para que ni código ni constantes lo pisen.
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

/// Sector reservado para la ventana de secretos (último de 1 MiB).
const SECTOR_NUMBER: u32 = 11;
/// Base absoluta de la ventana (sector 11).
pub const WINDOW_BASE: u32 = 0x080E_0000;
/// Tamaño de la ventana (= tamaño del sector 11).
pub const WINDOW_SIZE: u32 = 128 * 1024;

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
    _own: (),
}

impl FlashWindow {
    /// Crea el handle de la ventana reservada.
    ///
    /// # Safety
    ///
    /// Una sola instancia viva; el llamante garantiza que ningún otro código
    /// toca los registros de la interfaz de flash mientras exista, y que el
    /// linker excluye `WINDOW_BASE..WINDOW_BASE+WINDOW_SIZE` (memory.x).
    pub unsafe fn new() -> Self {
        Self { _own: () }
    }

    #[inline]
    fn reg_read(offset: u32) -> u32 {
        // SAFETY: registro MMIO válido del bloque FLASH.
        unsafe { core::ptr::read_volatile((FLASH_REG_BASE + offset) as *const u32) }
    }

    #[inline]
    fn reg_write(offset: u32, value: u32) {
        // SAFETY: registro MMIO válido del bloque FLASH; el handle serializa.
        unsafe { core::ptr::write_volatile((FLASH_REG_BASE + offset) as *mut u32, value) }
    }

    /// Espera fin de operación y devuelve los errores acumulados en SR.
    fn wait_done(&mut self) -> Result<(), FlashError> {
        while Self::reg_read(SR) & SR_BSY != 0 {}
        let sr = Self::reg_read(SR);
        // Limpia EOP y errores (write-1-to-clear) para la próxima operación.
        Self::reg_write(SR, sr & (SR_ERR_MASK | SR_EOP));
        if sr & SR_ERR_MASK != 0 {
            return Err(FlashError::Hw(sr & SR_ERR_MASK));
        }
        Ok(())
    }

    /// Espera a que el controlador quede libre y descarta flags rancios de
    /// operaciones anteriores. Para el ARRANQUE de una operación: un error
    /// viejo no debe abortar la nueva (a diferencia de [`Self::wait_done`]).
    fn clear_status(&mut self) {
        while Self::reg_read(SR) & SR_BSY != 0 {}
        Self::reg_write(SR, SR_ERR_MASK | SR_EOP);
    }

    /// Desbloquea FLASH_CR con la secuencia de claves. No-op si ya está libre.
    fn unlock(&mut self) {
        if Self::reg_read(CR) & CR_LOCK != 0 {
            Self::reg_write(KEYR, KEY1);
            Self::reg_write(KEYR, KEY2);
        }
    }

    /// Rebloquea FLASH_CR (limpia además PG/SER residuales).
    fn lock(&mut self) {
        Self::reg_write(CR, CR_LOCK);
    }

    /// `true` si `[addr, addr+len)` cabe dentro de la ventana.
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
        let base = (WINDOW_BASE + addr) as *const u8;
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
        self.clear_status();
        self.unlock();
        // PSIZE x8 + PG: cada escritura de byte dispara una programación.
        let cr = Self::reg_read(CR) & !(CR_PSIZE_MASK | CR_SER);
        Self::reg_write(CR, cr | CR_PG);
        let mut result = Ok(());
        let dst = (WINDOW_BASE + addr) as *mut u8;
        for (i, &b) in data.iter().enumerate() {
            // SAFETY: dirección dentro de la ventana validada; PG activo y
            // PSIZE x8, secuencia RM0090 §3.5.4 serializada por el handle.
            unsafe {
                core::ptr::write_volatile(dst.add(i), b);
            }
            result = self.wait_done();
            if result.is_err() {
                break;
            }
        }
        // Limpia PG y rebloquea pase lo que pase.
        self.lock();
        result?;
        // Verificación: relee y compara (detecta celdas no borradas/gastadas).
        let base = (WINDOW_BASE + addr) as *const u8;
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
        self.clear_status();
        self.unlock();
        // SER + SNB=11 + STRT: borrado del sector reservado (RM0090 §3.5.3).
        let cr = Self::reg_read(CR) & !(CR_PSIZE_MASK | CR_PG | (0b1111 << CR_SNB_SHIFT));
        Self::reg_write(CR, cr | CR_SER | (SECTOR_NUMBER << CR_SNB_SHIFT));
        Self::reg_write(CR, Self::reg_read(CR) | CR_STRT);
        let result = self.wait_done();
        self.lock();
        result
    }
}
