//! Rugus F5.B.1 — servicio de red bajo el scheduler en STM32F769I-DISCO.
//!
//! Demuestra la pila Ethernet+smoltcp (ya validada en `eth-link`) corriendo NO
//! en un super-bucle desnudo, sino como una **tarea de servicio del kernel
//! Rugus**, conviviendo con otra tarea privilegiada bajo el scheduler
//! cooperativo/preemptivo. Es el primer paso de F5.B (networking): el driver y
//! la pila viven en una tarea de prioridad `Service` que posee la `NetStack`;
//! una tarea `Kernel` late en paralelo (LED rojo). Sobre esa base, los PRs
//! siguientes exponen sockets a userland por IPC.
//!
//! Topología de red: IPv4 estática 192.168.0.50/24 (gw .1). La tarea de red
//! emite un **heartbeat UDP** cada ~1 s a 192.168.0.255:9000 (broadcast de
//! subred) con número de secuencia y uptime — observable con
//! `sudo tcpdump -i <iface> udp port 9000` desde un PC en la misma LAN.
//!
//! Tareas:
//! - kernel (priv, prioridad Kernel): latido en LD Red, cede el CPU.
//! - net    (priv, prioridad Service): posee la pila smoltcp; drena el DMA,
//!   hace `poll` y envía el heartbeat UDP. Cede el CPU entre iteraciones.

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use core::fmt::Write as _;
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
use rugus_hal_stm32f7::rcc;
use rugus_net::{NetStack, StaticConfig};
use rugus_runtime::entry;
use smoltcp::iface::{SocketHandle, SocketStorage};
use smoltcp::socket::udp;
use smoltcp::time::Instant;
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};

/// Puerto y destino del heartbeat UDP (broadcast de subred 192.168.0.0/24).
const HB_PORT: u16 = 9000;
const HB_DEST: Ipv4Address = Ipv4Address::new(192, 168, 0, 255);

const ETH_RING_ENTRIES: usize = 4;

#[repr(C, align(4096))]
struct Stack4k([u8; 4096]);

static mut STACK_KERNEL: Stack4k = Stack4k([0; 4096]);
static mut STACK_NET: Stack4k = Stack4k([0; 4096]);

#[link_section = ".eth_dma"]
static mut RX_RING: [RxRingEntry; ETH_RING_ENTRIES] = [RxRingEntry::INIT; ETH_RING_ENTRIES];
#[link_section = ".eth_dma"]
static mut TX_RING: [TxRingEntry; ETH_RING_ENTRIES] = [TxRingEntry::INIT; ETH_RING_ENTRIES];

// Estado de red de vida 'static, poseído por la tarea `net`. El DMA vive en su
// propio static y la `NetStack` toma prestada una referencia 'static a él, sin
// aliasing (dos statics distintos).
static mut ETH_DMA: Option<EthernetDMA<'static, 'static>> = None;
static mut SOCK_STORAGE: [SocketStorage; 2] = [SocketStorage::EMPTY; 2];
static mut UDP_RX_META: [udp::PacketMetadata; 8] = [udp::PacketMetadata::EMPTY; 8];
static mut UDP_RX_PAYLOAD: [u8; 1024] = [0; 1024];
static mut UDP_TX_META: [udp::PacketMetadata; 8] = [udp::PacketMetadata::EMPTY; 8];
static mut UDP_TX_PAYLOAD: [u8; 1024] = [0; 1024];
static mut NET: Option<NetStack<'static, EthernetDMA<'static, 'static>>> = None;
static mut UDP_HANDLE: Option<SocketHandle> = None;

/// LED de latido del kernel (LD Red), creado en `main` y manejado por la tarea
/// kernel (una tarea no puede tomar `dp` por sí misma).
static mut HB_LED: Option<LedPin> = None;

#[entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("cortex-m peripherals");
    let dp = pac::Peripherals::take().expect("device peripherals");

    let clocks = rcc::init(&dp);
    rugus_runtime::enable_cycle_counter(&mut cp);

    defmt::info!(
        "rugus net-service @ STM32F769I-DISCO, SYSCLK {} MHz",
        clocks.sysclk_mhz()
    );

    // FPU-context + fault handlers + layout MPU de sandbox. Luego reconfiguramos
    // la región MPU 1 como no-cacheable para el DMA de Ethernet (coherencia):
    // `enable_with_eth_dma` reprograma la región MPU 6 (alta) como ETH
    // Non-Cacheable y conserva el resto del mapa de `platform_init`; al ser de
    // mayor número gana el solapamiento con KERNEL_RAM (región 2), de modo que
    // los descriptores DMA quedan realmente fuera de caché.
    platform_init(&mut cp, &MpuLayout::STM32F769);
    cache::enable_with_eth_dma(&mut cp.SCB, &mut cp.CPUID, &mut cp.MPU);

    // Base de tiempo del kernel (SysTick de 1 ms): alimenta `now_ms` (preempción
    // del scheduler) y los `Instant` de smoltcp.
    time::init(&mut cp.SYST, clocks.hclk);

    // Bring-up de Ethernet: pines RMII, periférico, MAC+DMA, IRQ y PHY.
    configure_disco_pins(&dp);
    enable_peripheral();
    let parts = PartsIn::new(dp.ETHERNET_MAC, dp.ETHERNET_MMC, dp.ETHERNET_DMA);
    let (rx_ring, tx_ring) = eth_rings();
    let EthStack { mut dma, mac } = eth::init(parts, &clocks, rx_ring, tx_ring).expect("eth init");
    enable_eth_interrupt(&dma);
    defmt::info!("ETH MAC+DMA init OK; MAC {:02x}", DEFAULT_MAC);

    // Espera de enlace + autoneg (cable a CN10). En bring-up, antes de arrancar el
    // scheduler; tras el enlace ya no necesitamos el PHY/MII y lo consumimos.
    let mut phy = LAN8742A::new(mac, LAN8742_PHY_ADDR);
    init_phy(&mut phy);
    defmt::info!("esperando enlace PHY + autoneg (cable a CN10)...");
    while !link_established(&mut phy) {
        cortex_m::asm::delay(clocks.sysclk / 20);
    }
    sync_mac_speed_from_phy(&mut phy);
    dma.restart_after_link_up();
    defmt::info!("PHY link up (autoneg done)");

    // Construye la pila IPv4 estática + socket UDP, todo en almacenamiento
    // 'static, dejándola lista en el static `NET` para la tarea de red.
    let cfg = StaticConfig::home_lan();
    // SAFETY: arranque single-thread; estos statics solo se inicializan aquí y a
    // partir de `start()` los toca en exclusiva la tarea `net`.
    unsafe {
        ETH_DMA = Some(dma);
        let dma_ref: &'static mut EthernetDMA<'static, 'static> =
            (*addr_of_mut!(ETH_DMA)).as_mut().unwrap();
        let storage: &'static mut [SocketStorage] = &mut *addr_of_mut!(SOCK_STORAGE);
        let mut net = NetStack::new_static(DEFAULT_MAC, cfg, dma_ref, storage);

        let rx_meta: &mut [udp::PacketMetadata] = &mut *addr_of_mut!(UDP_RX_META);
        let rx_payload: &mut [u8] = &mut *addr_of_mut!(UDP_RX_PAYLOAD);
        let tx_meta: &mut [udp::PacketMetadata] = &mut *addr_of_mut!(UDP_TX_META);
        let tx_payload: &mut [u8] = &mut *addr_of_mut!(UDP_TX_PAYLOAD);
        let rx_buf = udp::PacketBuffer::new(rx_meta, rx_payload);
        let tx_buf = udp::PacketBuffer::new(tx_meta, tx_payload);
        let mut sock = udp::Socket::new(rx_buf, tx_buf);
        sock.bind(HB_PORT).expect("udp bind");
        UDP_HANDLE = Some(net.sockets_mut().add(sock));
        NET = Some(net);

        HB_LED = Some(LedPin::new(&dp.RCC, DiscoLed::Red));
    }

    defmt::info!(
        "IPv4 estatica {}.{}.{}.{}/{} — heartbeat UDP a {}.{}.{}.{}:{}",
        cfg.address.octets()[0],
        cfg.address.octets()[1],
        cfg.address.octets()[2],
        cfg.address.octets()[3],
        cfg.prefix_len,
        HB_DEST.octets()[0],
        HB_DEST.octets()[1],
        HB_DEST.octets()[2],
        HB_DEST.octets()[3],
        HB_PORT
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

/// Tarea de red: posee la pila smoltcp. Drena el DMA, hace `poll` y emite un
/// heartbeat UDP cada ~1 s. Cede el CPU entre iteraciones (cooperativa).
fn net_task() -> ! {
    let mut last_hb = time::now_ms();
    let mut seq = 0u32;
    loop {
        // SAFETY: `NET`/`UDP_HANDLE` solo los toca esta tarea tras `start()`.
        unsafe {
            if let Some(net) = (*addr_of_mut!(NET)).as_mut() {
                net.device_mut().service_dma();
                net.poll(Instant::from_millis(time::now_ms() as i64));

                let t = time::now_ms();
                if t.wrapping_sub(last_hb) >= 1000 {
                    last_hb = t;
                    seq = seq.wrapping_add(1);
                    if let Some(h) = UDP_HANDLE {
                        let sock = net.sockets_mut().get_mut::<udp::Socket>(h);
                        let mut buf = [0u8; 64];
                        let payload = format_hb(&mut buf, seq, t);
                        let ep = IpEndpoint::new(IpAddress::Ipv4(HB_DEST), HB_PORT);
                        match sock.send_slice(payload, ep) {
                            Ok(()) => defmt::info!(
                                "UDP hb seq={=u32} up={=u32}ms -> broadcast:{=u16}",
                                seq,
                                t,
                                HB_PORT
                            ),
                            Err(e) => {
                                defmt::warn!("UDP hb send err: {}", defmt::Debug2Format(&e))
                            }
                        }
                    }
                    // Empuja el datagrama recién encolado a la cola de TX del DMA.
                    net.poll(Instant::from_millis(time::now_ms() as i64));
                }
            }
        }
        rugus_kernel::cpu_yield();
    }
}

/// Escribe el cuerpo ASCII del heartbeat en `buf` y devuelve la porción usada.
fn format_hb(buf: &mut [u8], seq: u32, uptime_ms: u32) -> &[u8] {
    struct Cursor<'a> {
        b: &'a mut [u8],
        n: usize,
    }
    impl core::fmt::Write for Cursor<'_> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let bytes = s.as_bytes();
            let end = self.n + bytes.len();
            if end > self.b.len() {
                return Err(core::fmt::Error);
            }
            self.b[self.n..end].copy_from_slice(bytes);
            self.n = end;
            Ok(())
        }
    }
    let mut c = Cursor { b: buf, n: 0 };
    // Si no cabe (no debería con 64 bytes), envía lo escrito hasta el momento.
    let _ = writeln!(c, "RUGUS hb seq={} up={}ms", seq, uptime_ms);
    let n = c.n;
    &buf[..n]
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
