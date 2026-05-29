//! Handle unificado de dispositivo, agnóstico del transporte.
//!
//! Tanto el transporte serie (síncrono, hilos) como BLE (asíncrono, tokio en un
//! hilo dedicado) exponen el mismo par de canales:
//!
//! - `bytes_rx`: bytes crudos recibidos del dispositivo (`std::sync::mpsc`),
//!   leídos por la TUI con `try_recv` sin bloquear.
//! - `cmd_tx`: comandos a transmitir, ya serializados con `\r\n`
//!   (`tokio::sync::mpsc::unbounded`), enviados por la TUI sin bloquear.

use std::sync::mpsc::Receiver as StdReceiver;
use tokio::sync::mpsc::UnboundedSender;

use rugus_proto::Signature;

/// Tipo de transporte de un dispositivo detectado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportKind {
    /// Puerto serie (nombre del puerto).
    Serial(String),
    /// BLE (nombre anunciado o id del periférico).
    Ble(String),
}

impl TransportKind {
    /// Etiqueta legible para menús.
    pub fn label(&self) -> String {
        match self {
            TransportKind::Serial(p) => format!("serie  {p}"),
            TransportKind::Ble(n) => format!("BLE    {n}"),
        }
    }
}

/// Candidato detectado durante el auto-descubrimiento (aún sin sesión viva).
#[derive(Clone, Debug)]
pub struct Candidate {
    /// Transporte y dirección legible.
    pub kind: TransportKind,
    /// Clave de reconexión: nombre de puerto (serie) o id de periférico (BLE).
    pub addr: String,
    /// Firma IDENTIFY validada del dispositivo.
    pub signature: Signature,
}

impl Candidate {
    /// Etiqueta de una línea para el menú de selección.
    pub fn menu_line(&self) -> String {
        format!("{}  —  {}", self.kind.label(), self.signature.label())
    }
}

/// Sesión viva con un dispositivo: canales de E/S + metadatos.
pub struct Device {
    /// Transporte y dirección.
    pub kind: TransportKind,
    /// Firma IDENTIFY del dispositivo.
    pub signature: Signature,
    /// Bytes recibidos del dispositivo.
    pub bytes_rx: StdReceiver<Vec<u8>>,
    /// Comandos a transmitir (serializados con `\r\n`).
    pub cmd_tx: UnboundedSender<Vec<u8>>,
}

impl Device {
    /// Envía un comando ya serializado al cable. Devuelve `false` si el canal
    /// está cerrado (transporte caído).
    pub fn send(&self, wire: Vec<u8>) -> bool {
        self.cmd_tx.send(wire).is_ok()
    }
}
