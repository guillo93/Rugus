# `blink-stm32f769-disco`

Primer ejemplo Rugus en hardware real. Parpadea LD1 (rojo, PJ13) de la
[STM32F769I-DISCO](https://www.st.com/en/evaluation-tools/32f769idiscovery.html)
y emite logs `defmt` por SWD/RTT.

## Pre-requisitos

```powershell
rustup target add thumbv7em-none-eabihf
cargo install probe-rs-tools --locked
```

ST-LINK on-board conectado por USB.

## Ejecución

Desde la raíz del workspace o desde este directorio:

```powershell
cd examples\blink-stm32f769-disco
cargo run
```

`probe-rs` flashea, arranca el chip, y muestra logs RTT en la consola:

```
INFO  rugus blink @ STM32F769I-DISCO, HSI 16 MHz default
INFO  LD1 (PJ13) configured; toggling at ~1 Hz
```

Si no se ve nada, comprobar:

1. Cable USB ST-LINK conectado al puerto correcto (CN15 en la DISCO, no
   el USB HS user).
2. `probe-rs list` muestra la placa.
3. Permisos de USB (en Linux puede requerir udev rule; en Windows el
   driver lo instala ST-LINK utility).

## Qué demuestra

- Workspace Rugus compila contra `thumbv7em-none-eabihf` con FPU.
- `rugus-runtime` provee panic + RTT + entry sin tocar nada.
- `rugus-hal-stm32f7::gpio::LedPin` implementa el trait
  `rugus_hal::GpioPin`.
- Logs `defmt` con timestamps DWT funcionan.

## Qué **no** demuestra todavía (futuro)

- Clocks @ 216 MHz (HSI default es 16 MHz).
- Scheduler (es un único `loop {}`).
- MPU / dominios.
- SDRAM, LTDC, red, crypto.

Ver `docs/ROADMAP.md` para qué llega en cada hito.
