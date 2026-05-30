//! Stub del transporte BLE para builds sin la feature `ble`.
//!
//! El soporte BLE real (`btleplug`) arrastra `libdbus-sys`, que requiere
//! `libdbus-1-dev` / `dbus-devel` en Linux. El build por defecto es solo serie y
//! no debe necesitar esa dependencia de sistema. Recompila con `--features ble`
//! para habilitar el escaneo/sesión BLE de `ble.rs`.

use anyhow::{bail, Result};

use rugus_proto::Signature;

use crate::device::{Candidate, Device};

/// Sin soporte BLE compilado: no hay candidatos.
pub fn detect() -> Vec<Candidate> {
    Vec::new()
}

/// Sin soporte BLE compilado: error explicativo.
pub fn connect(_addr: &str, _name: String, _signature: Signature) -> Result<Device> {
    bail!("compilado sin soporte BLE; recompila con `--features ble` (requiere libdbus-1-dev/dbus-devel)")
}
