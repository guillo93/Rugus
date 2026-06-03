//! Telemetría de faults persistente y safe-mode (LOG-FREE).
//!
//! ## Qué resuelve
//!
//! El fault containment (kill+respawn) reacciona a cada fault de forma aislada,
//! pero no recuerda nada: tras un reset el kernel arranca sin memoria de qué
//! tareas fallaron, cuántas veces, ni si el último arranque acabó en tormenta de
//! faults. Esta estructura es el **libro de cuentas post-mortem**: contadores por
//! tarea, total acumulado, conteo de arranques y el último [`FaultReport`].
//!
//! ## Persistencia entre resets
//!
//! Pensada para vivir en RAM **no inicializada** (sección `.uninit` de
//! `cortex-m-rt`): el runtime NO la pone a cero al arrancar, así que su contenido
//! sobrevive a un reset por watchdog o por fault. El problema es distinguir un
//! arranque en frío (RAM con basura) de uno en caliente (datos válidos de la
//! ejecución previa). Se resuelve con un campo [`FaultTelemetry::magic`]: en frío
//! la basura casi nunca coincide con [`MAGIC`], así que [`FaultTelemetry::boot`]
//! reinicia los contadores y sella el magic; en caliente el magic coincide y solo
//! incrementa el conteo de arranques, preservando el historial.
//!
//! ## Safe-mode
//!
//! Si el total de faults supera [`SAFE_MODE_FAULT_THRESHOLD`], o una sola tarea
//! supera [`SAFE_MODE_TASK_THRESHOLD`], el sistema entra en *safe-mode*: el
//! supervisor de la placa puede consultarlo ([`FaultTelemetry::safe_mode`]) para
//! dejar de respawnear y degradarse de forma controlada en lugar de entrar en un
//! bucle de crash/respawn indefinido.
//!
//! Frontera de capas: este módulo es LOG-FREE y agnóstico al hardware; la
//! ubicación en `.uninit` y el logging los aporta `rugus-kernel`.

use crate::fault::FaultReport;
use crate::sched::MAX_TASKS;

/// Sello de validez. Con este valor presente, los datos en `.uninit` se
/// consideran de un arranque previo (reset en caliente). En frío la RAM sin
/// inicializar prácticamente nunca contiene este patrón. ASCII "RGST".
pub const MAGIC: u32 = 0x5247_5354;

/// Umbral de faults totales que dispara safe-mode.
pub const SAFE_MODE_FAULT_THRESHOLD: u32 = 16;

/// Umbral de faults de una sola tarea que dispara safe-mode (una tarea que
/// recae una y otra vez es señal de fallo determinista, no de glitch).
pub const SAFE_MODE_TASK_THRESHOLD: u32 = 5;

/// Libro de cuentas de faults persistente entre resets.
///
/// Diseñada para residir en `.uninit`: no implementa `Default` ni se inicializa
/// implícitamente. Use [`FaultTelemetry::boot`] al arrancar para validar el
/// magic y decidir frío vs caliente.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct FaultTelemetry {
    /// Sello de validez ([`MAGIC`] si los datos son de un arranque previo).
    pub magic: u32,
    /// Número de arranques observados (incluye el actual).
    pub boot_count: u32,
    /// Faults totales acumulados a lo largo de todos los arranques.
    pub total_faults: u32,
    /// Faults por tarea, indexado por `TaskId.0`.
    pub per_task: [u32; MAX_TASKS],
    /// Marca de validez del último reporte (`per_task` no basta porque el último
    /// reporte guarda detalle, no solo el conteo).
    pub has_last: bool,
    /// Tipo del último fault contenido (`u8`, [`crate::FaultKind`] codificado).
    pub last_kind: u8,
    /// `task_id` del último fault contenido.
    pub last_task: u8,
    /// Program counter del último fault contenido.
    pub last_pc: u32,
    /// Dirección culpable del último fault (0 si no la hubo).
    pub last_addr: u32,
}

impl FaultTelemetry {
    /// Resultado de validar el magic al arrancar.
    ///
    /// Devuelve `true` si fue un **reset en caliente** (datos previos válidos
    /// preservados), `false` si fue un **arranque en frío** (datos
    /// reinicializados). En ambos casos, al volver la estructura está sellada
    /// con [`MAGIC`] y `boot_count` ya incrementado para este arranque.
    ///
    /// # Safety
    ///
    /// `self` puede apuntar a RAM sin inicializar (`.uninit`). Esta función NO
    /// lee ningún campo cuyo valor dependa de inicialización previa salvo
    /// `magic`, que es un `u32` (cualquier patrón de bits es válido de leer).
    /// Por eso es seguro llamarla sobre memoria con basura.
    pub fn boot(&mut self) -> bool {
        if self.magic == MAGIC {
            // Reset en caliente: preservamos el historial, contamos el arranque.
            self.boot_count = self.boot_count.wrapping_add(1);
            true
        } else {
            // Arranque en frío: la basura no coincide con el magic. Reiniciamos.
            *self = Self::fresh();
            false
        }
    }

    /// Estado limpio sellado: contadores a cero, un arranque contabilizado.
    const fn fresh() -> Self {
        Self {
            magic: MAGIC,
            boot_count: 1,
            total_faults: 0,
            per_task: [0; MAX_TASKS],
            has_last: false,
            last_kind: 0,
            last_task: 0,
            last_pc: 0,
            last_addr: 0,
        }
    }

    /// Registra un fault contenido: incrementa total, contador de la tarea y
    /// guarda el último reporte. Idempotente respecto a la validez del magic
    /// (asume `boot` ya ejecutado).
    pub fn record(&mut self, report: &FaultReport) {
        self.total_faults = self.total_faults.saturating_add(1);
        let idx = report.task_id.0 as usize;
        if idx < MAX_TASKS {
            self.per_task[idx] = self.per_task[idx].saturating_add(1);
        }
        self.has_last = true;
        self.last_kind = report.kind as u8;
        self.last_task = report.task_id.0;
        self.last_pc = report.pc;
        self.last_addr = report.addr.unwrap_or(0);
    }

    /// `true` si el sistema debe entrar en safe-mode: o bien el total de faults
    /// supera [`SAFE_MODE_FAULT_THRESHOLD`], o bien una sola tarea supera
    /// [`SAFE_MODE_TASK_THRESHOLD`].
    pub fn safe_mode(&self) -> bool {
        if self.total_faults >= SAFE_MODE_FAULT_THRESHOLD {
            return true;
        }
        self.per_task.iter().any(|&c| c >= SAFE_MODE_TASK_THRESHOLD)
    }

    /// Faults contabilizados para una tarea concreta (0 si índice fuera de rango).
    pub fn faults_for(&self, task: usize) -> u32 {
        self.per_task.get(task).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Domain;
    use crate::fault::{FaultKind, FaultReport};
    use crate::sched::TaskId;

    fn report(task: u8, kind: FaultKind) -> FaultReport {
        FaultReport {
            kind,
            pc: 0x0800_1234,
            domain: Domain::App,
            task_id: TaskId(task),
            addr: Some(0xDEAD_BEEF),
        }
    }

    #[test]
    fn cold_boot_zeroes_and_seals() {
        // Simula RAM con basura: magic no coincide.
        let mut t = FaultTelemetry {
            magic: 0x1234_5678,
            boot_count: 999,
            total_faults: 999,
            per_task: [42; MAX_TASKS],
            has_last: true,
            last_kind: 9,
            last_task: 9,
            last_pc: 9,
            last_addr: 9,
        };
        let warm = t.boot();
        assert!(!warm, "magic distinto => arranque en frío");
        assert_eq!(t.magic, MAGIC);
        assert_eq!(t.boot_count, 1);
        assert_eq!(t.total_faults, 0);
        assert_eq!(t.per_task, [0; MAX_TASKS]);
        assert!(!t.has_last);
    }

    #[test]
    fn warm_boot_preserves_and_counts() {
        let mut t = FaultTelemetry {
            magic: MAGIC,
            boot_count: 3,
            total_faults: 7,
            per_task: [1, 2, 3, 1],
            has_last: true,
            last_kind: 1,
            last_task: 2,
            last_pc: 0x0800_0000,
            last_addr: 0,
        };
        let warm = t.boot();
        assert!(warm, "magic coincide => reset en caliente");
        assert_eq!(t.boot_count, 4, "cuenta el arranque actual");
        assert_eq!(t.total_faults, 7, "preserva historial");
        assert_eq!(t.per_task, [1, 2, 3, 1]);
    }

    #[test]
    fn record_increments_total_and_per_task() {
        let mut t = FaultTelemetry {
            magic: 0,
            ..FaultTelemetry::fresh()
        };
        t.boot();
        t.record(&report(2, FaultKind::MemManage));
        t.record(&report(2, FaultKind::UsageFault));
        assert_eq!(t.total_faults, 2);
        assert_eq!(t.faults_for(2), 2);
        assert_eq!(t.faults_for(0), 0);
        assert!(t.has_last);
        assert_eq!(t.last_task, 2);
        assert_eq!(t.last_kind, FaultKind::UsageFault as u8);
    }

    #[test]
    fn safe_mode_by_total_threshold() {
        let mut t = FaultTelemetry::fresh();
        for _ in 0..SAFE_MODE_FAULT_THRESHOLD {
            // Reparte entre tareas para no disparar el umbral por-tarea antes.
            t.record(&report(
                (t.total_faults % MAX_TASKS as u32) as u8,
                FaultKind::BusFault,
            ));
        }
        assert!(t.safe_mode());
    }

    #[test]
    fn safe_mode_by_single_task_threshold() {
        let mut t = FaultTelemetry::fresh();
        for _ in 0..SAFE_MODE_TASK_THRESHOLD {
            t.record(&report(1, FaultKind::HardFault));
        }
        assert!(t.safe_mode(), "una tarea reincidente dispara safe-mode");
        assert!(t.total_faults < SAFE_MODE_FAULT_THRESHOLD);
    }

    #[test]
    fn not_in_safe_mode_under_thresholds() {
        let mut t = FaultTelemetry::fresh();
        t.record(&report(0, FaultKind::BusFault));
        t.record(&report(1, FaultKind::BusFault));
        assert!(!t.safe_mode());
    }
}
