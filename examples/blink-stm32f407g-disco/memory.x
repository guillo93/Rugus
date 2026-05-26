/* STM32F407VGT6 — memory map for the Rugus G3 blink example.
 *
 * Minimal map for cortex-m-rt (FLASH + RAM). CCM (64 KB @ 0x10000000) is not
 * used in this example.
 */

MEMORY
{
  FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 1024K
  RAM   (rwx) : ORIGIN = 0x20000000, LENGTH = 128K
}
