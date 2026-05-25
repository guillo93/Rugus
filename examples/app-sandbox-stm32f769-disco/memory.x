/* STM32F769NIH6 — mapa con SDRAM para heap G1/G2. */

MEMORY
{
  FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 2048K
  RAM   (rwx) : ORIGIN = 0x20020000, LENGTH = 368K
  SDRAM (rw)  : ORIGIN = 0xC0000000, LENGTH = 16M
}

_heap_sdram_start = ORIGIN(SDRAM);
_heap_sdram_size  = 256K;
