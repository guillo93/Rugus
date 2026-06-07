/* STM32F103C8T6 — Rugus lite appliance. */

/* La última página de 1K (0x0800FC00) se reserva para la PSK de autenticación
 * de canal (challenge-response HMAC). Por eso FLASH es 63K, no 64K: el linker
 * no debe colocar código/.rodata sobre el almacén de secreto. Ver `psk.rs`. */
MEMORY
{
  FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 63K
  RAM   (rwx) : ORIGIN = 0x20000000, LENGTH = 20K
}
