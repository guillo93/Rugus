/* STM32F407VET6 — placa clon "Black F407VE" (FK407M3 y compatibles).
 *
 * Diferencia con la F407G-DISC1 (VGT6, 1 MiB flash): el VET6 tiene 512 KiB de
 * flash. La SRAM principal (SRAM1+SRAM2, 128 KiB) vive en 0x20000000; los 64 KiB
 * de CCM (0x10000000) no los usa cortex-m-rt y quedan sin mapear aquí.
 */

MEMORY
{
  FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 512K
  RAM   (rwx) : ORIGIN = 0x20000000, LENGTH = 128K
}
