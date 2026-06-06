//! I/D-Cache del Cortex-M7 — activar tras estabilizar el reloj del sistema.
//!
//! Usa las rutinas de `cortex-m` que incluyen invalidación y barreras DSB/ISB.
//!
//! Los anillos ETH DMA deben vivir en [`ETH_DMA_BASE`] y configurarse con
//! [`configure_eth_mpu`] **antes** de habilitar la D-cache (BSP ST F769).

use cortex_m::peripheral::{CPUID, MPU, SCB};

/// Descriptores/buffers ETH en `.eth_dma` — alineado a 16 KiB para MPU (ver `memory.x`).
pub const ETH_DMA_BASE: u32 = 0x2007_8000;
const ETH_DMA_MPU_SIZE: u8 = 13; // 2^(13+1) = 16 KiB

const MPU_RASR_ENABLE: u32 = 1;
const MPU_ATTR_NORMAL_NONCACHE: u32 = (1 << 19) | (1 << 18); // TEX=001, S=1, C=0, B=0
/// AP=011 — full access (privileged + unprivileged RW).
const MPU_AP_FULL: u32 = 0b011 << 24;
/// XN (eXecute Never) — DMA buffers must never be executed as code.
const MPU_RASR_XN: u32 = 1 << 28;

/// Marca la región ETH DMA como no cacheable (MPU región 1).
///
/// Llamar **antes** de [`enable`]. Los buffers en `.eth_dma` deben estar en
/// [`ETH_DMA_BASE`] (16 KiB @ `0x2007_8000`, ver `memory.x` de los ejemplos G4).
///
/// Secuencia BSP ST + Cortex-M7 ARMv7-M ARM B3.5:
/// 1. MPU off (`CTRL=0`) para reconfigurar sin race con Mem/Bus fault.
/// 2. Programar región 6 (RBAR/RASR) como Normal-Non-Cacheable, full access, XN
///    — número alto para ganar la prioridad de solapamiento si hay un mapa MPU
///    previo (p. ej. `platform_init`).
/// 3. MPU on con `PRIVDEFENA` para que el resto del mapa siga gobernado por
///    los atributos por defecto.
pub fn configure_eth_mpu(mpu: &mut MPU) {
    // SAFETY: MPU es singleton vía `&mut MPU`; ETH_DMA_BASE alineado a 16 KiB
    // por linker (ver `memory.x` de eth-link / https-get).
    unsafe {
        mpu.ctrl.write(0);
        cortex_m::asm::dsb();
        cortex_m::asm::isb();

        // Región 6 (alta): en ARMv7-M, ante solapamiento de regiones gana la de
        // MAYOR número. Si el arranque ya programó un mapa MPU completo (p. ej.
        // `platform_init`, cuya región KERNEL_RAM=2 cubre toda la SRAM interna
        // 0x2000_0000–0x2007_FFFF e INCLUYE la ventana ETH), una región ETH baja
        // (1) perdería la prioridad y los descriptores DMA quedarían cacheables:
        // el motor leería OWN=0 obsoleto de RAM y el TX se colgaría en estado
        // *suspended*. Con la región 6 la ventana ETH gana siempre y permanece
        // Normal-Non-Cacheable. Es inocuo cuando se usa de forma aislada
        // (eth-link/https-get): no hay solapamiento y la 6 está libre.
        mpu.rnr.write(6);
        mpu.rbar.write(ETH_DMA_BASE);
        mpu.rasr.write(
            MPU_RASR_ENABLE
                | MPU_AP_FULL
                | MPU_ATTR_NORMAL_NONCACHE
                | MPU_RASR_XN
                | ((ETH_DMA_MPU_SIZE as u32) << 1),
        );

        mpu.ctrl.write(1 | (1 << 2)); // ENABLE | PRIVDEFENA
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
    }
}

/// Habilita I-cache y D-cache del M7 si aún están apagadas.
///
/// Debe llamarse después de [`crate::rcc::init`] cuando SYSCLK ya corre a
/// la frecuencia objetivo.
pub fn enable(scb: &mut SCB, cpuid: &mut CPUID) {
    scb.enable_icache();
    scb.enable_dcache(cpuid);
}

/// MPU no-cache para `.eth_dma` + I/D-cache — orden requerido en ejemplos G4.
pub fn enable_with_eth_dma(scb: &mut SCB, cpuid: &mut CPUID, mpu: &mut MPU) {
    configure_eth_mpu(mpu);
    enable(scb, cpuid);
}

const CACHE_LINE: usize = 32;

fn dma_range(ptr: *const u8, len: usize) -> (usize, usize) {
    if len == 0 {
        return (0, 0);
    }
    let start = ptr as usize & !(CACHE_LINE - 1);
    let end = ptr as usize + len;
    let end_aligned = (end + CACHE_LINE - 1) & !(CACHE_LINE - 1);
    (start, end_aligned.saturating_sub(start))
}

/// Write-back CPU writes so Ethernet DMA sees fresh TX frame bytes.
pub fn clean_dcache_for_dma(data: &[u8]) {
    if !SCB::dcache_enabled() {
        return;
    }
    let (start, len) = dma_range(data.as_ptr(), data.len());
    if len == 0 {
        return;
    }
    // SAFETY: SCB is a singleton; address range covers the DMA buffer slice.
    unsafe {
        cortex_m::Peripherals::steal()
            .SCB
            .clean_dcache_by_address(start, len);
    }
}

/// Drop stale cache lines after Ethernet DMA wrote an RX frame.
pub fn invalidate_dcache_for_dma(data: &[u8]) {
    if !SCB::dcache_enabled() {
        return;
    }
    let (start, len) = dma_range(data.as_ptr(), data.len());
    if len == 0 {
        return;
    }
    // SAFETY: SCB is a singleton; address range covers the DMA buffer slice.
    unsafe {
        cortex_m::Peripherals::steal()
            .SCB
            .invalidate_dcache_by_address(start, len);
    }
}
