//! Dominios de memoria Rugus — ver `docs/SECURITY_MODEL.md`.

/// Dominio lógico mapeado a regiones MPU por el backend arch.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    /// Kernel `.text` / `.data` — solo privilegiado.
    Kernel = 0,
    /// Periféricos y drivers HAL — solo privilegiado.
    Drivers = 1,
    /// Servicios userland (arena + `.text` del servicio).
    Services = 2,
    /// App userland activa (arena remapeada en context switch).
    App = 3,
}

impl Domain {
    /// Nombre corto para logs RTT.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Kernel => "Kernel",
            Self::Drivers => "Drivers",
            Self::Services => "Services",
            Self::App => "App",
        }
    }
}
