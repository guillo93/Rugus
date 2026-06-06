//! QUADSPI — NOR flash Quad-SPI en STM32F769I-DISCO (F5.C.1).
//!
//! Driver del controlador QUADSPI del STM32F769 contra la flash NOR **Macronix
//! MX25L51245G** (512 Mbit = 64 MiB) soldada en la DISCO (UM2033 §5.14 la
//! describe genéricamente como "512-Mbit Quad-SPI NOR"; el JEDEC ID real
//! `C2 20 1A` confirma Macronix). Implementa
//! el trait [`rugus_hal::BlockDevice`]: lectura arbitraria, programación por
//! página (256 B) y borrado por subsector (4 KiB).
//!
//! ## Hardware (UM2033, schematics)
//!
//! | Señal | Pin | AF |
//! |-------|-----|----|
//! | QUADSPI_CLK      | PB2  | AF9  |
//! | QUADSPI_BK1_NCS  | PB6  | AF10 |
//! | QUADSPI_BK1_IO0  | PC9  | AF9  |
//! | QUADSPI_BK1_IO1  | PC10 | AF9  |
//! | QUADSPI_BK1_IO2  | PE2  | AF9  |
//! | QUADSPI_BK1_IO3  | PD13 | AF9  |
//!
//! ## Modo de operación
//!
//! Para bringup robusto se usa **modo single-line (1-1-1)** e **indirecto**
//! (sin memory-mapped): IO0/IO1 actúan como MOSI/MISO. Esto basta para validar
//! el medio y dar un `BlockDevice` funcional; el modo quad (4 líneas) y el
//! memory-mapped quedan como optimización posterior. La flash se conmuta a
//! **direccionamiento de 4 bytes** (comando 0xB7) porque 64 MiB excede el rango
//! de 24 bits.

use crate::pac;
use core::ptr;
use rugus_hal::BlockDevice;

/// Capacidad de la MX25L51245G: 512 Mbit = 64 MiB.
pub const CAPACITY: u64 = 64 * 1024 * 1024;
/// Tamaño de página (page program).
pub const PAGE_SIZE: usize = 256;
/// Tamaño de subsector borrable.
pub const SUBSECTOR_SIZE: usize = 4 * 1024;
/// `FSIZE` para DCR: `log2(64 MiB) - 1 = 26 - 1 = 25`.
const FSIZE: u8 = 25;

// Comandos MX25L51245G (single-line; estándar JEDEC SFDP).
const CMD_RDID: u8 = 0x9F; // JEDEC ID (3 bytes: fabricante, tipo, capacidad)
const CMD_RDSR: u8 = 0x05; // Read status register
const CMD_WREN: u8 = 0x06; // Write enable
const CMD_ENTER_4B: u8 = 0xB7; // Enter 4-byte address mode
const CMD_FAST_READ: u8 = 0x0B; // Fast read (con dummy cycles)
const CMD_PAGE_PROGRAM: u8 = 0x02; // Page program
const CMD_SUBSECTOR_ERASE: u8 = 0x20; // 4 KiB subsector erase

const SR_WIP: u8 = 0x01; // Status register: write-in-progress

/// JEDEC ID byte 0 esperado: fabricante Macronix.
pub const JEDEC_MACRONIX: u8 = 0xC2;
/// JEDEC ID byte 2 esperado: capacidad 512 Mbit (MX25L51245G = `0x1A`).
pub const JEDEC_CAP_512M: u8 = 0x1A;

/// Errores del driver QSPI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QspiError {
    /// El controlador no completó la transacción a tiempo (timeout HW).
    Timeout,
    /// JEDEC ID no coincide con la MX25L51245G esperada.
    UnknownDevice([u8; 3]),
    /// Dirección o longitud fuera del medio.
    OutOfRange,
}

/// Modo de fase de datos en CCR.
#[derive(Clone, Copy)]
enum DataDir {
    /// Sin fase de datos.
    None,
    /// Lectura indirecta (FMODE=01).
    Read,
    /// Escritura indirecta (FMODE=00).
    Write,
}

/// Driver QUADSPI para la flash NOR de la F769I-DISCO.
pub struct Qspi {
    qspi: pac::QUADSPI,
}

impl Qspi {
    /// Inicializa pines, reloj y controlador, conmuta la flash a 4-byte mode y
    /// verifica el JEDEC ID. Consume el periférico `QUADSPI` del PAC.
    pub fn new(qspi: pac::QUADSPI, rcc: &pac::RCC) -> Result<Self, QspiError> {
        configure_pins(rcc);
        enable_clock(rcc);

        // Controlador: prescaler /8 (216/8 = 27 MHz, conservador para bringup),
        // FTHRES=0 (1 byte), CKMODE=0 (modo 0), CSHT=1 (2 ciclos de CS alto).
        qspi.cr.modify(|_, w| unsafe {
            w.prescaler().bits(7);
            w.fthres().bits(0);
            w.en().set_bit()
        });
        qspi.dcr.modify(|_, w| unsafe {
            w.fsize().bits(FSIZE);
            w.csht().bits(1);
            w.ckmode().clear_bit()
        });

        let mut dev = Self { qspi };
        dev.enter_4byte_mode()?;

        let mut id = [0u8; 3];
        dev.read_jedec(&mut id)?;
        if id[0] != JEDEC_MACRONIX || id[2] != JEDEC_CAP_512M {
            return Err(QspiError::UnknownDevice(id));
        }
        Ok(dev)
    }

    /// Lee el JEDEC ID (3 bytes) de la flash.
    pub fn read_jedec(&mut self, out: &mut [u8; 3]) -> Result<(), QspiError> {
        self.command(CMD_RDID, None, 0, DataDir::Read, Some(out))
    }

    /// Conmuta la flash a direccionamiento de 4 bytes (necesario >16 MiB).
    fn enter_4byte_mode(&mut self) -> Result<(), QspiError> {
        self.write_enable()?;
        self.command(CMD_ENTER_4B, None, 0, DataDir::None, None)
    }

    fn write_enable(&mut self) -> Result<(), QspiError> {
        self.command(CMD_WREN, None, 0, DataDir::None, None)
    }

    /// Espera a que termine la operación interna de la flash (WIP=0).
    fn wait_busy(&mut self) -> Result<(), QspiError> {
        // Sondeo por software del bit WIP del status register.
        for _ in 0..50_000_000u32 {
            let mut sr = [0u8; 1];
            self.command(CMD_RDSR, None, 0, DataDir::Read, Some(&mut sr))?;
            if sr[0] & SR_WIP == 0 {
                return Ok(());
            }
        }
        Err(QspiError::Timeout)
    }

    /// Ejecuta una transacción QUADSPI en modo single-line indirecto.
    ///
    /// `instruction` es el opcode; `address` la dirección opcional (4 bytes);
    /// `dummy` ciclos de dummy; `dir` la dirección de la fase de datos y `data`
    /// el buffer (lectura: se rellena; escritura: se envía).
    fn command(
        &mut self,
        instruction: u8,
        address: Option<u32>,
        dummy: u8,
        dir: DataDir,
        data: Option<&mut [u8]>,
    ) -> Result<(), QspiError> {
        // Espera a que el controlador esté libre de una transacción previa.
        self.wait_not_busy_ctrl()?;

        // Limpia banderas de transferencia previas.
        self.qspi.fcr.write(|w| w.ctcf().set_bit().ctef().set_bit());

        // Longitud de datos: DLR = nbytes - 1 (solo si hay datos).
        let nbytes = match (&dir, &data) {
            (DataDir::None, _) | (_, None) => 0,
            (_, Some(buf)) => buf.len(),
        };
        if nbytes > 0 {
            self.qspi
                .dlr
                .write(|w| unsafe { w.dl().bits((nbytes - 1) as u32) });
        }

        // CCR: instrucción single-line; address single-line 32-bit si aplica;
        // data single-line según `dir`; dummy cycles.
        let fmode: u8 = match dir {
            DataDir::None | DataDir::Write => 0b00,
            DataDir::Read => 0b01,
        };
        let admode: u8 = if address.is_some() { 0b01 } else { 0b00 };
        let dmode: u8 = match dir {
            DataDir::None => 0b00,
            _ => 0b01,
        };
        self.qspi.ccr.write(|w| unsafe {
            w.fmode().bits(fmode);
            w.imode().bits(0b01); // instrucción en single-line
            w.admode().bits(admode);
            w.adsize().bits(0b11); // 32-bit address
            w.dmode().bits(dmode);
            w.dcyc().bits(dummy);
            w.instruction().bits(instruction)
        });

        // Con fase de dirección, escribir AR dispara la transacción.
        if let Some(addr) = address {
            self.qspi.ar.write(|w| unsafe { w.address().bits(addr) });
        }

        match dir {
            DataDir::Read => self.read_data(data)?,
            DataDir::Write => self.write_data(data)?,
            DataDir::None => {}
        }

        // Espera a TCF (transfer complete) y limpia.
        self.wait_tcf()?;
        self.qspi.fcr.write(|w| w.ctcf().set_bit());
        Ok(())
    }

    fn read_data(&mut self, data: Option<&mut [u8]>) -> Result<(), QspiError> {
        let Some(buf) = data else { return Ok(()) };
        let dr = self.qspi.dr.as_ptr() as *const u8;
        for byte in buf.iter_mut() {
            self.wait_ftf()?;
            // Lectura de 8 bits del FIFO de datos.
            *byte = unsafe { ptr::read_volatile(dr) };
        }
        Ok(())
    }

    fn write_data(&mut self, data: Option<&mut [u8]>) -> Result<(), QspiError> {
        let Some(buf) = data else { return Ok(()) };
        let dr = self.qspi.dr.as_ptr() as *mut u8;
        for byte in buf.iter() {
            self.wait_ftf()?;
            unsafe { ptr::write_volatile(dr, *byte) };
        }
        Ok(())
    }

    fn wait_not_busy_ctrl(&self) -> Result<(), QspiError> {
        for _ in 0..0x00FF_FFFFu32 {
            if self.qspi.sr.read().busy().bit_is_clear() {
                return Ok(());
            }
        }
        Err(QspiError::Timeout)
    }

    fn wait_ftf(&self) -> Result<(), QspiError> {
        for _ in 0..0x00FF_FFFFu32 {
            if self.qspi.sr.read().ftf().bit_is_set() {
                return Ok(());
            }
        }
        Err(QspiError::Timeout)
    }

    fn wait_tcf(&self) -> Result<(), QspiError> {
        for _ in 0..0x00FF_FFFFu32 {
            if self.qspi.sr.read().tcf().bit_is_set() {
                return Ok(());
            }
        }
        Err(QspiError::Timeout)
    }
}

impl BlockDevice for Qspi {
    type Error = QspiError;

    fn capacity(&self) -> u64 {
        CAPACITY
    }

    fn prog_size(&self) -> usize {
        PAGE_SIZE
    }

    fn erase_size(&self) -> usize {
        SUBSECTOR_SIZE
    }

    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), QspiError> {
        if (addr as u64) + (buf.len() as u64) > CAPACITY {
            return Err(QspiError::OutOfRange);
        }
        if buf.is_empty() {
            return Ok(());
        }
        // FAST_READ con 8 dummy cycles (single-line).
        self.command(CMD_FAST_READ, Some(addr), 8, DataDir::Read, Some(buf))
    }

    fn program(&mut self, addr: u32, data: &[u8]) -> Result<(), QspiError> {
        if (addr as u64) + (data.len() as u64) > CAPACITY {
            return Err(QspiError::OutOfRange);
        }
        // Page program: no puede cruzar el límite de página de 256 B; troceamos.
        let mut offset = 0usize;
        let mut cur = addr;
        while offset < data.len() {
            let page_end = (cur & !(PAGE_SIZE as u32 - 1)) + PAGE_SIZE as u32;
            let chunk = core::cmp::min(data.len() - offset, (page_end - cur) as usize);
            self.write_enable()?;
            // `command` toma `&mut [u8]`; copiamos el chunk a través de un buffer
            // mutable temporal en stack del tamaño de página.
            let mut tmp = [0u8; PAGE_SIZE];
            tmp[..chunk].copy_from_slice(&data[offset..offset + chunk]);
            self.command(
                CMD_PAGE_PROGRAM,
                Some(cur),
                0,
                DataDir::Write,
                Some(&mut tmp[..chunk]),
            )?;
            self.wait_busy()?;
            offset += chunk;
            cur += chunk as u32;
        }
        Ok(())
    }

    fn erase_sector(&mut self, addr: u32) -> Result<(), QspiError> {
        if (addr as u64) >= CAPACITY {
            return Err(QspiError::OutOfRange);
        }
        let base = addr & !(SUBSECTOR_SIZE as u32 - 1);
        self.write_enable()?;
        self.command(CMD_SUBSECTOR_ERASE, Some(base), 0, DataDir::None, None)?;
        self.wait_busy()
    }
}

/// Habilita el reloj del periférico QUADSPI (AHB3).
fn enable_clock(rcc: &pac::RCC) {
    rcc.ahb3enr.modify(|_, w| w.qspien().set_bit());
    let _ = rcc.ahb3enr.read().bits();
}

/// Configura los 6 pines QSPI (UM2033) como AF push-pull very-high-speed.
fn configure_pins(rcc: &pac::RCC) {
    rcc.ahb1enr.modify(|_, w| {
        w.gpioben().enabled();
        w.gpiocen().enabled();
        w.gpioden().enabled();
        w.gpioeen().enabled()
    });
    let _ = rcc.ahb1enr.read().bits();

    // (puerto, pin, AF). GPIOB/GPIOC tienen tipos `RegisterBlock` distintos en
    // el PAC pero el layout es idéntico al de `gpiod`; casteamos el puntero.
    type Gpio = pac::gpiod::RegisterBlock;
    af_pin(unsafe { &*(pac::GPIOB::ptr() as *const Gpio) }, 2, 9); // CLK
    af_pin(unsafe { &*(pac::GPIOB::ptr() as *const Gpio) }, 6, 10); // NCS
    af_pin(unsafe { &*(pac::GPIOC::ptr() as *const Gpio) }, 9, 9); // IO0
    af_pin(unsafe { &*(pac::GPIOC::ptr() as *const Gpio) }, 10, 9); // IO1
    af_pin(unsafe { &*pac::GPIOE::ptr() }, 2, 9); // IO2
    af_pin(unsafe { &*pac::GPIOD::ptr() }, 13, 9); // IO3
}

fn af_pin(port: &pac::gpiod::RegisterBlock, pin: u8, af: u32) {
    const OSPEED_VERY_HIGH: u32 = 0b11;
    let bit = pin as u32;
    let shift = bit * 2;
    // MODER = AF (0b10)
    port.moder
        .modify(|r, w| unsafe { w.bits((r.bits() & !(0b11 << shift)) | (0b10 << shift)) });
    // OTYPER = push-pull (0)
    port.otyper
        .modify(|r, w| unsafe { w.bits(r.bits() & !(1 << bit)) });
    // OSPEEDR = very-high
    port.ospeedr.modify(|r, w| unsafe {
        w.bits((r.bits() & !(0b11 << shift)) | (OSPEED_VERY_HIGH << shift))
    });
    // PUPDR = none (0)
    port.pupdr
        .modify(|r, w| unsafe { w.bits(r.bits() & !(0b11 << shift)) });
    // AFR
    let afr_shift = (bit % 8) * 4;
    if bit < 8 {
        port.afrl
            .modify(|r, w| unsafe { w.bits((r.bits() & !(0xF << afr_shift)) | (af << afr_shift)) });
    } else {
        port.afrh
            .modify(|r, w| unsafe { w.bits((r.bits() & !(0xF << afr_shift)) | (af << afr_shift)) });
    }
}
