//! Transporte de red (LAN/WiFi): descubrimiento por UDP y sesión por TCP.
//!
//! El descubrimiento reutiliza el handshake `IDENTIFY` sobre UDP: el host emite
//! la solicitud por broadcast al puerto [`DISCOVERY_PORT`] y los dispositivos
//! Rugus con pila de red responden su firma al emisor. La firma de red lleva el
//! campo extra `tcp=<puerto>`, que indica dónde abrir la sesión de consola.
//!
//! La sesión viva es un `TcpStream` partido en dos: un hilo lector vuelca los
//! bytes recibidos a `bytes_rx` y un hilo escritor consume `cmd_tx` y los
//! transmite — el mismo contrato de canales que el transporte serie, de modo
//! que la TUI y el auto-handshake funcionan sin cambios.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use rugus_proto::identify::IDENTIFY_REQUEST;
use rugus_proto::{parse_signature, LineAssembler, Signature, DISCOVERY_PORT};

use crate::device::{Candidate, Device, TransportKind};

/// Ventana de escucha de respuestas al broadcast de descubrimiento.
const DISCOVERY_WINDOW: Duration = Duration::from_millis(800);
/// Timeout de conexión/lectura TCP al sondear una dirección concreta.
const PROBE_TIMEOUT: Duration = Duration::from_millis(700);

/// Descubre dispositivos Rugus en la red local emitiendo `IDENTIFY` por
/// broadcast UDP y recogiendo las firmas que respondan en una ventana corta.
pub fn detect() -> Vec<Candidate> {
    discover_broadcast().unwrap_or_default()
}

/// Sondea una dirección `ip:puerto` concreta abriendo TCP y enviando
/// `IDENTIFY`; devuelve un candidato si responde una firma válida.
pub fn detect_one(target: &str) -> Vec<Candidate> {
    match probe_tcp(target) {
        Ok((addr, sig)) => vec![Candidate {
            kind: TransportKind::Net(addr.to_string()),
            addr: addr.to_string(),
            signature: sig,
        }],
        Err(_) => Vec::new(),
    }
}

/// Emite el broadcast de descubrimiento y recolecta candidatos únicos.
fn discover_broadcast() -> Result<Vec<Candidate>> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).context("bind UDP de descubrimiento")?;
    sock.set_broadcast(true).context("habilitar broadcast")?;
    sock.set_read_timeout(Some(Duration::from_millis(120))).ok();

    let dest = SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT));
    sock.send_to(IDENTIFY_REQUEST.as_bytes(), dest)
        .context("enviar IDENTIFY por broadcast")?;

    let mut found: Vec<Candidate> = Vec::new();
    let mut buf = [0u8; 512];
    let deadline = Instant::now() + DISCOVERY_WINDOW;
    while Instant::now() < deadline {
        match sock.recv_from(&mut buf) {
            Ok((n, src)) => {
                if let Some(cand) = candidate_from_reply(&buf[..n], src) {
                    // Dedup por dirección de consola.
                    if !found.iter().any(|c| c.addr == cand.addr) {
                        found.push(cand);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
    Ok(found)
}

/// Construye un candidato a partir de una respuesta UDP: parsea la firma y
/// deriva la dirección de la consola TCP de la IP del emisor + campo `tcp`.
fn candidate_from_reply(bytes: &[u8], src: SocketAddr) -> Option<Candidate> {
    let text = core_str(bytes)?;
    let sig = parse_signature(text).ok()?;
    let port = console_port(&sig)?;
    let addr = SocketAddr::new(src.ip(), port);
    Some(Candidate {
        kind: TransportKind::Net(addr.to_string()),
        addr: addr.to_string(),
        signature: sig,
    })
}

/// Extrae el puerto TCP de la consola del campo extra `tcp` de la firma.
fn console_port(sig: &Signature) -> Option<u16> {
    sig.extra.get("tcp").and_then(|v| v.parse::<u16>().ok())
}

/// Convierte bytes a `&str` recortando terminadores; `None` si no es UTF-8.
fn core_str(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes).ok().map(|s| s.trim())
}

/// Abre TCP a `target`, envía `IDENTIFY` y trata de parsear la firma.
fn probe_tcp(target: &str) -> Result<(SocketAddr, Signature)> {
    let addr = resolve(target)?;
    let mut stream = TcpStream::connect_timeout(&addr, PROBE_TIMEOUT)
        .with_context(|| format!("conectar {addr}"))?;
    stream.set_read_timeout(Some(PROBE_TIMEOUT)).ok();
    stream.write_all(IDENTIFY_REQUEST.as_bytes())?;
    stream.flush().ok();

    let mut asm = LineAssembler::new();
    let mut buf = [0u8; 256];
    let deadline = Instant::now() + PROBE_TIMEOUT;
    while Instant::now() < deadline {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                for line in asm.push(&buf[..n]) {
                    if let Ok(sig) = parse_signature(&line) {
                        return Ok((addr, sig));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("sin firma Rugus en {addr}")
}

/// Resuelve `target` (`ip:puerto`, o `ip` usando el puerto de descubrimiento).
fn resolve(target: &str) -> Result<SocketAddr> {
    if let Ok(addr) = target.parse::<SocketAddr>() {
        return Ok(addr);
    }
    if let Ok(ip) = target.parse::<std::net::IpAddr>() {
        return Ok(SocketAddr::new(ip, DISCOVERY_PORT));
    }
    anyhow::bail!("dirección de red inválida: {target} (usa ip o ip:puerto)")
}

/// Abre una sesión TCP viva: hilo lector → `bytes_rx`, hilo escritor ← `cmd_tx`.
pub fn connect(target: &str, signature: Signature) -> Result<Device> {
    let addr = resolve(target)?;
    let stream = TcpStream::connect_timeout(&addr, PROBE_TIMEOUT)
        .with_context(|| format!("conectar {addr}"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .ok();
    let mut reader = stream.try_clone().context("clonar socket TCP")?;
    let mut writer = stream;

    let (bytes_tx, bytes_rx) = mpsc::channel::<Vec<u8>>();
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

    // Hilo lector: vuelca bytes recibidos al canal de la TUI.
    thread::spawn(move || {
        let mut buf = [0u8; 512];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // Conexión cerrada por el dispositivo.
                Ok(n) => {
                    if bytes_tx.send(buf[..n].to_vec()).is_err() {
                        break; // TUI cerró el canal.
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => break,
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
        kind: TransportKind::Net(addr.to_string()),
        signature,
        bytes_rx,
        cmd_tx,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn sig_with_tcp(port: &str) -> Vec<u8> {
        format!("RUGUS;tier=full;chip=f769;proto=1;shell=rush;cli=1.0.0;tcp={port}").into_bytes()
    }

    #[test]
    fn reply_builds_candidate_with_source_ip_and_tcp_port() {
        let src: SocketAddr = "192.168.0.50:9001".parse().unwrap();
        let cand = candidate_from_reply(&sig_with_tcp("7777"), src).unwrap();
        assert_eq!(cand.addr, "192.168.0.50:7777");
        assert_eq!(cand.kind, TransportKind::Net("192.168.0.50:7777".into()));
        assert_eq!(cand.signature.chip, "f769");
    }

    #[test]
    fn reply_without_tcp_field_is_rejected() {
        let src: SocketAddr = "192.168.0.50:9001".parse().unwrap();
        let bytes = b"RUGUS;tier=full;chip=f769;proto=1;shell=rush;cli=1.0.0";
        assert!(candidate_from_reply(bytes, src).is_none());
    }

    #[test]
    fn non_rugus_reply_is_rejected() {
        let src: SocketAddr = "10.0.0.1:9001".parse().unwrap();
        assert!(candidate_from_reply(b"HELLO world", src).is_none());
    }

    #[test]
    fn resolve_accepts_ip_and_ip_port() {
        assert_eq!(
            resolve("192.168.0.50:7777").unwrap(),
            "192.168.0.50:7777".parse().unwrap()
        );
        let only_ip = resolve("192.168.0.50").unwrap();
        assert_eq!(only_ip.ip(), "192.168.0.50".parse::<IpAddr>().unwrap());
        assert_eq!(only_ip.port(), DISCOVERY_PORT);
        assert!(resolve("no-es-ip").is_err());
    }
}
