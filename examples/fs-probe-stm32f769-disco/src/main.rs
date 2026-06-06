//! Rugus F5.C.2 — montaje y uso de `rugus-fs` sobre la QSPI NOR de la
//! STM32F769I-DISCO (Macronix MX25L51245G).
//!
//! Valida el almacén clave-valor *log-structured* [`rugus_fs::Rufs`] contra
//! hardware real:
//!
//! 1. Inicializa relojes + I/D-cache y crea el driver QSPI (`BlockDevice`).
//! 2. **Monta** la FS sobre la flash (escaneo + reconstrucción de índice).
//! 3. Lee un **contador de arranques** persistente, lo incrementa y lo reescribe
//!    (demuestra persistencia real entre power-cycles).
//! 4. Escribe un par clave/valor, lo relee y verifica.
//! 5. **Remonta** la FS en el mismo arranque y comprueba que el contador y la
//!    clave sobreviven al re-escaneo (prueba del camino de montaje).
//!
//! LED rojo (LD1/PJ13): parpadeo rápido = OK; fijo encendido = fallo. Todo el
//! detalle sale por SWD/RTT (defmt).

#![no_std]
#![no_main]

use rugus_runtime as _; // panic-probe + defmt-rtt
use rugus_runtime::entry;

use rugus_fs::{Error as FsError, Rufs};
use rugus_hal::GpioPin;
use rugus_hal_stm32f7::cache;
use rugus_hal_stm32f7::gpio::{DiscoLed, LedPin};
use rugus_hal_stm32f7::pac;
use rugus_hal_stm32f7::qspi::Qspi;
use rugus_hal_stm32f7::rcc;

/// Capacidad del índice en RAM (claves vivas + tombstones). Sobra para el probe.
const FS_KEYS: usize = 16;

const KEY_BOOTS: &[u8] = b"boot_count";
const KEY_HELLO: &[u8] = b"hello";
const VAL_HELLO: &[u8] = b"rugus-fs sobre MX25L51245G";

#[entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("cortex-m peripherals already taken");
    let dp = pac::Peripherals::take().expect("device peripherals already taken");

    let clocks = rcc::init(&dp);
    cache::enable(&mut cp.SCB, &mut cp.CPUID);
    rugus_runtime::enable_cycle_counter(&mut cp);

    defmt::info!(
        "rugus fs-probe @ STM32F769I-DISCO, SYSCLK {} MHz",
        clocks.sysclk_mhz()
    );

    let mut led = LedPin::new(&dp.RCC, DiscoLed::Red);

    match run(dp.QUADSPI, &dp.RCC) {
        Ok(boots) => {
            defmt::info!("FS PROBE OK — arranque nº {} (persistente)", boots);
            loop {
                led.toggle().ok();
                cortex_m::asm::delay(20_000_000);
            }
        }
        Err(e) => {
            defmt::error!("FS PROBE FALLO: {}", e);
            led.set_low().ok();
            loop {
                cortex_m::asm::wfi();
            }
        }
    }
}

/// Resultado legible para defmt.
#[derive(defmt::Format)]
enum ProbeError {
    QspiInit,
    Mount,
    SetGet,
    Verify,
    Remount,
}

fn run(qspi: pac::QUADSPI, rcc: &pac::RCC) -> Result<u32, ProbeError> {
    let flash = Qspi::new(qspi, rcc).map_err(|e| {
        defmt::error!("Qspi::new error: {}", defmt::Debug2Format(&e));
        ProbeError::QspiInit
    })?;

    // 1) Montar la FS sobre la flash.
    let mut fs = Rufs::<_, FS_KEYS>::mount(flash).map_err(|e| {
        defmt::error!("mount error: {}", defmt::Debug2Format(&e));
        ProbeError::Mount
    })?;
    defmt::info!("FS montada: {} claves vivas", fs.len());

    // 2) Contador de arranques persistente.
    let mut buf = [0u8; 4];
    let prev = match fs.get(KEY_BOOTS, &mut buf) {
        Ok(4) => u32::from_le_bytes(buf),
        Ok(_) => 0,
        Err(FsError::NotFound) => 0,
        Err(e) => {
            defmt::error!("get boot_count error: {}", defmt::Debug2Format(&e));
            return Err(ProbeError::SetGet);
        }
    };
    let boots = prev + 1;
    fs.set(KEY_BOOTS, &boots.to_le_bytes()).map_err(|e| {
        defmt::error!("set boot_count error: {}", defmt::Debug2Format(&e));
        ProbeError::SetGet
    })?;
    defmt::info!("boot_count: {} -> {}", prev, boots);

    // 3) Par clave/valor de prueba + verificación.
    fs.set(KEY_HELLO, VAL_HELLO)
        .map_err(|_| ProbeError::SetGet)?;
    let mut rb = [0u8; 64];
    let n = fs.get(KEY_HELLO, &mut rb).map_err(|_| ProbeError::Verify)?;
    if &rb[..n] != VAL_HELLO {
        defmt::error!("read-back no coincide: {=[u8]:a}", rb[..n]);
        return Err(ProbeError::Verify);
    }
    defmt::info!("set/get OK ('{=[u8]:a}', {} B)", VAL_HELLO, n);

    // 4) Remontar en el mismo arranque: recuperar el dispositivo y re-escanear.
    let flash = fs.into_device();
    let mut fs2 = Rufs::<_, FS_KEYS>::mount(flash).map_err(|e| {
        defmt::error!("remount error: {}", defmt::Debug2Format(&e));
        ProbeError::Remount
    })?;
    let mut vb = [0u8; 4];
    let again = match fs2.get(KEY_BOOTS, &mut vb) {
        Ok(4) => u32::from_le_bytes(vb),
        _ => return Err(ProbeError::Remount),
    };
    let m = fs2
        .get(KEY_HELLO, &mut rb)
        .map_err(|_| ProbeError::Remount)?;
    if again != boots || &rb[..m] != VAL_HELLO {
        defmt::error!("remontaje inconsistente: boots={} hello_len={}", again, m);
        return Err(ProbeError::Remount);
    }
    defmt::info!(
        "remontaje OK: boot_count={} y clave 'hello' intactos",
        again
    );

    Ok(boots)
}
