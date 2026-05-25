# Security Policy

## Versiones soportadas

Rugus está en **génesis (hito G0)**. Hasta que se publique `1.0`, solo la
rama `main` recibe parches de seguridad. Versiones pre-1.0 son frágiles
por definición — si despliegas algo basado en Rugus en producción antes
de `1.0`, asume que el riesgo es tuyo.

| Versión | Soporte de seguridad |
|---------|----------------------|
| `main`  | ✅ activa |
| `< 1.0` (releases puntuales) | ❌ ninguna (snapshot histórico) |
| `1.x` (futuro) | ✅ activa durante el ciclo `1.x` |

## Reportar una vulnerabilidad

**No** abras un issue público para vulnerabilidades. Reporta de forma
privada vía uno de estos canales, en orden de preferencia:

1. **GitHub Security Advisories** —
   <https://github.com/guillo93/Rugus/security/advisories/new>. Es el
   canal canónico; permite triage + fix + CVE coordinado.
2. **Email** — `luiguihez93@gmail.com` con asunto `[Rugus security]`.

Incluye en el reporte:

- Versión / commit hash afectado.
- Arch y chip donde se observó (si aplica): `cortex-m`, `stm32f7`, etc.
- Descripción del impacto (escalada de privilegio, ejecución arbitraria,
  DoS, fuga de información, bypass de MPU, etc.).
- Pasos mínimos para reproducir, idealmente con PoC.
- Tu propuesta de mitigación si la tienes.

## Qué esperar

- **Acuse de recibo** en ≤ 7 días.
- **Triage inicial** en ≤ 14 días (severidad, ámbito, plan).
- **Fix coordinado** según severidad: crítica (≤ 30 días), alta
  (≤ 60 días), media (≤ 90 días), baja (próxima release ordinaria).
- **Disclosure pública** tras el fix, con crédito al reporter (salvo que
  prefieras anonimato).
- **CVE** solicitado cuando aplique vía GitHub Security Advisories.

## Ámbito

**En ámbito** (reportar):

- Bugs de seguridad de memoria en `unsafe` blocks (use-after-free,
  buffer overflow, data races no captadas por el borrow checker).
- Bypasses del modelo MPU descrito en `docs/SECURITY_MODEL.md`.
- Vulnerabilidades en el syscall ABI (`docs/SYSCALL_ABI.md`) que
  permitan acceso no autorizado desde una app.
- Bugs en el path de boot verificado o OTA que permitan ejecución de
  firmware no firmado (post hito G6).
- Crypto rota o mal usada en `rugus-crypto` (post G4).

**Fuera de ámbito** (no son vulnerabilidades de Rugus):

- Bugs en código de aplicación que use Rugus incorrectamente.
- Ataques de canal lateral (DPA, EM) sobre el hardware. Rugus no
  pretende ser HSM ni resistir atacantes con acceso físico
  prolongado.
- Vulnerabilidades en dependencias upstream (`stm32f7` PAC, `smoltcp`,
  `embedded-tls`). Reportar al proyecto upstream correspondiente; aquí
  podemos publicar advisory si Rugus las amplifica.
- Bugs que solo se manifiestan con `#![deny(unsafe_code)]` desactivado
  en crates donde la política lo prohíbe.

## Política frente a investigadores

Welcome. Reportes responsables son siempre acogidos. No habrá acciones
legales contra investigadores que actúen de buena fe siguiendo esta
política.
