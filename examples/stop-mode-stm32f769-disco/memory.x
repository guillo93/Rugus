/* STM32F769NIH6 — memory map para el ejemplo STOP mode de Rugus.
 *
 * Mapa mínimo que cortex-m-rt espera (FLASH + RAM). RAM = SRAM1 (368 KB),
 * suficiente para las tareas de la demo. DTCM/ITCM/SDRAM se añaden cuando
 * un ejemplo las requiera.
 */

MEMORY
{
  FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 2048K
  RAM   (rwx) : ORIGIN = 0x20020000, LENGTH = 368K
}
