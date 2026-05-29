//! Transporte serie: enumeración, sondeo IDENTIFY y sesión por hilos.

use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use rugus_proto::identify::IDENTIFY_REQUEST;
use rugus_proto::{parse_signature, LineAssembler, Signature};

use crate::device::{Candidate, Device, TransportKind};

/// Baud rate de la consola Rugus (USART1 / puente de módulos).
pub const BAUD: u32 = 115_200;

/// Tiempo de lectura al sondear cada puerto en busca de la firma IDENTIFY.
const PROBE_WINDOW: Duration = Duration::from_millis(700);

/// Enumera puertos serie, envía IDENTIFY a cada uno y devuelve los que
/// responden con una firma Rugus válida.
pub fn detect() -> Vec<Candidate> {
    let ports = match serialport::available_ports() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };

    let mut found = Vec::new();
    for info in ports {
        if let Ok(sig) = probe_port(&info.port_name) {
            found.push(Candidate {
                kind: TransportKind::Serial(info.port_name.clone()),
                addr: info.port_name,
                signature: sig,
            });
        }
    }
    found
}

/// Sondea un único puerto serie por nombre; devuelve un candidato si responde.
pub fn detect_one(port_name: &str) -> Vec<Candidate> {
    match probe_port(port_name) {
        Ok(sig) => vec![Candidate {
            kind: TransportKind::Serial(port_name.to_string()),
            addr: port_name.to_string(),
            signature: sig,
        }],
        Err(_) => Vec::new(),
    }
}

/// Abre un puerto, envía IDENTIFY y trata de parsear la firma.
fn probe_port(port_name: &str) -> Result<Signature> {
    let mut port = serialport::new(port_name, BAUD)
        .timeout(Duration::from_millis(120))
        .open()
        .with_context(|| format!("abrir {port_name}"))?;

    port.write_all(IDENTIFY_REQUEST.as_bytes())?;
    port.flush().ok();

    let mut asm = LineAssembler::new();
    let mut buf = [0u8; 256];
    let deadline = Instant::now() + PROBE_WINDOW;
    while Instant::now() < deadline {
        match port.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => {
                for line in asm.push(&buf[..n]) {
                    if let Ok(sig) = parse_signature(&line) {
                        return Ok(sig);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("sin firma Rugus en {port_name}")
}

/// Abre una sesión serie viva: hilo lector → `bytes_rx`, hilo escritor ← `cmd_tx`.
pub fn connect(port_name: &str, signature: Signature) -> Result<Device> {
    let port = serialport::new(port_name, BAUD)
        .timeout(Duration::from_millis(100))
        .open()
        .with_context(|| format!("abrir {port_name}"))?;
    let mut reader = port;
    let mut writer = reader.try_clone().context("clonar puerto serie")?;

    let (bytes_tx, bytes_rx) = mpsc::channel::<Vec<u8>>();
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

    // Hilo lector: vuelca bytes recibidos al canal de la TUI.
    thread::spawn(move || {
        let mut buf = [0u8; 256];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {}
                Ok(n) => {
                    if bytes_tx.send(buf[..n].to_vec()).is_err() {
                        break; // TUI cerró el canal.
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => break, // Puerto desconectado.
            }
        }
    });

    // Hilo escritor: consume comandos y los transmite.
    thread::spawn(move || {
        while let Some(data) = cmd_rx.blocking_recv() {
            if writer.write_all(&data).is_err() {
                break;
            }
            let _ = writer.flush();
        }
    });

    Ok(Device {
        kind: TransportKind::Serial(port_name.to_string()),
        signature,
        bytes_rx,
        cmd_tx,
    })
}
