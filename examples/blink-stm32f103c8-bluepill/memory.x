/* STM32F103C8T6 — memory map for the Rugus lite blink example.
 *
 * Minimal map for cortex-m-rt (FLASH + RAM).
 */

MEMORY
{
  FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 64K
  RAM   (rwx) : ORIGIN = 0x20000000, LENGTH = 20K
}
