/* STM32F769NIH6 — memory map para el ejemplo blink de Rugus.
 *
 * Mapa mínimo que cortex-m-rt espera (FLASH + RAM). En ejemplos futuros
 * que usen SDRAM/ITCM/DTCM explícitamente, este archivo se extenderá con
 * las regiones correspondientes (ver `crates/rugus-hal-stm32f7/` doc).
 *
 * Total SRAM del chip 512 KB; aquí mapeamos RAM = SRAM1 (368 KB), suficiente
 * para el blink. DTCM/ITCM/SDRAM se añaden cuando un ejemplo las requiera.
 */

MEMORY
{
  FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 2048K
  RAM   (rwx) : ORIGIN = 0x20020000, LENGTH = 368K
}
