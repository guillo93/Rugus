/* STM32F407VET6 (clon FK407M3-VET6) — SRAM interna (sin SDRAM externa).
   FLASH limitada a 384K: el sector 7 (128K en 0x08060000) queda reservado como
   ventana de secretos (PSK de la autenticación de canal, ver
   rugus_hal_stm32f4::flash::FlashWindow::new_ve512k). Ni código ni datos deben
   pisarlo. (El VET6 tiene 512K: sectores 0-7; el último es el 7, no el 11.) */

MEMORY
{
  FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 384K
  RAM   (rwx) : ORIGIN = 0x20000000, LENGTH = 128K
}
