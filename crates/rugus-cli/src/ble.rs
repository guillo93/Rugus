//! Transporte BLE (btleplug, async) con fachada síncrona para la TUI.
//!
//! BLE es asíncrono; lo encapsulamos en un hilo dedicado con un runtime tokio.
//! La detección abre un runtime temporal, escanea, conecta, sondea IDENTIFY y
//! desconecta. La sesión viva reconecta por id de periférico y puentea
//! notificaciones ↔ canales de la TUI.
//!
//! Soporta dos perfiles comunes de puente serie-BLE:
//! - HM-10 / CC254x: servicio `FFE0`, característica `FFE1` (notify + write).
//! - Nordic UART Service (NUS): TX `6E400003` (notify), RX `6E400002` (write).

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Result};
use btleplug::api::{
    Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::StreamExt;

use rugus_proto::identify::IDENTIFY_REQUEST;
use rugus_proto::{parse_signature, LineAssembler, Signature};

use crate::device::{Candidate, Device, TransportKind};

/// Duración del escaneo BLE durante la detección.
const SCAN_TIME: Duration = Duration::from_millis(2500);
/// Ventana de espera de la respuesta IDENTIFY tras suscribirse.
const IDENTIFY_TIMEOUT: Duration = Duration::from_millis(2500);
/// Tiempo máximo para reencontrar un periférico al reconectar.
const RECONNECT_SCAN: Duration = Duration::from_millis(4000);

/// Característica de notificación y de escritura elegidas en un periférico.
struct Channels {
    notify: Characteristic,
    write: Characteristic,
    write_type: WriteType,
}

/// Escanea, sondea cada periférico y devuelve los que responden firma Rugus.
///
/// No falla si no hay adaptador BLE: simplemente devuelve una lista vacía.
pub fn detect() -> Vec<Candidate> {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return Vec::new(),
    };
    rt.block_on(async { detect_async().await.unwrap_or_default() })
}

async fn detect_async() -> Result<Vec<Candidate>> {
    let adapter = first_adapter().await?;
    adapter.start_scan(ScanFilter::default()).await?;
    tokio::time::sleep(SCAN_TIME).await;
    let peripherals = adapter.peripherals().await?;

    let mut found = Vec::new();
    for p in peripherals {
        let name = peripheral_name(&p).await;
        match probe(&p).await {
            Ok(sig) => {
                found.push(Candidate {
                    kind: TransportKind::Ble(name),
                    addr: p.id().to_string(),
                    signature: sig,
                });
            }
            Err(_) => {
                let _ = p.disconnect().await;
            }
        }
    }
    let _ = adapter.stop_scan().await;
    Ok(found)
}

/// Conecta a un periférico por su id y abre una sesión viva en un hilo tokio.
pub fn connect(addr: &str, name: String, signature: Signature) -> Result<Device> {
    let (bytes_tx, bytes_rx) = mpsc::channel::<Vec<u8>>();
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let addr = addr.to_string();

    let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();

    thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = ready_tx.send(Err(anyhow!("runtime BLE: {e}")));
                return;
            }
        };
        rt.block_on(async move {
            match session(&addr, bytes_tx, cmd_rx).await {
                Ok(setup) => {
                    let _ = ready_tx.send(Ok(()));
                    setup.await;
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        });
    });

    // Esperar a que la sesión confirme conexión + suscripción.
    match ready_rx.recv_timeout(RECONNECT_SCAN + IDENTIFY_TIMEOUT + Duration::from_secs(2)) {
        Ok(Ok(())) => Ok(Device {
            kind: TransportKind::Ble(name),
            signature,
            bytes_rx,
            cmd_tx,
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(anyhow!("timeout estableciendo sesión BLE")),
    }
}

/// Prepara la sesión y devuelve un future que la mantiene viva (bucle E/S).
async fn session(
    addr: &str,
    bytes_tx: mpsc::Sender<Vec<u8>>,
    mut cmd_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
) -> Result<impl std::future::Future<Output = ()>> {
    let adapter = first_adapter().await?;
    adapter.start_scan(ScanFilter::default()).await?;
    let peripheral = find_by_id(&adapter, addr, RECONNECT_SCAN)
        .await
        .ok_or_else(|| anyhow!("periférico BLE {addr} no encontrado"))?;
    let _ = adapter.stop_scan().await;

    if !peripheral.is_connected().await? {
        peripheral.connect().await?;
    }
    peripheral.discover_services().await?;
    let channels = pick_channels(&peripheral).ok_or_else(|| anyhow!("sin chars notify/write"))?;
    peripheral.subscribe(&channels.notify).await?;
    let mut notifications = peripheral.notifications().await?;

    Ok(async move {
        loop {
            tokio::select! {
                maybe = notifications.next() => {
                    match maybe {
                        Some(n) => {
                            if bytes_tx.send(n.value).is_err() {
                                break; // TUI cerró.
                            }
                        }
                        None => break, // Stream terminado.
                    }
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(data) => {
                            if peripheral.write(&channels.write, &data, channels.write_type).await.is_err() {
                                break;
                            }
                        }
                        None => break, // TUI cerró.
                    }
                }
            }
        }
        let _ = peripheral.disconnect().await;
    })
}

/// Conecta, descubre, sondea IDENTIFY y devuelve la firma (deja conectado;
/// el caller decide desconectar).
async fn probe(peripheral: &Peripheral) -> Result<Signature> {
    if !peripheral.is_connected().await? {
        peripheral.connect().await?;
    }
    peripheral.discover_services().await?;
    let channels = pick_channels(peripheral).ok_or_else(|| anyhow!("sin chars notify/write"))?;
    peripheral.subscribe(&channels.notify).await?;
    let mut notifications = peripheral.notifications().await?;

    peripheral
        .write(
            &channels.write,
            IDENTIFY_REQUEST.as_bytes(),
            channels.write_type,
        )
        .await?;

    let mut asm = LineAssembler::new();
    let result = tokio::time::timeout(IDENTIFY_TIMEOUT, async {
        while let Some(n) = notifications.next().await {
            for line in asm.push(&n.value) {
                if let Ok(sig) = parse_signature(&line) {
                    return Some(sig);
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten();

    result.ok_or_else(|| anyhow!("sin firma Rugus por BLE"))
}

/// Elige una característica notify y una de escritura del periférico.
fn pick_channels(peripheral: &Peripheral) -> Option<Channels> {
    let chars = peripheral.characteristics();
    let notify = chars
        .iter()
        .find(|c| c.properties.contains(CharPropFlags::NOTIFY))
        .or_else(|| {
            chars
                .iter()
                .find(|c| c.properties.contains(CharPropFlags::INDICATE))
        })?
        .clone();

    let (write, write_type) = chars
        .iter()
        .find(|c| c.properties.contains(CharPropFlags::WRITE))
        .map(|c| (c.clone(), WriteType::WithResponse))
        .or_else(|| {
            chars
                .iter()
                .find(|c| c.properties.contains(CharPropFlags::WRITE_WITHOUT_RESPONSE))
                .map(|c| (c.clone(), WriteType::WithoutResponse))
        })?;

    Some(Channels {
        notify,
        write,
        write_type,
    })
}

async fn first_adapter() -> Result<Adapter> {
    let manager = Manager::new().await?;
    let adapters = manager.adapters().await?;
    adapters
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("sin adaptador BLE"))
}

async fn find_by_id(adapter: &Adapter, addr: &str, timeout: Duration) -> Option<Peripheral> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(peripherals) = adapter.peripherals().await {
            for p in peripherals {
                if p.id().to_string() == addr {
                    return Some(p);
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn peripheral_name(peripheral: &Peripheral) -> String {
    match peripheral.properties().await {
        Ok(Some(props)) => props
            .local_name
            .unwrap_or_else(|| peripheral.id().to_string()),
        _ => peripheral.id().to_string(),
    }
}
