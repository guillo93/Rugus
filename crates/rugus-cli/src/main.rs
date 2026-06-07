//! `rugus-cli` — cliente de escritorio para dispositivos Rugus.
//!
//! Auto-detecta dispositivos por serie y BLE usando el protocolo `IDENTIFY`
//! (crate `rugus-proto`), conecta y los maneja desde una TUI (ratatui).

mod auth;
#[cfg(feature = "ble")]
mod ble;
#[cfg(not(feature = "ble"))]
#[path = "ble_stub.rs"]
mod ble;
mod detect;
mod device;
mod serial;
mod tui;

use std::io::{self, Write};

use anyhow::{anyhow, Result};

use crate::detect::Options;
use crate::device::{Candidate, Device, TransportKind};

fn main() -> Result<()> {
    let args = Args::parse(std::env::args().skip(1));

    if args.help {
        print_help();
        return Ok(());
    }

    // PSK para el auto-handshake: --psk <hex> o variable RUGUS_PSK. Decodificada
    // a bytes aquí; si el hex es inválido, abortamos antes de conectar.
    let psk = resolve_psk(&args)?;

    // Conexión directa a un puerto serie concreto.
    if let Some(port) = &args.serial_port {
        let cands = serial::detect_one(port);
        let candidate = cands
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("no se detectó un dispositivo Rugus en {port}"))?;
        return launch(candidate, args.list, psk);
    }

    let opts = Options {
        serial: !args.no_serial,
        // BLE solo si el binario se compiló con `--features ble`.
        ble: cfg!(feature = "ble") && !args.no_ble,
    };

    eprintln!(
        "Buscando dispositivos Rugus (serie{})…",
        if opts.ble { " + BLE" } else { "" }
    );
    let candidates = detect::discover(opts);

    if candidates.is_empty() {
        eprintln!("No se encontraron dispositivos Rugus.");
        eprintln!("Sugerencias: conecta el adaptador USB-TTL/BLE, comprueba permisos");
        eprintln!("(grupo `dialout`), y que el firmware responda a IDENTIFY.");
        return Ok(());
    }

    if args.list {
        print_devices(&candidates);
        return Ok(());
    }

    let candidate = if candidates.len() == 1 {
        candidates.into_iter().next().unwrap()
    } else {
        choose(candidates)?
    };

    launch(candidate, false, psk)
}

/// Resuelve la PSK del auto-handshake: prioriza `--psk`, cae a `RUGUS_PSK`.
/// Devuelve los bytes decodificados, o `None` si no se aportó ninguna.
fn resolve_psk(args: &Args) -> Result<Option<Vec<u8>>> {
    let hex = args.psk.clone().or_else(|| std::env::var("RUGUS_PSK").ok());
    match hex {
        Some(h) if !h.trim().is_empty() => rugus_proto::decode_hex(h.trim())
            .map(Some)
            .ok_or_else(|| anyhow!("PSK inválida: debe ser hex de longitud par")),
        _ => Ok(None),
    }
}

/// Conecta a un candidato y arranca la TUI (o solo lista si `list_only`).
fn launch(candidate: Candidate, list_only: bool, psk: Option<Vec<u8>>) -> Result<()> {
    if list_only {
        print_devices(std::slice::from_ref(&candidate));
        return Ok(());
    }
    let device = connect(candidate)?;
    tui::run(device, psk)
}

/// Abre la sesión viva según el transporte del candidato.
fn connect(candidate: Candidate) -> Result<Device> {
    match &candidate.kind {
        TransportKind::Serial(port) => serial::connect(port, candidate.signature.clone()),
        TransportKind::Ble(name) => {
            ble::connect(&candidate.addr, name.clone(), candidate.signature.clone())
        }
    }
}

/// Menú de selección cuando hay varios dispositivos.
fn choose(candidates: Vec<Candidate>) -> Result<Candidate> {
    eprintln!("\nVarios dispositivos Rugus detectados:");
    for (i, c) in candidates.iter().enumerate() {
        eprintln!("  [{}] {}", i + 1, c.menu_line());
    }
    eprint!("Elige [1-{}]: ", candidates.len());
    io::stderr().flush().ok();

    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let idx: usize = line
        .trim()
        .parse()
        .map_err(|_| anyhow!("selección inválida"))?;
    if idx == 0 || idx > candidates.len() {
        return Err(anyhow!("selección fuera de rango"));
    }
    Ok(candidates.into_iter().nth(idx - 1).unwrap())
}

fn print_devices(candidates: &[Candidate]) {
    println!("Dispositivos Rugus detectados ({}):", candidates.len());
    for c in candidates {
        println!("  - {}", c.menu_line());
    }
}

fn print_help() {
    let ble_state = if cfg!(feature = "ble") {
        "compilado: sí"
    } else {
        "compilado: no (recompila con --features ble)"
    };
    println!(
        "rugus — cliente de escritorio Rugus\n\n\
USO:\n  rugus [OPCIONES]\n\n\
OPCIONES:\n  \
--serial <PUERTO>   Conecta directo a un puerto serie (p. ej. /dev/ttyUSB0)\n  \
--no-ble            No escanear BLE\n  \
--no-serial         No sondear puertos serie\n  \
--psk <HEX>         PSK para auto-autenticar la sesión (o variable RUGUS_PSK)\n  \
--list              Detecta y lista dispositivos, luego sale\n  \
-h, --help          Muestra esta ayuda\n\n\
Auto-detección: enumera puertos serie (y BLE si está compilado), envía IDENTIFY\n\
y lista solo los dispositivos que responden una firma RUGUS válida. Si hay uno,\n\
conecta; si hay varios, ofrece un menú.\n\n\
Auto-handshake: con --psk/RUGUS_PSK el cliente ejecuta knock/prove al conectar y\n\
abre la sesión autenticada automáticamente (la PSK nunca viaja por el cable).\n\n\
Soporte BLE: {ble_state}."
    );
}

/// Argumentos de línea de comandos.
struct Args {
    help: bool,
    list: bool,
    no_ble: bool,
    no_serial: bool,
    serial_port: Option<String>,
    psk: Option<String>,
}

impl Args {
    fn parse<I: Iterator<Item = String>>(mut it: I) -> Args {
        let mut args = Args {
            help: false,
            list: false,
            no_ble: false,
            no_serial: false,
            serial_port: None,
            psk: None,
        };
        while let Some(a) = it.next() {
            match a.as_str() {
                "-h" | "--help" => args.help = true,
                "--list" => args.list = true,
                "--no-ble" => args.no_ble = true,
                "--no-serial" => args.no_serial = true,
                "--serial" => args.serial_port = it.next(),
                "--psk" => args.psk = it.next(),
                _ => {}
            }
        }
        args
    }
}
