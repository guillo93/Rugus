/* STM32F769NIH6 — SRAM1 for ETH descriptor rings + smoltcp (G4 step 1). */

MEMORY
{
  FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 2048K
  RAM   (rwx) : ORIGIN = 0x20020000, LENGTH = 368K
}
