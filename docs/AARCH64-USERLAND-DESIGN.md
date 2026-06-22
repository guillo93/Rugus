# G6.2 — Integración de userland EL0 en el Scheduler AArch64 (diseño)

Estado: **propuesta de diseño** (sin implementar). Continúa G6.1
(`examples/rpi3-userland`, PR #126), que probó en HW los primitivos de
protección de memoria de ARMv8-A de forma aislada. Aquí se diseña su
integración en el `rugus_core::sched::Scheduler<CortexA>` para activar
`HAS_MEMORY_PROTECTION = true` en el backend `rugus-arch-cortex-a`.

## 1. El problema central

El backend AArch64 hoy conmuta contextos de forma **cooperativa** con
`cpu_switch`: un frame de 160 B (callee-saved x19–x30 + d8–d15) y reanudación
por `ret` (retorno de función). La preempción (G5/PR #122) reusa ese mismo
`cpu_switch` **anidado** dentro del frame de excepción del IRQ.

Una tarea **EL0** no encaja en ese modelo: solo se la puede reanudar con `eret`
(restaurando `SPSR_EL1`/`ELR_EL1`/`SP_EL0`), nunca con `ret`. Y como una tarea
EL0 jamás llama a `yield_now` (es código sin privilegio), **siempre** se la
guarda dentro de una excepción (su `SVC` o un IRQ del timer) y se la restaura con
`eret`.

Conclusión: el frame de 160 B/`ret` y el frame de excepción/`eret` son
**incompatibles** para un scheduler que mezcle tareas EL1 y EL0. En Cortex-M
esto no se nota porque **todo** switch pasa por PendSV (una excepción) y el
retorno de excepción unifica privilegiado/no-privilegiado "gratis". AArch64 no
tiene PendSV: hay que unificarlo a mano.

## 2. Decisión de diseño: frame de excepción **uniforme** + `eret` siempre

Adoptar **un único formato de frame** para todas las tareas (EL0 y EL1) y
reanudar **siempre** con `eret`. El frame guarda el estado completo:

```
offset  contenido
  0..248  x0..x30            (31 GPR)
  248     SP_EL0             (pila userland; irrelevante en tareas EL1)
  256     ELR_EL1            (PC de reanudación)
  264     SPSR_EL1           (EL destino + DAIF: EL1h=0x5 / EL0t=0x0)
  272..   (relleno a 16; NEON callee-saved d8–d15 si se decide salvarlos)
```

- **EL1 (kernel):** `SPSR=EL1h`, `ELR=` punto de reanudación, `SP_EL0` sin uso.
- **EL0 (userland):** `SPSR=EL0t`, `ELR=` PC userland, `SP_EL0=` pila de usuario.

Ventajas: un único camino `restore_and_eret`, soporta EL0/EL1, y `eret` de EL1h
a EL1h es válido (no exige cambio de privilegio). Coste: frame algo mayor y un
`eret` por switch (barato). Sustituye al `cpu_switch` de 160 B.

## 3. Cómo se construye el frame según el origen del switch

Hay tres orígenes de conmutación; todos terminan en el **mismo** epílogo
`restore_and_eret`:

1. **Cooperativo (hilo EL1, `yield_now`/`sleep_ms`):** la tarea EL1 cede
   llamando (vía `Arch::switch_context`) a una rutina que **sintetiza** un frame
   de excepción del propio hilo: guarda x19–x30 (los caller-saved están muertos
   en un límite de llamada AAPCS, sus huecos pueden quedar como estén),
   `ELR=lr`, `SPSR=EL1h`, `SP_EL0` irrelevante. Guarda el `SP` del frame en
   `prev->context`.
2. **Preempción (IRQ del timer):** el handler de IRQ ya apila el frame completo
   de la tarea interrumpida (EL0 o EL1). Ese frame **es** su contexto.
3. **Syscall (`SVC` desde EL0):** el handler de `SVC` apila el frame completo de
   la tarea EL0. Ese frame es su contexto.

`init_task_stack(privileged)` fabrica el frame inicial directamente con el
`SPSR`/`ELR`/`SP_EL0` correspondientes y x0–x30 = 0.

## 4. La fricción con el API de `rugus-core` (clave a resolver)

`rugus_core::sched` invoca `A::switch_context(prev, next)` desde:
- `yield_now`/`sleep_ms` → **contexto de hilo** (caso 1).
- `preempt_tick` → **dentro del handler de IRQ** (caso 2).

En el caso 1 hay que *construir* el frame de `prev`; en el caso 2 el frame de
`prev` **ya está apilado** por el handler. Una misma `switch_context` no puede
asumir ambos.

**Opción recomendada — unificar por trampa síncrona:**
- `Arch::switch_context` (caso cooperativo) ejecuta un `SVC #SWITCH`. El handler
  de `SVC` apila el frame del hilo que cedió, lo guarda en `prev->context`, carga
  `next->context` y hace `restore_and_eret`. Así el caso 1 también pasa por una
  excepción y comparte epílogo.
- En la preempción, el handler de IRQ ya tiene el frame; tras `preempt_tick`
  elegir la siguiente tarea, el **propio epílogo del handler** hace el switch
  (guardar `SP` en la saliente, cargar el de la entrante, `restore_and_eret`),
  **sin** volver a llamar a `switch_context`.

Esto exige un pequeño ajuste de cómo el backend AArch64 expone la preempción:
hoy `preempt_tick` llama a `switch_context`; en el modelo unificado, en AArch64
la elección (`pick_next`) y el cambio de `SP` los coordina el handler. Dos vías:
  (a) un hook arch-específico que `preempt_tick` use en AArch64 para *solo*
      seleccionar (sin conmutar), y el handler conmuta; o
  (b) `switch_context` detecta si se la llama desde excepción (leyendo si ya hay
      un frame en curso / un flag por-CPU) y evita re-apilar.

Recomendación: **(a)**, más explícita y sin estado oculto. Se puede modelar como
un `Arch::request_switch(prev, next)` que en Cortex-M pende PendSV y en AArch64
deja el par (prev,next) para que el epílogo de la excepción lo aplique. Es la
generalización honesta del "PendSV": *pedir* el switch, no ejecutarlo en línea.

> Nota: este punto (4) es el de mayor riesgo y el que conviene prototipar
> primero en un ejemplo standalone antes de tocar `rugus-core`.

## 5. ABI de syscall (SVC)

- Convención: número de syscall en `x8`, argumentos en `x0..x5`, retorno en `x0`
  (estilo Linux AArch64). `svc #0`.
- El handler de `SVC` (Lower EL AArch64 Sync, `ESR.EC==0x15`) enruta a la capa de
  syscall de `rugus-core` (`syscall::dispatch`/`lite`), validando punteros de
  usuario (que deben caer en la región EL0 de la tarea — reutilizar el helper de
  validación userland de F1.2).
- `yield`/`sleep` desde EL0 son syscalls que acaban en la lógica del scheduler.

## 6. Aislamiento entre múltiples tareas EL0 (`on_task_switch`)

G6.1 usó **una** región EL0 compartida (aísla kernel↔usuario, no usuario↔usuario).
Para varias tareas userland mutuamente aisladas:

- **MVP (un solo dominio de usuario):** todas las tareas EL0 comparten la región
  EL0; `HAS_MEMORY_PROTECTION=true` protege el kernel de userland. Suficiente para
  el primer hito integrado. `on_task_switch` no necesita reprogramar tablas.
- **Aislamiento real por tarea:** **TTBR0 por tarea** + `ASID` (para no vaciar el
  TLB entero en cada switch). `on_task_switch(mode, base, len)` escribe
  `TTBR0_EL1` con la tabla de la tarea entrante (`+ASID` en bits altos) y un
  `ISB`. El kernel mapea su propia ventana en `TTBR1_EL1` (direcciones altas) o
  comparte entradas globales (`nG=0`). Es el equivalente AArch64 de reprogramar
  la región MPU del stack en Cortex-M, pero a nivel de espacio de direcciones.

Recomendación: empezar por el MVP y dejar TTBR0-por-tarea como G6.3.

## 7. `init_task_stack(privileged)` — resumen

- Reservar el frame uniforme (§2) en el tope de la pila *kernel* de la tarea.
- x0–x30 = 0; `ELR=entry`.
- `privileged==true`  → `SPSR=EL1h (0x5)`, `SP_EL0`=irrelevante.
- `privileged==false` → `SPSR=EL0t (0x0)`, `SP_EL0`= tope de la pila *usuario* de
  la tarea (en la región EL0 mapeada `AP=01`, `UXN=0`).
- `Context` pasa a guardar el `SP` del frame (igual que hoy) — el formato del
  frame es lo que cambia.

## 8. Plan de validación (incremental, en HW por mini-UART)

1. **Prototipo standalone** del epílogo unificado + `request_switch` (sin
   `rugus-core`): 1 tarea EL1 supervisora + 1 tarea EL0 que hace syscalls
   (`putchar`, `yield`) y un acceso indebido contenido. Valida §2/§4/§5.
2. **Integración en `rugus-arch-cortex-a`:** `HAS_MEMORY_PROTECTION=true`, nuevo
   frame, `init_task_stack` EL0/EL1, epílogo unificado, handler de `SVC`.
3. **Ejemplo `rpi3-userland-sched`:** `Scheduler<CortexA>` con una tarea EL0 real
   planificada junto a una EL1; cooperativo primero, luego con el timer
   (preempción de EL0 incluida). `coil` muestra la tarea userland.
4. **Multitarea EL0 aislada (G6.3):** TTBR0+ASID por tarea.

## 9. Riesgos

- **§4 (invocación del switch):** el mayor. Prototipar aislado primero.
- **Coherencia de TLB/cache** al cambiar TTBR0 o mapear código userland
  (`IC IALLU`/`DSB`/`ISB`, como en G6.1).
- **Validación de punteros de syscall** desde EL0 (no confiar en `x0..x5`).
- **NEON caller-saved** en el frame si las tareas usan FP intensivo (hoy se
  salvan solo d8–d15 callee-saved vía el switch).
- **Regresión en STM32:** ninguna esperada (cambios acotados al crate AArch64),
  pero el cambio de formato de frame del backend AArch64 afecta a
  `rpi3-kernel`/`rpi3-preempt`/`rpi3-console` → revalidar los tres en HW.

## 10. Resumen de la decisión

Unificar **un solo frame de excepción** y reanudar siempre con `eret`;
generalizar el "pend switch" del PendSV a un `request_switch` arch-agnóstico que
en AArch64 aplica el epílogo en el retorno de excepción. Empezar por un dominio
de usuario único (MVP) y prototipar el §4 aislado antes de tocar `rugus-core`.
