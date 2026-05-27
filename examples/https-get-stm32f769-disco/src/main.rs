//! Rugus G4 — HTTPS GET against a LAN server on STM32F769I-DISCO.

#![no_std]
#![no_main]

extern crate alloc;

use core::cell::RefCell;

use cortex_m::interrupt::Mutex;
use cortex_m_rt::exception;
use embedded_io::{Read, Write};
use ieee802_3_miim::phy::lan87xxa::LAN8742A;
use rugus_core::heap;
use rugus_crypto::SoftwareRng;
use rugus_hal_stm32f7::cache;
use rugus_hal_stm32f7::eth::{
    self, configure_disco_pins, enable_eth_interrupt, enable_peripheral, eth_interrupt_handler,
    init_phy, link_established, sync_mac_speed_from_phy, take_eth_irq_pending, EthStack,
    EthernetDMA, PartsIn, RxRingEntry, TxRingEntry, DEFAULT_MAC, LAN8742_PHY_ADDR,
};
use rugus_hal_stm32f7::fmc::{self, SDRAM_BASE};
use rugus_hal_stm32f7::pac;
use rugus_hal_stm32f7::rcc;
use rugus_net::{
    tcp_connect, Endpoint, NetStack, StaticConfig, TcpError, TcpIo, DEFAULT_TCP_LOCAL_PORT,
};
use rugus_runtime::entry;
use rugus_tls::{Aes128GcmSha256, TlsClient};
use smoltcp::iface::SocketStorage;
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use stm32f7::stm32f7x9::interrupt;

/// SNI / Host header — match your test server certificate CN or `-servername`.
const SERVER_NAME: &str = "rugus-test";
/// TLS record read buffer (max TLS record ≈ 16 KiB).
const TLS_READ_LEN: usize = 16384;
/// TLS record write buffer.
const TLS_WRITE_LEN: usize = 4096;

static TIME_MS: Mutex<RefCell<u64>> = Mutex::new(RefCell::new(0));

const ETH_RING_ENTRIES: usize = 4;

#[link_section = ".eth_dma"]
static mut RX_RING: [RxRingEntry; ETH_RING_ENTRIES] = [RxRingEntry::INIT; ETH_RING_ENTRIES];

#[link_section = ".eth_dma"]
static mut TX_RING: [TxRingEntry; ETH_RING_ENTRIES] = [TxRingEntry::INIT; ETH_RING_ENTRIES];

static mut TCP_RX_BUF: [u8; 2048] = [0; 2048];
static mut TCP_TX_BUF: [u8; 2048] = [0; 2048];

#[entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("cp");
    let dp = pac::Peripherals::take().expect("dp");

    let clocks = rcc::init(&dp);
    // Match eth-link exactly: ETH MPU + caches BEFORE SysTick / GPIOs.
    cache::enable_with_eth_dma(&mut cp.SCB, &mut cp.CPUID, &mut cp.MPU);
    setup_systick(&mut cp.SYST);

    defmt::info!(
        "rugus https-get @ STM32F769I-DISCO, SYSCLK {} MHz",
        clocks.sysclk_mhz()
    );

    configure_disco_pins(&dp);
    defmt::debug!("RMII pins configured");
    enable_peripheral();
    defmt::debug!("ETH peripheral enabled");

    // Heap on internal SRAM only — TLS buffers are stack-allocated; this only
    // satisfies the global allocator if any indirect dep tries to alloc.
    init_heap(&dp, &mut cp);

    let parts = PartsIn::new(dp.ETHERNET_MAC, dp.ETHERNET_MMC, dp.ETHERNET_DMA);
    let (rx_ring, tx_ring) = eth_rings();

    let EthStack { mut dma, mac } = eth::init(parts, &clocks, rx_ring, tx_ring).expect("eth init");
    defmt::debug!("ETH MAC+DMA init OK (rings idle until link up)");

    enable_eth_interrupt(&dma);

    defmt::info!("MAC {:02x}", DEFAULT_MAC);

    let mut phy = LAN8742A::new(mac, LAN8742_PHY_ADDR);
    init_phy(&mut phy);

    defmt::info!("waiting for PHY link + autoneg (cable to CN10)...");
    while !link_established(&mut phy) {
        cortex_m::asm::delay(clocks.sysclk / 20);
    }
    sync_mac_speed_from_phy(&mut phy);
    defmt::info!("PHY link up (autoneg done)");
    dma.restart_after_link_up();

    // Enable DWT cycle counter (used by `defmt::timestamp!` and TLS RNG seed)
    // AFTER the Ethernet stack is up, to keep the early init sequence
    // byte-for-byte identical to the eth-link smoke test.
    rugus_runtime::enable_cycle_counter(&mut cp);

    let regs = eth::eth_regs(&dma);
    defmt::info!(
        "ETH regs maccr={:08x} dmabmr={:08x} dmasr={:08x} dmaomr={:08x} mmc_rx={} mmc_tx={}",
        regs.maccr,
        regs.dmabmr,
        regs.dmasr,
        regs.dmaomr,
        regs.mmc_rx_unicast,
        regs.mmc_tx_good
    );

    let cfg = StaticConfig::home_lan();
    let mut socket_storage: [SocketStorage; 2] = Default::default();
    let mut net = NetStack::new_static(DEFAULT_MAC, cfg, &mut dma, &mut socket_storage);

    wait_ipv4(&mut net, &cfg);

    // Probe phase: drive smoltcp for ~8 s so the host can ARP/ping the board
    // and confirm L2/L3 from the firmware side before any TCP attempt.
    defmt::info!("L2 probe window 8 s — try `ping 192.168.0.50` from host now");
    let probe_start = now_ms();
    let mut last_log = probe_start;
    while now_ms().saturating_sub(probe_start) < 8_000 {
        net.device_mut().service_dma();
        net.poll(Instant::from_millis(now_ms() as i64));
        let t = now_ms();
        if t.saturating_sub(last_log) >= 1000 {
            last_log = t;
            let stats = eth::eth_stats(net.device_mut());
            defmt::info!(
                "L2 t={=u64}ms rx={=u32} tx={=u32} rps={=u8} tps={=u8} rbus={=bool} tbus={=bool}",
                t.saturating_sub(probe_start),
                stats.rx_frames,
                stats.tx_frames,
                stats.rx_dma_state,
                stats.tx_dma_state,
                stats.rx_buf_unavail,
                stats.tx_buf_unavail
            );
        }
        cortex_m::asm::delay(clocks.sysclk / 100);
    }
    defmt::info!("L2 probe done");

    let (tcp_handle, remote) = {
        let rx = unsafe { &mut *core::ptr::addr_of_mut!(TCP_RX_BUF) };
        let tx = unsafe { &mut *core::ptr::addr_of_mut!(TCP_TX_BUF) };
        let tcp_rx = tcp::SocketBuffer::new(&mut rx[..]);
        let tcp_tx = tcp::SocketBuffer::new(&mut tx[..]);
        let tcp = tcp::Socket::new(tcp_rx, tcp_tx);
        let handle = net.sockets_mut().add(tcp);
        (handle, Endpoint::lan_https_server())
    };

    defmt::info!(
        "TCP connect {}.{}.{}.{}:{}",
        remote.addr.octets()[0],
        remote.addr.octets()[1],
        remote.addr.octets()[2],
        remote.addr.octets()[3],
        remote.port
    );

    match tcp_connect(
        &mut net,
        tcp_handle,
        remote,
        DEFAULT_TCP_LOCAL_PORT,
        now_ms,
        15_000,
    ) {
        Ok(()) => defmt::info!("TCP established"),
        Err(e) => {
            defmt::error!("tcp connect failed: {}", tcp_error_str(e));
            defmt::info!(
                "eth_stats: {:?}",
                defmt::Debug2Format(&eth::eth_stats(&dma))
            );
            defmt::info!("eth_regs: {:?}", defmt::Debug2Format(&eth::eth_regs(&dma)));
            loop {
                idle_or_delay(clocks.sysclk / 100);
            }
        }
    }

    {
        let mut tls_read = [0u8; TLS_READ_LEN];
        let mut tls_write = [0u8; TLS_WRITE_LEN];
        let transport = TcpIo::new(&mut net, tcp_handle, now_ms);
        let mut tls: TlsClient<'_, _, Aes128GcmSha256> =
            TlsClient::new(transport, &mut tls_read, &mut tls_write);

        let seed = cortex_m::peripheral::DWT::cycle_count() as u64 ^ now_ms();
        let mut rng = SoftwareRng::seed_from_u64(seed);

        defmt::info!("TLS handshake with SNI {}", SERVER_NAME);
        tls.connect(SERVER_NAME, &mut rng).expect("tls handshake");
        defmt::info!("TLS session open");

        const REQUEST: &[u8] = b"GET / HTTP/1.1\r\nHost: rugus-test\r\nConnection: close\r\n\r\n";
        tls.write_all(REQUEST).expect("http write");
        tls.flush().expect("tls flush");
        defmt::info!("HTTP request sent");

        let mut response = [0u8; 512];
        let n = tls.read(&mut response).expect("http read");
        if n > 0 {
            if let Ok(text) = core::str::from_utf8(&response[..n]) {
                defmt::info!("HTTP response: {}", text);
            } else {
                defmt::info!("HTTP response: {} bytes (binary)", n);
            }
        } else {
            defmt::warn!("empty HTTP response");
        }
    }

    defmt::info!("https-get complete");
    loop {
        idle_or_delay(clocks.sysclk / 100);
    }
}

fn tcp_error_str(e: TcpError) -> &'static str {
    match e {
        TcpError::Timeout => "timeout",
        TcpError::Closed => "closed",
        TcpError::InvalidState => "invalid state",
        TcpError::WouldBlock => "would block",
    }
}

fn eth_rings() -> (&'static mut [RxRingEntry], &'static mut [TxRingEntry]) {
    unsafe {
        let rx = core::ptr::addr_of_mut!(RX_RING);
        let tx = core::ptr::addr_of_mut!(TX_RING);
        (&mut *rx, &mut *tx)
    }
}

fn init_heap(_dp: &pac::Peripherals, _cp: &mut cortex_m::Peripherals) {
    // Heap on internal SRAM only — SDRAM/FMC bring-up not needed for the
    // current 64 KiB working set (TLS rec buffers + smoltcp). Skipping FMC
    // also keeps the ETH RMII GPIO bank (PG) untouched by the FMC pinmux,
    // which empirically matters on this revision.
    static mut HEAP_FALLBACK: [u8; 64 * 1024] = [0; 64 * 1024];
    let _ = SDRAM_BASE;
    let _ = fmc::SDRAM_SIZE;
    unsafe {
        heap::init(core::ptr::addr_of_mut!(HEAP_FALLBACK).cast(), 64 * 1024);
    }
    defmt::info!("heap: 64 KiB on internal SRAM");
}

fn wait_ipv4(net: &mut NetStack<'_, EthernetDMA<'_, '_>>, cfg: &StaticConfig) {
    defmt::info!(
        "static IPv4 {}.{}.{}.{}/{}",
        cfg.address.octets()[0],
        cfg.address.octets()[1],
        cfg.address.octets()[2],
        cfg.address.octets()[3],
        cfg.prefix_len
    );
    loop {
        net.device_mut().service_dma();
        net.poll(Instant::from_millis(now_ms() as i64));
        if net.ipv4().is_some() {
            defmt::info!("IPv4 ready");
            break;
        }
    }
}

fn setup_systick(syst: &mut cortex_m::peripheral::SYST) {
    syst.set_reload(cortex_m::peripheral::SYST::get_ticks_per_10ms() / 10);
    syst.enable_counter();
    syst.enable_interrupt();
}

fn now_ms() -> u64 {
    cortex_m::interrupt::free(|cs| *TIME_MS.borrow(cs).borrow())
}

fn idle_or_delay(cycles: u32) {
    if take_eth_irq_pending() {
        cortex_m::asm::delay(cycles / 64);
    } else {
        cortex_m::asm::wfi();
    }
}

#[interrupt]
fn ETH() {
    let _ = eth_interrupt_handler();
}

#[exception]
fn SysTick() {
    cortex_m::interrupt::free(|cs| {
        *TIME_MS.borrow(cs).borrow_mut() += 1;
    });
}
