# Rugus lite — inferencia ML (fase 6, stub)

Documento de **planificación** para ML embebido en el appliance F103.
No hay runtime ML en firmware aún; esta fase fija el marco de trabajo.

## Objetivo

Permitir modelos **tiny** (quantized int8) para sensores locales — clasificación
simple, umbrales aprendidos, anomaly flags — sin cargar el TCB del kernel.

## Principios (kernel serio)

1. **Inferencia fuera del kernel** — tarea userland o servicio post-`hatch`.
2. **Modelo en SD** — archivo `.rfn` referencia + blob `.afr`/`.mlr` (formato TBD).
3. **Sin alloc en IRQ** — inferencia solo en contexto de tarea cooperativa.
4. **Anchor** — `anchor` detiene inferencia y GPIO no seguros.

## Límites F103C8

| Recurso | Límite | Implicación |
|---------|--------|-------------|
| Flash | 64 KiB | Modelos < 16 KiB recomendados |
| SRAM | 20 KiB | Tensor arena 2–4 KiB máximo |
| FPU | No | Solo int8 / fixed-point |
| MPU | No | Validación software en servicio |

## Roadmap propuesto

| Hito | Entregable |
|------|------------|
| ML-0 | Este documento + hook `inference_run` en syscall lite (stub) |
| ML-1 | Integrar `llm/embedded` o `micromlgen` export en `.afr` |
| ML-2 | Comando CLI `prism` (nombre reservado) → ejecutar modelo |
| ML-3 | Verify HW con sensor I2C real + modelo demo |

## Comando CLI reservado (futuro)

| Comando | Operación | Estado |
|---------|-----------|--------|
| `prism` | `inference_run` | no implementado |

## Referencias

- [`RUGUS-LITE-APPLIANCE.md`](RUGUS-LITE-APPLIANCE.md)
- [`RUGUS-KERNEL-VISION.md`](RUGUS-KERNEL-VISION.md)
