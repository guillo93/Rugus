//! Rugus F6.3b — consola universal `rush` sobre red (LAN/WiFi) en STM32F769I-DISCO.
//!
//! Tercer transporte de Rugus, tras serie y BLE: el dispositivo se descubre por
//! red y expone la **misma consola `rush`** sobre TCP, con el **mismo gate de
//! autenticación de canal** (challenge-response HMAC) que la personalidad lite
//! del F103. La PSK vive en un subsector QSPI que la consola no puede leer
//! ([`psk`]); `rush` nunca ve el secreto (lo delega en [`auth`]).
//!
//! Topología: IPv4 estática 192.168.0.50/24 (gw .1). Dos servicios de red sobre
//! la misma pila smoltcp ya validada en `net-service`:
//!
//! - **Descubrimiento** (UDP `DISCOVERY_PORT` = 9001): el host (`rugus-cli`)
//!   emite `IDENTIFY` por broadcast; el dispositivo responde su firma al emisor
//!   con el campo extra `tcp=<CONSOLE_PORT>`, indicando dónde abrir la consola.
//! - **Consola** (TCP `CONSOLE_PORT` = 7777): al conectar, `rush` corre sobre el
//!   stream byte a byte; sin sesión autenticada solo pasan IDENTIFY y el propio
//!   handshake (`knock`/`prove`/`lock`/`enroll`). Cada nueva conexión re-bloquea.
//!
//! Tareas:
//! - kernel (priv, prioridad Kernel): latido en LD Red, cede el CPU.
//! - net    (priv, prioridad Service): posee la pila smoltcp + QSPI + consola.

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use core::ptr::addr_of_mut;

use ieee802_3_miim::phy::lan87xxa::LAN8742A;
use rugus_arch_cortex_m::{platform_init, time, MpuLayout};
use rugus_core::sched::Priority;
use rugus_hal::GpioPin;
use rugus_hal_stm32f7::cache;
use rugus_hal_stm32f7::eth::{
    self, configure_disco_pins, enable_eth_interrupt, enable_peripheral, eth_interrupt_handler,
    init_phy, link_established, sync_mac_speed_from_phy, EthStack, EthernetDMA, PartsIn,
    RxRingEntry, TxRingEntry, DEFAULT_MAC, LAN8742_PHY_ADDR,
};
use rugus_hal_stm32f7::gpio::{DiscoLed, LedPin};
use rugus_hal_stm32f7::pac::{self, interrupt};
use rugus_hal_stm32f7::qspi::Qspi;
use rugus_hal_stm32f7::rcc;
use rugus_net::{NetStack, StaticConfig};
use rugus_runtime::entry;
use rush::{execute_authed, identify, parse, write_signature_ext, AuthHooks, Session, Write};
use smoltcp::iface::{SocketHandle, SocketStorage};
use smoltcp::socket::{tcp, udp};
use smoltcp::time::Instant;

/// Puerto UDP de descubrimiento (espejo de `rugus_proto::DISCOVERY_PORT`).
const DISCOVERY_PORT: u16 = 9001;
/// Puerto TCP donde escucha la consola `rush`.
const CONSOLE_PORT: u16 = 7777;
/// Campo extra de la firma que anuncia el puerto de consola al host.
const SIG_EXTRA: &str = ";tcp=7777";
/// Tier/chip de esta placa para la firma IDENTIFY (tier full, familia f769).
const TIER: &str = "full";
const CHIP: &str = "f769";

const ETH_RING_ENTRIES: usize = 4;
/// Capacidad de la línea de comando en curso (consola sobre TCP).
const LINE_CAP: usize = 256;
/// Buffer de salida de la consola por iteración (firma + respuestas rush).
const OUT_CAP: usize = 1024;

#[repr(C, align(4096))]
struct Stack4k([u8; 4096]);

static mut STACK_KERNEL: Stack4k = Stack4k([0; 4096]);
static mut STACK_NET: Stack4k = Stack4k([0; 4096]);

#[link_section = ".eth_dma"]
static mut RX_RING: [RxRingEntry; ETH_RING_ENTRIES] = [RxRingEntry::INIT; ETH_RING_ENTRIES];
#[link_section = ".eth_dma"]
static mut TX_RING: [TxRingEntry; ETH_RING_ENTRIES] = [TxRingEntry::INIT; ETH_RING_ENTRIES];

// Estado de red 'static, poseído por la tarea `net` tras `start()`.
static mut ETH_DMA: Option<EthernetDMA<'static, 'static>> = None;
static mut SOCK_STORAGE: [SocketStorage; 4] = [SocketStorage::EMPTY; 4];
static mut NET: Option<NetStack<'static, EthernetDMA<'static, 'static>>> = None;

// Almacenamiento de sockets UDP (descubrimiento) y TCP (consola).
static mut UDP_RX_META: [udp::PacketMetadata; 8] = [udp::PacketMetadata::EMPTY; 8];
static mut UDP_RX_PAYLOAD: [u8; 512] = [0; 512];
static mut UDP_TX_META: [udp::PacketMetadata; 8] = [udp::PacketMetadata::EMPTY; 8];
static mut UDP_TX_PAYLOAD: [u8; 512] = [0; 512];
static mut UDP_HANDLE: Option<SocketHandle> = None;

static mut TCP_RX_PAYLOAD: [u8; 1024] = [0; 1024];
static mut TCP_TX_PAYLOAD: [u8; 1024] = [0; 1024];
static mut TCP_HANDLE: Option<SocketHandle> = None;

// Estado de la consola sobre TCP: sesión de auth, línea en curso y si hay un
// cliente conectado (para re-bloquear y re-anunciar el banner por conexión).
static mut SESSION: Session = Session::new();
static mut AUTH_HOOKS: Option<AuthHooks> = None;
static mut LINE: [u8; LINE_CAP] = [0; LINE_CAP];
static mut LINE_LEN: usize = 0;
static mut CLIENT_CONNECTED: bool = false;

/// LED de latido del kernel (LD Red), manejado por la tarea kernel.
static mut HB_LED: Option<LedPin> = None;

/// Escritor de salida hacia un buffer de pila; lo vacía la consola al socket.
struct BufOut<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl Write for BufOut<'_> {
    fn write_str(&mut self, s: &str) -> Result<(), ()> {
        let b = s.as_bytes();
        let end = self.len + b.len();
        if end > self.buf.len() {
            return Err(());
        }
        self.buf[self.len..end].copy_from_slice(b);
        self.len = end;
        Ok(())
    }
}

#[entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("cortex-m peripherals");
    let dp = pac::Peripherals::take().expect("device peripherals");

    let clocks = rcc::init(&dp);
    rugus_runtime::enable_cycle_counter(&mut cp);

    defmt::info!(
        "rugus net-console @ STM32F769I-DISCO, SYSCLK {} MHz",
        clocks.sysclk_mhz()
    );

    // FPU-context + fault handlers + layout MPU; región alta no-cacheable para
    // el DMA de Ethernet (coherencia), igual que en `net-service`.
    platform_init(&mut cp, &MpuLayout::STM32F769);
    cache::enable_with_eth_dma(&mut cp.SCB, &mut cp.CPUID, &mut cp.MPU);

    // Base de tiempo del kernel (SysTick 1 ms): preempción + `Instant` smoltcp +
    // `now_ms` del gate de auth (timeout de sesión).
    time::init(&mut cp.SYST, clocks.hclk);

    // Bring-up de Ethernet: pines RMII, periférico, MAC+DMA, IRQ y PHY.
    configure_disco_pins(&dp);
    enable_peripheral();

    // Almacén de PSK en QSPI: trae el handle al módulo `psk` (consume QUADSPI,
    // por eso va tras los usos de `&dp` completos). `Qspi::new` valida el medio.
    let qspi = Qspi::new(dp.QUADSPI, &dp.RCC).expect("qspi init");
    // SAFETY: arranque single-thread; el handle solo lo usa la tarea `net`.
    unsafe {
        psk::install(qspi);
        AUTH_HOOKS = Some(auth::hooks());
    }
    defmt::info!("QSPI PSK store listo (provisioned={})", psk::provisioned());

    let parts = PartsIn::new(dp.ETHERNET_MAC, dp.ETHERNET_MMC, dp.ETHERNET_DMA);
    let (rx_ring, tx_ring) = eth_rings();
    let EthStack { mut dma, mac } = eth::init(parts, &clocks, rx_ring, tx_ring).expect("eth init");
    enable_eth_interrupt(&dma);

    let mut phy = LAN8742A::new(mac, LAN8742_PHY_ADDR);
    init_phy(&mut phy);
    defmt::info!("esperando enlace PHY + autoneg (cable a CN10)...");
    while !link_established(&mut phy) {
        cortex_m::asm::delay(clocks.sysclk / 20);
    }
    sync_mac_speed_from_phy(&mut phy);
    dma.restart_after_link_up();
    defmt::info!("PHY link up (autoneg done)");

    // Pila IPv4 estática + sockets UDP (descubrimiento) y TCP (consola).
    let cfg = StaticConfig::home_lan();
    // SAFETY: arranque single-thread; estos statics se inicializan solo aquí y a
    // partir de `start()` los toca en exclusiva la tarea `net`.
    unsafe {
        ETH_DMA = Some(dma);
        let dma_ref: &'static mut EthernetDMA<'static, 'static> =
            (*addr_of_mut!(ETH_DMA)).as_mut().unwrap();
        let storage: &'static mut [SocketStorage] = &mut *addr_of_mut!(SOCK_STORAGE);
        let mut net = NetStack::new_static(DEFAULT_MAC, cfg, dma_ref, storage);

        // Socket UDP de descubrimiento, escuchando en DISCOVERY_PORT.
        let urx_meta: &mut [udp::PacketMetadata] = &mut *addr_of_mut!(UDP_RX_META);
        let urx_pay: &mut [u8] = &mut *addr_of_mut!(UDP_RX_PAYLOAD);
        let utx_meta: &mut [udp::PacketMetadata] = &mut *addr_of_mut!(UDP_TX_META);
        let utx_pay: &mut [u8] = &mut *addr_of_mut!(UDP_TX_PAYLOAD);
        let udp_rx = udp::PacketBuffer::new(urx_meta, urx_pay);
        let udp_tx = udp::PacketBuffer::new(utx_meta, utx_pay);
        let mut usock = udp::Socket::new(udp_rx, udp_tx);
        usock.bind(DISCOVERY_PORT).expect("udp bind");
        UDP_HANDLE = Some(net.sockets_mut().add(usock));

        // Socket TCP de consola, en escucha pasiva sobre CONSOLE_PORT.
        let trx_pay: &mut [u8] = &mut *addr_of_mut!(TCP_RX_PAYLOAD);
        let ttx_pay: &mut [u8] = &mut *addr_of_mut!(TCP_TX_PAYLOAD);
        let tcp_rx = tcp::SocketBuffer::new(trx_pay);
        let tcp_tx = tcp::SocketBuffer::new(ttx_pay);
        let mut tsock = tcp::Socket::new(tcp_rx, tcp_tx);
        tsock.listen(CONSOLE_PORT).expect("tcp listen");
        TCP_HANDLE = Some(net.sockets_mut().add(tsock));

        NET = Some(net);
        HB_LED = Some(LedPin::new(&dp.RCC, DiscoLed::Red));
    }

    defmt::info!(
        "IPv4 192.168.0.50/24 — descubrimiento UDP:{} · consola TCP:{}",
        DISCOVERY_PORT,
        CONSOLE_PORT
    );

    // SAFETY: pilas estáticas vivas para todo el kernel; spawn antes de start.
    unsafe {
        rugus_kernel::spawn(
            &mut (*addr_of_mut!(STACK_KERNEL)).0,
            kernel_task,
            Priority::Kernel,
        )
        .expect("spawn kernel");
        rugus_kernel::spawn(
            &mut (*addr_of_mut!(STACK_NET)).0,
            net_task,
            Priority::Service,
        )
        .expect("spawn net");
        defmt::info!("scheduler: 2 tareas (kernel + net), starting");
        rugus_kernel::start();
    }
}

/// Tarea kernel: latido visible en LD Red ~1 Hz; cede el CPU al resto.
fn kernel_task() -> ! {
    let mut last = time::now_ms();
    loop {
        let t = time::now_ms();
        if t.wrapping_sub(last) >= 500 {
            last = t;
            // SAFETY: solo esta tarea toca el LED de latido.
            unsafe {
                if let Some(led) = (*addr_of_mut!(HB_LED)).as_mut() {
                    let _ = led.toggle();
                }
            }
        }
        rugus_kernel::cpu_yield();
    }
}

/// Tarea de red: posee la pila smoltcp. Drena el DMA, hace `poll` y atiende el
/// descubrimiento (UDP) y la consola (TCP). Cede el CPU entre iteraciones.
fn net_task() -> ! {
    loop {
        // SAFETY: `NET`/handles/consola solo los toca esta tarea tras `start()`.
        unsafe {
            if let Some(net) = (*addr_of_mut!(NET)).as_mut() {
                net.device_mut().service_dma();
                net.poll(Instant::from_millis(time::now_ms() as i64));
                service_discovery(net);
                service_console(net);
                // Empuja a TX lo que se haya encolado en esta iteración.
                net.poll(Instant::from_millis(time::now_ms() as i64));
            }
        }
        rugus_kernel::cpu_yield();
    }
}

/// Atiende el socket UDP de descubrimiento: ante un `IDENTIFY`/ENQ, responde la
/// firma (con `tcp=<CONSOLE_PORT>`) al emisor.
///
/// # Safety
/// Solo la tarea `net` la llama; accede a `UDP_HANDLE` sin reentrada.
unsafe fn service_discovery(net: &mut NetStack<'static, EthernetDMA<'static, 'static>>) {
    let Some(h) = (unsafe { UDP_HANDLE }) else {
        return;
    };
    let sock = net.sockets_mut().get_mut::<udp::Socket>(h);
    if !sock.can_recv() {
        return;
    }
    let mut req = [0u8; 64];
    let (n, meta) = match sock.recv_slice(&mut req) {
        Ok(v) => v,
        Err(_) => return,
    };
    if !is_identify(&req[..n]) {
        return;
    }
    let mut buf = [0u8; OUT_CAP];
    let mut out = BufOut {
        buf: &mut buf,
        len: 0,
    };
    write_signature_ext(&mut out, TIER, CHIP, SIG_EXTRA);
    let len = out.len;
    let _ = sock.send_slice(&buf[..len], meta.endpoint);
    defmt::info!("descubrimiento: IDENTIFY respondido a un host");
}

/// `true` si el payload es una solicitud IDENTIFY (línea `IDENTIFY` o byte ENQ).
fn is_identify(req: &[u8]) -> bool {
    if req.contains(&identify::ENQ) {
        return true;
    }
    core::str::from_utf8(req)
        .map(|s| s.trim().eq_ignore_ascii_case("identify"))
        .unwrap_or(false)
}

/// Atiende el socket TCP de consola: gestiona conexión/desconexión, corre `rush`
/// sobre los bytes recibidos y vuelca la salida al stream.
///
/// # Safety
/// Solo la tarea `net` la llama; accede a la consola/sesión sin reentrada.
unsafe fn service_console(net: &mut NetStack<'static, EthernetDMA<'static, 'static>>) {
    let Some(h) = (unsafe { TCP_HANDLE }) else {
        return;
    };
    let hooks = match unsafe { (*addr_of_mut!(AUTH_HOOKS)).as_ref() } {
        Some(hk) => hk,
        None => return,
    };

    let sock = net.sockets_mut().get_mut::<tcp::Socket>(h);
    let active = sock.is_active();

    // Transición de conexión: re-bloquea la sesión y saluda con el banner.
    if active && !(unsafe { CLIENT_CONNECTED }) {
        unsafe {
            CLIENT_CONNECTED = true;
            SESSION = Session::new();
            LINE_LEN = 0;
        }
        let mut buf = [0u8; OUT_CAP];
        let mut out = BufOut {
            buf: &mut buf,
            len: 0,
        };
        let _ = out.write_str(
            "\r\nRugus F769 net-console.\r\nCanal gateado: autentícate con `knock` y `prove`.\r\n\r\n",
        );
        let len = out.len;
        if sock.can_send() {
            let _ = sock.send_slice(&buf[..len]);
        }
    }

    // Desconexión: el cliente cerró; reanuda la escucha pasiva.
    if !active && (unsafe { CLIENT_CONNECTED }) {
        unsafe { CLIENT_CONNECTED = false };
        sock.abort();
        let _ = sock.listen(CONSOLE_PORT);
        return;
    }

    if !sock.can_recv() {
        return;
    }
    let mut rx = [0u8; 256];
    let n = match sock.recv_slice(&mut rx) {
        Ok(n) => n,
        Err(_) => return,
    };
    if n == 0 {
        return;
    }

    // Procesa los bytes recibidos a través de `rush`, acumulando la salida.
    let mut buf = [0u8; OUT_CAP];
    let mut out = BufOut {
        buf: &mut buf,
        len: 0,
    };
    process_console(&rx[..n], &mut out, hooks);
    let len = out.len;
    if len > 0 && sock.can_send() {
        let _ = sock.send_slice(&buf[..len]);
    }
}

/// Alimenta `rush` byte a byte: ENQ/`IDENTIFY` → firma de red; CR/LF cierra la
/// línea y la ejecuta con el gate de auth; backspace edita; el resto acumula.
///
/// # Safety
/// Usa los statics `LINE`/`LINE_LEN`/`SESSION`; solo la tarea `net` la llama.
unsafe fn process_console(rx: &[u8], out: &mut BufOut<'_>, hooks: &AuthHooks) {
    for &b in rx {
        // Fast-path de descubrimiento por byte de control.
        if b == identify::ENQ {
            write_signature_ext(out, TIER, CHIP, SIG_EXTRA);
            continue;
        }
        if b == b'\r' || b == b'\n' {
            let len = unsafe { LINE_LEN };
            if len > 0 {
                let line = core::str::from_utf8(unsafe { &LINE[..len] }).unwrap_or("");
                // IDENTIFY por línea: firma de red propia (tier/chip de la placa).
                if line.trim().eq_ignore_ascii_case("identify") {
                    write_signature_ext(out, TIER, CHIP, SIG_EXTRA);
                } else {
                    let cmd = parse(line);
                    execute_authed(cmd, line, out, unsafe { &mut SESSION }, hooks);
                }
                unsafe { LINE_LEN = 0 };
            }
        } else if b == 0x7F || b == 0x08 {
            unsafe {
                LINE_LEN = LINE_LEN.saturating_sub(1);
            }
        } else {
            unsafe {
                if LINE_LEN < LINE.len() {
                    LINE[LINE_LEN] = b;
                    LINE_LEN += 1;
                }
            }
        }
    }
}

fn eth_rings() -> (&'static mut [RxRingEntry], &'static mut [TxRingEntry]) {
    // SAFETY: rings estáticos en `.eth_dma`, accedidos solo durante el bring-up
    // y luego propiedad del DMA.
    unsafe {
        let rx = addr_of_mut!(RX_RING);
        let tx = addr_of_mut!(TX_RING);
        (&mut *rx, &mut *tx)
    }
}

#[interrupt]
fn ETH() {
    let _ = eth_interrupt_handler();
}

mod auth;
mod psk;
