//! Rugus G4 step 1 — Ethernet link + static IPv4 on STM32F769I-DISCO.

#![no_std]
#![no_main]

use core::cell::RefCell;

use cortex_m::interrupt::Mutex;
use cortex_m_rt::exception;
use ieee802_3_miim::phy::lan87xxa::LAN8742A;
use rugus_hal_stm32f7::cache;
use rugus_hal_stm32f7::eth::{
    self, configure_disco_pins, enable_eth_interrupt, enable_peripheral, eth_interrupt_handler,
    init_phy, link_up, EthStack, PartsIn, RxRingEntry, TxRingEntry, DEFAULT_MAC, LAN8742_PHY_ADDR,
};
use rugus_hal_stm32f7::pac;
use rugus_hal_stm32f7::rcc;
use rugus_net::{NetStack, StaticConfig};
use rugus_runtime::entry;
use smoltcp::iface::SocketStorage;
use smoltcp::time::Instant;
use stm32f7::stm32f7x9::interrupt;

static TIME_MS: Mutex<RefCell<u64>> = Mutex::new(RefCell::new(0));

#[entry]
fn main() -> ! {
    let mut cp = cortex_m::Peripherals::take().expect("cp");
    let dp = pac::Peripherals::take().expect("dp");

    let clocks = rcc::init(&dp);
    cache::enable(&mut cp.SCB, &mut cp.CPUID);
    setup_systick(&mut cp.SYST);

    defmt::info!(
        "rugus eth-link @ STM32F769I-DISCO, SYSCLK {} MHz",
        clocks.sysclk_mhz()
    );

    configure_disco_pins(&dp);
    defmt::debug!("RMII pins configured");
    enable_peripheral();
    defmt::debug!("ETH peripheral enabled");

    let parts = PartsIn::new(dp.ETHERNET_MAC, dp.ETHERNET_MMC, dp.ETHERNET_DMA);

    let mut rx_ring: [RxRingEntry; 4] = Default::default();
    let mut tx_ring: [TxRingEntry; 4] = Default::default();

    let EthStack { mut dma, mac } =
        eth::init(parts, &clocks, &mut rx_ring, &mut tx_ring).expect("eth init");
    defmt::debug!("ETH MAC+DMA init OK");

    enable_eth_interrupt(&dma);

    defmt::info!("MAC {:02x}", DEFAULT_MAC);

    let mut phy = LAN8742A::new(mac, LAN8742_PHY_ADDR);
    init_phy(&mut phy);

    defmt::info!("waiting for PHY link (cable to LAN port)...");
    while !link_up(&mut phy) {
        cortex_m::asm::delay(clocks.sysclk / 20);
    }
    defmt::info!("PHY link up");

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

    loop {
        net.poll(Instant::from_millis(now_ms() as i64));
        cortex_m::asm::delay(clocks.sysclk / 100);
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
