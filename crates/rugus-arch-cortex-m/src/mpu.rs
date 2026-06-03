//! MPU Cortex-M7 — 8 regiones, política priv/user por dominio Rugus.
//!
//! Con `PRIVDEFENA` el código privilegiado usa el mapa por defecto; las apps
//! userland solo acceden a regiones MPU marcadas para usuario.

use cortex_m::peripheral::MPU;

/// Índices HW de las 8 regiones (Cortex-M7).
pub mod region {
    /// Periféricos `0x4000_0000` — dominio [`Domain::Drivers`](rugus_core::domain::Domain::Drivers).
    pub const DRIVERS: u8 = 0;
    /// SDRAM — kernel heap, solo privilegiado.
    pub const SDRAM: u8 = 1;
    /// SRAM interna — datos kernel, solo privilegiado.
    pub const KERNEL_RAM: u8 = 2;
    /// Flash — RX user+priv (código app/kernel compartido en G2).
    pub const FLASH: u8 = 3;
    /// Stack de la app activa — RW user (remapeado en context switch).
    pub const APP_STACK: u8 = 4;
    /// Reservado — dominio Services (futuro G3+).
    pub const SERVICES: u8 = 5;
    /// Reservado — Secrets (priv R--, futuro).
    pub const SECRETS: u8 = 6;
    /// Guarda de pila: 32 B sin acceso en la base del stack de la tarea activa.
    /// Es la región de MAYOR número, así que gana la prioridad de solapamiento
    /// de ARMv7-M sobre `KERNEL_RAM`/`APP_STACK` y convierte un desbordamiento
    /// de pila (priv o user) en un MemManage limpio en vez de corrupción silente.
    pub const STACK_GUARD: u8 = 7;
}

/// Tamaños de región (campo SIZE del RASR): región cubre `2^(SIZE+1)` bytes.
mod size {
    pub const B_256K: u8 = 17;
    pub const B_512K: u8 = 18;
    pub const B_1M: u8 = 19;
    pub const B_2M: u8 = 20;
    pub const B_16M: u8 = 23;
    pub const B_512M: u8 = 28;
}

/// AP[2:0] en RASR — ver ARMv7-M ARM B3.5.
///
/// OJO con la codificación: `0b011` es RW priv + RW user (acceso completo),
/// mientras que `0b111` es RO/RO (solo lectura para ambos). Confundirlos deja
/// el stack de la app como solo-lectura y la primera escritura (el `push` del
/// prólogo) dispara MemManage aunque la región parezca correcta.
mod ap {
    /// Sin acceso para nadie (ni priv ni user) — guarda de pila.
    pub const NONE: u32 = 0b000 << 24;
    /// Priv RW, user sin acceso.
    pub const PRIV_RW: u32 = 0b001 << 24;
    /// Priv RO, user RO (flash ejecutable/lectura).
    pub const FULL_RO: u32 = 0b110 << 24;
    /// Priv RW, user RW (stack de app).
    pub const FULL_RW: u32 = 0b011 << 24;
}

/// Bytes guardados en la base de cada pila (campo SIZE 4 → región de 32 B).
const GUARD_SIZE_FIELD: u8 = 4;

const RASR_ENABLE: u32 = 1 << 0;

/// Bit XN (eXecute-Never) del RASR. Política W^X (F4.7): toda región escribible
/// (RAM kernel, SDRAM/heap, stack de app) se marca exec-never; el código vive
/// SOLO en flash (RX, write-never). Así una escritura maliciosa o accidental a
/// RAM no puede convertirse en código ejecutable (no hay W∧X en ninguna región).
const RASR_XN: u32 = 1 << 28;

/// TEX=0, C=1, B=1 — SRAM normal write-back (permite accesos no alineados M7).
const ATTR_NORMAL_WB: u32 = (1 << 17) | (1 << 16);
/// TEX=0, C=1, B=0 — flash normal read-only.
const ATTR_NORMAL_RO: u32 = 1 << 17;

/// Mapa de memoria por defecto (STM32F769), usado por la heurística
/// `domain_for_pc` y como rango de referencia. La configuración real de la MPU
/// la decide [`MpuLayout`], que cada placa pasa a [`init`].
pub mod layout {
    /// Flash interna.
    pub const FLASH_BASE: u32 = 0x0800_0000;
    /// Periféricos AHB/APB.
    pub const PERIPH_BASE: u32 = 0x4000_0000;
    /// SRAM interna (incluye `.data`/`.bss`/stacks kernel).
    pub const RAM_BASE: u32 = 0x2000_0000;
    /// SDRAM externa (heap G1).
    pub const SDRAM_BASE: u32 = 0xC000_0000;
}

/// Mapa de memoria y tamaños de región MPU específicos de la placa.
///
/// Los campos `*_size` son el campo SIZE del RASR (la región cubre
/// `2^(size+1)` bytes). Cada personalidad/placa con MPU debe pasar su propio
/// `MpuLayout` a [`init`] en vez de heredar el de otra placa: un tamaño de RAM
/// kernel demasiado grande o un base de flash equivocado dejarían huecos de
/// protección o faults espurios.
#[derive(Clone, Copy)]
pub struct MpuLayout {
    /// Base de la ventana de periféricos.
    pub periph_base: u32,
    /// Tamaño de la región de periféricos (campo SIZE).
    pub periph_size: u8,
    /// Base de la SDRAM externa (ignorado si `sdram_size == 0`).
    pub sdram_base: u32,
    /// Tamaño de la región SDRAM; `0` = sin SDRAM externa (región deshabilitada).
    pub sdram_size: u8,
    /// Base de la SRAM interna del kernel.
    pub ram_base: u32,
    /// Tamaño de la región de SRAM kernel (campo SIZE).
    pub ram_size: u8,
    /// Base de la flash interna.
    pub flash_base: u32,
    /// Tamaño de la región de flash (campo SIZE).
    pub flash_size: u8,
}

impl MpuLayout {
    /// STM32F769 (Cortex-M7): 512 K SRAM + SDRAM externa 16 M, flash 2 M.
    pub const STM32F769: Self = Self {
        periph_base: 0x4000_0000,
        periph_size: size::B_512M,
        sdram_base: 0xC000_0000,
        sdram_size: size::B_16M,
        ram_base: 0x2000_0000,
        ram_size: size::B_512K,
        flash_base: 0x0800_0000,
        flash_size: size::B_2M,
    };

    /// STM32F407 (Cortex-M4): 128 K SRAM (+CCM), sin SDRAM externa, flash 1 M.
    /// La región de RAM se redondea a 256 K (priv-only, cubrir de más es seguro).
    pub const STM32F407: Self = Self {
        periph_base: 0x4000_0000,
        periph_size: size::B_512M,
        sdram_base: 0,
        sdram_size: 0,
        ram_base: 0x2000_0000,
        ram_size: size::B_256K,
        flash_base: 0x0800_0000,
        flash_size: size::B_1M,
    };
}

/// Programa las regiones estáticas y habilita la MPU para la placa dada.
///
/// Debe llamarse una vez desde el kernel antes de arrancar tareas userland.
pub fn init(mpu: &mut MPU, layout: &MpuLayout) {
    disable(mpu);

    // Región 0: periféricos — Drivers, solo privilegiado (device, XN).
    configure_region(
        mpu,
        region::DRIVERS,
        layout.periph_base,
        layout.periph_size,
        ap::PRIV_RW,
        true,
        0,
    );
    // Región 1: SDRAM — kernel heap, solo privilegiado (si la placa la tiene).
    // W^X (F4.7): heap escribible ⇒ exec-never (xn=true).
    if layout.sdram_size == 0 {
        disable_region(mpu, region::SDRAM);
    } else {
        configure_region(
            mpu,
            region::SDRAM,
            layout.sdram_base,
            layout.sdram_size,
            ap::PRIV_RW,
            true,
            ATTR_NORMAL_WB,
        );
    }
    // Región 2: SRAM — kernel data/stacks, solo privilegiado.
    // W^X (F4.7): RAM escribible ⇒ exec-never (xn=true). El código del kernel
    // vive en flash (región 3, RX); nada se ejecuta desde SRAM.
    configure_region(
        mpu,
        region::KERNEL_RAM,
        layout.ram_base,
        layout.ram_size,
        ap::PRIV_RW,
        true,
        ATTR_NORMAL_WB,
    );
    // Región 3: flash — RX user+priv.
    configure_region(
        mpu,
        region::FLASH,
        layout.flash_base,
        layout.flash_size,
        ap::FULL_RO,
        false,
        ATTR_NORMAL_RO,
    );
    // Región 4: app stack — deshabilitada hasta el primer switch a app userland.
    disable_region(mpu, region::APP_STACK);

    // Regiones 5–6 sin usar; 7 es la guarda de pila (se programa en cada switch).
    disable_region(mpu, region::SERVICES);
    disable_region(mpu, region::SECRETS);
    disable_region(mpu, region::STACK_GUARD);

    // PRIVDEFENA: kernel privilegiado usa mapa por defecto además de MPU.
    const CTRL_PRIVDEFENA: u32 = 1 << 2;
    const CTRL_ENABLE: u32 = 1 << 0;
    // SAFETY: única escritura de MPU en init; regiones ya programadas.
    unsafe {
        mpu.ctrl.write(CTRL_ENABLE | CTRL_PRIVDEFENA);
    }
    sync_mpu();
}

/// Garantiza que los cambios en la configuración MPU surtan efecto antes de
/// cualquier acceso a memoria que dependa de ellos. ARMv7-M recomienda `DSB;
/// ISB` tras reprogramar regiones para que la nueva configuración rija en la
/// tarea recién conmutada sin depender de la sincronización implícita del
/// exception return.
fn sync_mpu() {
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
}

/// Precalcula los valores `RBAR`/`RASR` de la región [`region::APP_STACK`] para
/// una tarea userland con stack `[base, base+len)`.
///
/// Se guarda por tarea en su `Context` y lo escribe el propio context switch
/// (PendSV) de forma atómica con la conmutación de registros: así la región MPU
/// del stack SIEMPRE corresponde a la tarea que realmente se restaura, inmune a
/// cualquier entrelazado entre preempción (SysTick), cesión cooperativa y
/// reanudación tras fault (todos difieren el switch real al PendSV).
///
/// `RASR` ya trae el bit ENABLE; el `RBAR` NO incluye número de región ni bit
/// VALID porque el switch selecciona la región vía `MPU_RNR`.
pub fn app_region_for(base: u32, len: u32) -> (u32, u32) {
    let size = region_size_for(len as usize);
    let aligned = align_down(base, size);
    let rbar = aligned & !0x1F;
    // W^X (F4.7): el stack de la app es RW ⇒ exec-never. Userland no puede
    // ejecutar shellcode depositado en su propia pila (el código está en flash).
    let rasr = RASR_ENABLE | ap::FULL_RW | ATTR_NORMAL_WB | RASR_XN | ((size as u32) << 1);
    (rbar, rasr)
}

/// Precalcula los valores `RBAR`/`RASR` de la región [`region::STACK_GUARD`]
/// (guarda de pila de 32 B sin acceso) en la base `base` del stack de una tarea.
///
/// Igual que [`app_region_for`], se guarda por tarea en su `Context` y lo escribe
/// el propio context switch (PendSV/bootstrap) de forma atómica con la
/// conmutación: la guarda SIEMPRE cubre la base del stack de la tarea que se
/// restaura, inmune al entrelazado preempción/cesión/reanudación-tras-fault.
///
/// `base` es la dirección MÁS BAJA del stack (el extremo al que se acerca el SP
/// al crecer); cualquier acceso a esos 32 B dispara MemManage antes de desbordar
/// a la RAM vecina. La región 7 gana la prioridad de solapamiento sobre
/// `KERNEL_RAM`/`APP_STACK`, así que protege tanto a tareas privilegiadas como
/// userland. `RASR` ya trae ENABLE + XN; `RBAR` sin nº de región ni VALID.
pub fn guard_region_for(base: u32) -> (u32, u32) {
    let rbar = base & !0x1F;
    let rasr = RASR_ENABLE | ap::NONE | ATTR_NORMAL_WB | RASR_XN | ((GUARD_SIZE_FIELD as u32) << 1);
    (rbar, rasr)
}

/// Audita la invariante W^X (F4.7) sobre las regiones MPU actualmente
/// programadas: ninguna región habilitada puede ser a la vez escribible Y
/// ejecutable. Devuelve `true` si la política se cumple en las 8 regiones.
///
/// "Escribible" = AP ∈ {priv-RW `0b001`, full-RW `0b011`}; "ejecutable" = bit XN
/// a 0. Pensado para llamarse al arranque tras [`init`] (las regiones por-tarea
/// `APP_STACK`/`STACK_GUARD` aún no están activas, pero sus generadores
/// `app_region_for`/`guard_region_for` ya fijan XN, así que el barrido por
/// switch tampoco rompe la invariante). Defensa en profundidad: detecta una
/// regresión de atributos antes de exponer la superficie a userland.
pub fn audit_wx(mpu: &mut MPU) -> bool {
    const AP_PRIV_RW: u32 = 0b001;
    const AP_FULL_RW: u32 = 0b011;
    for rn in 0u8..8 {
        // SAFETY: lectura de registros MPU; RNR exclusivo en este barrido único.
        let rasr = unsafe {
            mpu.rnr.write(rn as u32);
            mpu.rasr.read()
        };
        if rasr & RASR_ENABLE == 0 {
            continue;
        }
        let ap = (rasr >> 24) & 0b111;
        let writable = ap == AP_PRIV_RW || ap == AP_FULL_RW;
        let executable = rasr & RASR_XN == 0;
        if writable && executable {
            return false;
        }
    }
    true
}

fn configure_region(mpu: &mut MPU, rn: u8, base: u32, size: u8, ap: u32, xn: bool, attr: u32) {
    let rbar = base & !0x1F;
    let xn_bit = if xn { RASR_XN } else { 0 };
    let rasr = RASR_ENABLE | ap | attr | xn_bit | ((size as u32) << 1);
    // SAFETY: MPU deshabilitada o RNR exclusivo en init/switch cooperativo.
    unsafe {
        mpu.rnr.write(rn as u32);
        mpu.rbar.write(rbar);
        mpu.rasr.write(rasr);
    }
}

fn disable_region(mpu: &mut MPU, rn: u8) {
    unsafe {
        mpu.rnr.write(rn as u32);
        mpu.rasr.write(0);
    }
}

fn disable(mpu: &mut MPU) {
    unsafe {
        mpu.ctrl.write(0);
    }
}

/// Calcula el campo SIZE mínimo que cubre `len` bytes (potencia de 2, ≥32 B).
pub fn region_size_for(len: usize) -> u8 {
    let mut need = len.max(32);
    if !need.is_power_of_two() {
        need = need.next_power_of_two();
    }
    let mut size = 4u8;
    while (1usize << (size as u32 + 1)) < need {
        size += 1;
    }
    size
}

/// Alinea `addr` hacia abajo al límite de la región MPU.
pub fn align_down(addr: u32, size_field: u8) -> u32 {
    let region_bytes = 1u32 << (size_field as u32 + 1);
    addr & !(region_bytes - 1)
}
