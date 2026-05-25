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
    /// Reservado.
    pub const SPARE: u8 = 7;
}

/// Tamaños de región (campo SIZE del RASR): región cubre `2^(SIZE+1)` bytes.
mod size {
    pub const B_512K: u8 = 18;
    pub const B_2M: u8 = 20;
    pub const B_16M: u8 = 23;
    pub const B_512M: u8 = 28;
}

/// AP[2:0] en RASR — ver ARMv7-M ARM.
mod ap {
    pub const PRIV_RW: u32 = 0b001 << 24;
    pub const FULL_RO: u32 = 0b110 << 24;
    pub const FULL_RW: u32 = 0b111 << 24;
}

const RASR_ENABLE: u32 = 1 << 0;

/// TEX=0, C=1, B=1 — SRAM normal write-back (permite accesos no alineados M7).
const ATTR_NORMAL_WB: u32 = (1 << 17) | (1 << 16);
/// TEX=0, C=1, B=0 — flash normal read-only.
const ATTR_NORMAL_RO: u32 = 1 << 17;

/// Mapa de memoria STM32F769 usado por los ejemplos Rugus.
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

/// Programa las regiones estáticas y habilita la MPU.
///
/// Debe llamarse una vez desde el kernel antes de arrancar tareas userland.
pub fn init(mpu: &mut MPU) {
    disable(mpu);

    // Región 0: periféricos — Drivers, solo privilegiado (device, XN).
    configure_region(
        mpu,
        region::DRIVERS,
        layout::PERIPH_BASE,
        size::B_512M,
        ap::PRIV_RW,
        true,
        0,
    );
    // Región 1: SDRAM — kernel heap, solo privilegiado.
    configure_region(
        mpu,
        region::SDRAM,
        layout::SDRAM_BASE,
        size::B_16M,
        ap::PRIV_RW,
        false,
        ATTR_NORMAL_WB,
    );
    // Región 2: SRAM — kernel data/stacks, solo privilegiado.
    configure_region(
        mpu,
        region::KERNEL_RAM,
        layout::RAM_BASE,
        size::B_512K,
        ap::PRIV_RW,
        false,
        ATTR_NORMAL_WB,
    );
    // Región 3: flash — RX user+priv.
    configure_region(
        mpu,
        region::FLASH,
        layout::FLASH_BASE,
        size::B_2M,
        ap::FULL_RO,
        false,
        ATTR_NORMAL_RO,
    );
    // Región 4: app stack — deshabilitada hasta el primer switch a app userland.
    disable_region(mpu, region::APP_STACK);

    // Regiones 5–7 sin usar.
    disable_region(mpu, region::SERVICES);
    disable_region(mpu, region::SECRETS);
    disable_region(mpu, region::SPARE);

    // PRIVDEFENA: kernel privilegiado usa mapa por defecto además de MPU.
    const CTRL_PRIVDEFENA: u32 = 1 << 2;
    const CTRL_ENABLE: u32 = 1 << 0;
    // SAFETY: única escritura de MPU en init; regiones ya programadas.
    unsafe {
        mpu.ctrl.write(CTRL_ENABLE | CTRL_PRIVDEFENA);
    }
}

/// Remapea la región de stack de la app activa (dominio App).
///
/// `stack_base` debe estar alineado al tamaño de región (`2^(size+1)`).
pub fn remap_app_stack(mpu: &mut MPU, stack_base: u32, size: u8) {
    configure_region(
        mpu,
        region::APP_STACK,
        stack_base,
        size,
        ap::FULL_RW,
        false,
        ATTR_NORMAL_WB,
    );
}

/// Deshabilita la región de stack app (p. ej. al volver a tarea kernel).
pub fn clear_app_stack(mpu: &mut MPU) {
    disable_region(mpu, region::APP_STACK);
}

fn configure_region(mpu: &mut MPU, rn: u8, base: u32, size: u8, ap: u32, xn: bool, attr: u32) {
    let rbar = base & !0x1F;
    let xn_bit = if xn { 1u32 << 28 } else { 0 };
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
