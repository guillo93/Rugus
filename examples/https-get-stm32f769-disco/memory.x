/* STM32F769NIH6 — SDRAM heap + ETH DMA (16 KiB @ 0x20078000, MPU-aligned). */

MEMORY
{
  FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 2048K
  RAM   (rwx) : ORIGIN = 0x20020000, LENGTH = 352K
  ETH   (rw)  : ORIGIN = 0x20078000, LENGTH = 16K
  SDRAM (rw)  : ORIGIN = 0xC0000000, LENGTH = 16M
}

SECTIONS
{
  .eth_dma (NOLOAD) : ALIGN(32) {
    KEEP(*(.eth_dma))
    KEEP(*(.eth_dma.*))
    . = ALIGN(32);
  } > ETH
}

_heap_sdram_start = ORIGIN(SDRAM);
_heap_sdram_size  = 512K;
