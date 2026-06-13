/* STM32F407VGT6 — internal SRAM only (no external SDRAM on DISC1).
   FLASH limitada a 896K: el sector 11 (128K en 0x080E0000) queda reservado
   como ventana de secretos (PSK de la autenticación de canal, ver
   rugus_hal_stm32f4::flash). Ni código ni datos deben pisarlo. */

MEMORY
{
  FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 896K
  RAM   (rwx) : ORIGIN = 0x20000000, LENGTH = 128K
}
