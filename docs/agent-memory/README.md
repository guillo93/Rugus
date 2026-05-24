# Agent Memory

Memoria persistente del proyecto Rugus destinada a **agentes IA** (Claude,
Codex, Copilot, otros) que asistan en este repo en sesiones futuras. Esta
carpeta se versiona en el repo a propósito: cualquier agente que clone
debe empezar leyendo aquí antes de proponer cambios.

## Orden de lectura al empezar sesión

1. **`project.md`** — visión, decisiones bloqueadas, arquitecturas/chips
   planificados.
2. **`preferences.md`** — preferencias del owner formuladas como reglas.
3. **Entrada más reciente de `../../AGENT_LOG.md`** — qué hizo el agente
   anterior.
4. **`../ARCHITECTURE.md` y `../ROADMAP.md`**.
5. Si tocas porting: **`../PORTING.md`**.
6. Si tocas el ABI o el modelo de seguridad: **`../SYSCALL_ABI.md`**,
   **`../SECURITY_MODEL.md`**, **`../INVARIANTS.md`**.

## Al cerrar sesión

- Añade entrada en `../../AGENT_LOG.md` (modelo, fecha, scope, decisiones,
  estado, próximo paso).
- Si descubres una **preferencia nueva** del owner (corrección o
  confirmación de algo no obvio), añade entrada `P-N` a `preferences.md`
  — no edites las existentes salvo refinamiento.
- Si el proyecto pivota (cambio de objetivo, nueva arch añadida fuera de
  plan), actualiza `project.md`.

## Política frente a memoria privada del agente

El agente Claude (u otros) puede tener memoria persistente en su entorno
local (e.g. `~/.claude/projects/...`). **En conflicto entre esa memoria
privada y los archivos de esta carpeta, gana esta carpeta** — es la fuente
de verdad del proyecto.

## Formato

Markdown plano, sin frontmatter. Conciso y directo. Toda preferencia o
decisión debe ir con `**Why:**` (la razón) y `**How to apply:**` (cuándo
y cómo aplicar). Sin razón documentada, la regla se vuelve dogma ciego.
