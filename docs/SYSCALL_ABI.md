# Syscall ABI v0.1 (borrador)

Contrato user→kernel arch-independiente de Rugus. Esta versión es
**borrador**; estabilización en hito G2.

## Convención por arch

| Arch | Instrucción | Registros args | Retorno |
|------|-------------|----------------|---------|
| ARMv7-M / ARMv7E-M / ARMv8-M | `SVC #imm8` | r0-r3 | r0 (i32) |
| ARMv8-A | `SVC #0` | x0-x5 | x0 (i64) |
| RISC-V (RV32) | `ECALL` | a0-a5 | a0 (i32) |
| AVR | software int / call sentinel | r24:r25 | r24:r25 |

`rugus-core::syscall::Id` es el enum común; cada arch encodea el ID en su
convención.

## Tabla v0.1

| ID  | Nombre              | args                              | Retorno |
|-----|---------------------|-----------------------------------|---------|
| 0x00 | `yield_now`        | —                                 | 0 |
| 0x01 | `sleep_ms`         | (ms)                              | 0 |
| 0x02 | `task_id`          | —                                 | TaskId |
| 0x03 | `log`              | (level, ptr, len)                 | 0 |
| 0x10 | `ipc_send`         | (dst, msg_ptr, len)               | 0 / -EBUSY |
| 0x11 | `ipc_recv`         | (buf_ptr, cap, timeout_ms)        | bytes / -ETIMEDOUT |
| 0x30 | `net_socket`       | (kind)                            | handle |
| 0x31 | `net_connect`      | (sock, ip, port)                  | 0 / -EHOSTUNREACH |
| 0x32 | `net_send`         | (sock, ptr, len)                  | bytes |
| 0x33 | `net_recv`         | (sock, ptr, cap, timeout)         | bytes |
| 0x34 | `net_close`        | (sock)                            | 0 / -EINVAL |
| 0x50 | `fs_open`          | (key_id)                          | handle / -ENOSYS |
| 0x51 | `fs_read`          | (handle, pool_slot)               | bytes / -ENOENT |
| 0x52 | `fs_write`         | (handle, pool_slot, len)          | 0 / -EINVAL |
| 0x53 | `fs_close`         | (handle)                          | 0 / -EINVAL |
| 0x40 | `crypto_sign`      | (payload_ptr, len, sig_out_ptr)   | 0 / -EDENIED |
| 0x41 | `rng_fill`         | (buf_ptr, len)                    | 0 |
| 0xFE | `panic_app`        | (reason_code)                     | nunca |
| 0xFF | `extended`         | (id_extra en otro reg)            | depende |

## Errores

```rust
#[repr(i32)]
pub enum Errno {
    Einval       = -1,
    Ebusy        = -2,
    Etimedout    = -3,
    Ehostunreach = -4,
    Edenied      = -5,
    Eoverflow    = -6,
    Enomem       = -7,
    Efault       = -8,  // puntero rechazado por MPU/MMU
}
```

## Validación de punteros

Todo puntero pasado por una app se valida contra los permisos del dominio
caller antes de cualquier acceso:

1. Kernel pregunta al backend arch: "¿es `[ptr, ptr+len)` accesible con
   permisos X desde el dominio activo?"
2. Backend consulta su mecanismo HW (MPU regions, MMU page table, PMP
   entries).
3. Si falla: retorna `Errno::Efault`.
4. Acceso siempre por `core::ptr::copy_nonoverlapping` tras validación.

**Nunca** `unsafe { *user_ptr }` en un syscall handler.

## Versionado

- Pre-1.0 (G0-G2): breaking changes permitidos entre minor.
- 1.0 (post-G2): freeze. Cambios aditivos solo.
- Constante `rugus_core::syscall::ABI_VERSION` expuesta para que apps
  detecten incompatibilidad al arrancar.
