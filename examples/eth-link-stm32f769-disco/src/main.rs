//! Rugus G4 step 1 — Ethernet link + static IPv4 on STM32F769I-DISCO.

#![no_std]
#![no_main]

use core::cell::RefCell;

use cortex_m::interrupt::Mutex;
use cortex_m_rt::exception;
use ieee802_3_miim::phy::lan87xxa::LAN8742A;
use ieee802_3_miim::Phy;
use rugus_hal_stm32f7::cache;
use rugus_hal_stm32f7::eth::{
    self, configure_disco_pins, enable_eth_interrupt, enable_peripheral, eth_interrupt_handler,
    eth_regs, eth_stats, init_phy, link_established, sync_mac_speed_from_phy, EthStack, PartsIn,
    RxRingEntry, TxRingEntry, DEFAULT_MAC, LAN8742_PHY_ADDR,
};
use rugus_hal_stm32f7::pac;
use rugus_hal_stm32f7::rcc;
use rugus_net::{NetStack, StaticConfig};
use rugus_runtime::entry;
use smoltcp::iface::SocketStorage;
use smoltcp::time::Instant;
use stm32f7::stm32f7x9::interrupt;

const ETH_RING_ENTRIES: usize = 4;

#[link_section = ".eth_dma"]
static mut RX_RING: [RxRingEntry; ETH_RING_ENTRIES] = [RxRingEntry::INIT; ETH_RING_ENTRIES];

#[link_section = ".eth_dma"]
static mut TX_RING: [TxRingEntry; ETH_RING_ENTRIES] = [TxRingEntry::INIT; ETH_RING_ENTRIES];

static TIME_MS: Mutex<RefCell<u64>> = Mutex::new(RefCell::new(0));

#[entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("cp");
    let dp = pac::Peripherals::take().expect("dp");

    let clocks = rcc::init(&dp);
    cache::enable_with_eth_dma(&mut cp.SCB, &mut cp.CPUID, &mut cp.MPU);
    setup_systick(&mut cp.SYST, clocks.hclk);

    defmt::info!(
        "rugus eth-link @ STM32F769I-DISCO, SYSCLK {} MHz",
        clocks.sysclk_mhz()
    );

    configure_disco_pins(&dp);
    defmt::debug!("RMII pins configured");
    enable_peripheral();
    defmt::debug!("ETH peripheral enabled");

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

    let regs = eth_regs(&dma);
    defmt::info!(
        "ETH regs maccr={:08x} dmabmr={:08x} dmasr={:08x} dmaomr={:08x} mmc_rx={} mmc_tx={}",
        regs.maccr,
        regs.dmabmr,
        regs.dmasr,
        regs.dmaomr,
        regs.mmc_rx_unicast,
        regs.mmc_tx_good
    );
    let phy_bmsr = phy.read(1);
    defmt::info!(
        "PHY BMSR={:04x} link_bit={}",
        phy_bmsr,
        phy_bmsr & 0x0004 != 0
    );

    defmt::debug!(
        "ETH DMA restarted sr={} st={} rps={} tps={}",
        eth_stats(&dma).rx_dma_enabled,
        eth_stats(&dma).tx_dma_enabled,
        eth_stats(&dma).rx_dma_state,
        eth_stats(&dma).tx_dma_state
    );

    let cfg = StaticConfig::home_lan();
    let mut socket_storage: [SocketStorage; 1] = Default::default();
    let mut net = NetStack::new_static(DEFAULT_MAC, cfg, &mut dma, &mut socket_storage);

    defmt::info!(
        "static IPv4 {}.{}.{}.{}/{}",
        cfg.address.octets()[0],
        cfg.address.octets()[1],
        cfg.address.octets()[2],
        cfg.address.octets()[3],
        cfg.prefix_len
    );

    loop {
        net.poll(Instant::from_millis(now_ms() as i64));
        if let Some(ip) = net.ipv4() {
            defmt::info!(
                "IPv4 address {}.{}.{}.{}",
                ip.octets()[0],
                ip.octets()[1],
                ip.octets()[2],
                ip.octets()[3]
            );
            break;
        }
        cortex_m::asm::delay(clocks.sysclk / 50);
    }

    let mut last_log_ms = now_ms();
    let mut last_rx = 0u32;
    let traffic_start = now_ms();
    loop {
        net.device_mut().service_dma();
        net.poll(Instant::from_millis(now_ms() as i64));

        let t = now_ms();
        if t.saturating_sub(last_log_ms) >= 1000 {
            last_log_ms = t;
            let stats = eth_stats(net.device_mut());
            let in_window = t.saturating_sub(traffic_start) < 30_000;
            if stats.rx_frames != last_rx {
                defmt::info!(
                    "ETH rx={} tx={} sr={} st={} rps={} tps={} rbus={} tbus={}",
                    stats.rx_frames,
                    stats.tx_frames,
                    stats.rx_dma_enabled,
                    stats.tx_dma_enabled,
                    stats.rx_dma_state,
                    stats.tx_dma_state,
                    stats.rx_buf_unavail,
                    stats.tx_buf_unavail
                );
                last_rx = stats.rx_frames;
            } else if in_window {
                defmt::info!(
                    "ETH idle rx={} tx={} rps={} tps={} (ping 192.168.0.50 now)",
                    stats.rx_frames,
                    stats.tx_frames,
                    stats.rx_dma_state,
                    stats.tx_dma_state
                );
            } else {
                defmt::debug!(
                    "ETH idle rx={} tx={} sr={} st={} rps={} tps={}",
                    stats.rx_frames,
                    stats.tx_frames,
                    stats.rx_dma_enabled,
                    stats.tx_dma_enabled,
                    stats.rx_dma_state,
                    stats.tx_dma_state
                );
            }
        }

        cortex_m::asm::delay(clocks.sysclk / 100);
    }
}

fn eth_rings() -> (&'static mut [RxRingEntry], &'static mut [TxRingEntry]) {
    unsafe {
        let rx = core::ptr::addr_of_mut!(RX_RING);
        let tx = core::ptr::addr_of_mut!(TX_RING);
        (&mut *rx, &mut *tx)
    }
}

fn setup_systick(syst: &mut cortex_m::peripheral::SYST, hclk_hz: u32) {
    // Tick de 1 ms derivado del reloj del core (HCLK), no del registro de
    // calibración CALIB: en el STM32F7 ese valor no es fiable y dejaba `now_ms`
    // corriendo ~13x rápido, encogiendo todos los timeouts de smoltcp (ARP,
    // retransmisión TCP, DHCP). Fijamos explícitamente la fuente al reloj del
    // procesador y derivamos el reload de la frecuencia conocida.
    syst.set_clock_source(cortex_m::peripheral::syst::SystClkSource::Core);
    syst.set_reload(hclk_hz / 1000 - 1);
    syst.clear_current();
    syst.enable_counter();
    syst.enable_interrupt();
}

fn now_ms() -> u64 {
    cortex_m::interrupt::free(|cs| *TIME_MS.borrow(cs).borrow())
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
