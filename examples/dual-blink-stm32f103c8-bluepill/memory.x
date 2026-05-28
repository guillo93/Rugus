/* STM32F103C8T6 — memory map for the Rugus lite dual-blink example. */

MEMORY
{
  FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 64K
  RAM   (rwx) : ORIGIN = 0x20000000, LENGTH = 20K
}
