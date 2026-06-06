//! Rugus F5.B.2 — sockets userland por syscall + IPC bajo MPU en STM32F769I-DISCO.
//!
//! Da el segundo paso de F5.B (networking): expone la pila de red a una tarea
//! **userland** (nPRIV, dominio App, sandboxeada por la MPU) mediante un diseño
//! **híbrido**:
//!
//! - **Plano de control** (crear/conectar/cerrar socket) → syscalls finas
//!   `net_socket`/`net_connect`/`net_close` (SVC 0x30/0x31/0x34). El dispatch del
//!   kernel las rutea a los [`NetHooks`] que registra esta placa; el servicio de
//!   red (tarea privilegiada `net_task`) es el ÚNICO que toca smoltcp.
//! - **Plano de datos** (TX de alto volumen) → canal IPC `ChanCb` (id 0) + un
//!   **pool de buffers compartido** mapeado App-RW por la región MPU 5
//!   (`SERVICES`, libre y persistente entre cambios de contexto). La app escribe
//!   el payload en un slot del pool y envía el índice por el canal; el servicio
//!   drena el canal, lee el slot y transmite por smoltcp. Así el grueso de los
//!   datos NO viaja por registros de syscall (cero copias kernel) y la frontera
//!   de confianza se reduce a tres syscalls de control validadas.
//!
//! Topología: IPv4 estática 192.168.0.50/24 (gw .1). La app userland:
//!   1. crea un socket UDP (`net_socket(0)`),
//!   2. lo liga al broadcast 192.168.0.255:9000 (`net_connect`),
//!   3. emite un heartbeat por el pool+canal cada ~1 s — observable con
//!      `sudo tcpdump -i <iface> udp port 9000` desde un PC en la misma LAN;
//!   4. crea un socket TCP cliente (`net_socket(1)`) y conecta a un peer LAN
//!      (192.168.0.112:7777) — si hay un listener (`nc -l 7777`) recibe la línea.
//!
//! Tareas:
//! - kernel  (priv, prioridad Kernel): latido en LD Red, cede el CPU.
//! - net     (priv, prioridad Service): posee la `NetStack`; instala los
//!   `NetHooks`, drena el DMA, hace `poll`, atiende el canal de uplink y empuja
//!   el connect TCP asíncrono. Cede el CPU entre iteraciones.
//! - app     (USER, prioridad App): usa solo syscalls + el pool compartido. No
//!   accede a smoltcp ni a periféricos (la MPU se lo impide).

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

use core::ptr::addr_of_mut;

use ieee802_3_miim::phy::lan87xxa::LAN8742A;
use rugus_arch_cortex_m::{mpu_app_region_for, mpu_region, platform_init, time, MpuLayout};
use rugus_core::sched::Priority;
use rugus_core::syscall::user as svc_user;
use rugus_core::syscall::{self, NetHooks};
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
use smoltcp::socket::{tcp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};

/// Tipos de socket aceptados por `net_socket` (parte del ABI de red).
const KIND_UDP: u32 = 0;
const KIND_TCP: u32 = 1;

/// Canal IPC (id 0 del scheduler) por el que la app envía índices de slot del
/// pool de TX al servicio de red. Mensaje = índice de slot (0..POOL_SLOTS).
const UPLINK_CHAN: u32 = 0;

/// Heartbeat UDP: broadcast de subred 192.168.0.0/24, puerto 9000.
const HB_PORT: u16 = 9000;
const HB_DEST: Ipv4Address = Ipv4Address::new(192, 168, 0, 255);
const UDP_LOCAL_PORT: u16 = 9000;

/// Peer TCP de demostración (un PC de la LAN con `nc -l 7777`). El connect es
/// asíncrono y best-effort: sin listener simplemente no se establece.
const TCP_DEST: Ipv4Address = Ipv4Address::new(192, 168, 0, 112);
const TCP_PORT: u16 = 7777;
const TCP_LOCAL_PORT: u16 = 49152;

const ETH_RING_ENTRIES: usize = 4;

// ---------------------------------------------------------------------------
// Pool de TX compartido (plano de datos). Vive en SRAM y se mapea App-RW por la
// región MPU 5 (SERVICES) para que la app userland pueda ESCRIBIR el payload sin
// pasar por el kernel. El servicio de red lo lee (privilegiado) y transmite. El
// pool es power-of-two (256 B) y está alineado a su tamaño para satisfacer las
// reglas de región MPU de ARMv7-M.
// ---------------------------------------------------------------------------
const POOL_SLOTS: usize = 4;
const SLOT_DATA: usize = 56;

/// Un slot del pool: socket destino (handle), longitud útil y payload. 64 B.
#[repr(C)]
#[derive(Clone, Copy)]
struct Slot {
    handle: u32,
    len: u32,
    data: [u8; SLOT_DATA],
}

impl Slot {
    const EMPTY: Self = Self {
        handle: 0,
        len: 0,
        data: [0; SLOT_DATA],
    };
}

/// Pool completo, alineado a 256 B (= su tamaño) para la región MPU.
#[repr(C, align(256))]
struct Pool {
    slots: [Slot; POOL_SLOTS],
}

static mut TX_POOL: Pool = Pool {
    slots: [Slot::EMPTY; POOL_SLOTS],
};

#[repr(C, align(4096))]
struct Stack4k([u8; 4096]);

static mut STACK_KERNEL: Stack4k = Stack4k([0; 4096]);
static mut STACK_NET: Stack4k = Stack4k([0; 4096]);
static mut STACK_APP: Stack4k = Stack4k([0; 4096]);

#[link_section = ".eth_dma"]
static mut RX_RING: [RxRingEntry; ETH_RING_ENTRIES] = [RxRingEntry::INIT; ETH_RING_ENTRIES];
#[link_section = ".eth_dma"]
static mut TX_RING: [TxRingEntry; ETH_RING_ENTRIES] = [TxRingEntry::INIT; ETH_RING_ENTRIES];

// Estado de red de vida 'static, poseído por la tarea `net`. El DMA vive en su
// propio static; la `NetStack` toma prestada una referencia 'static a él.
static mut ETH_DMA: Option<EthernetDMA<'static, 'static>> = None;
static mut SOCK_STORAGE: [SocketStorage; 4] = [SocketStorage::EMPTY; 4];
static mut UDP_RX_META: [udp::PacketMetadata; 8] = [udp::PacketMetadata::EMPTY; 8];
static mut UDP_RX_PAYLOAD: [u8; 1024] = [0; 1024];
static mut UDP_TX_META: [udp::PacketMetadata; 8] = [udp::PacketMetadata::EMPTY; 8];
static mut UDP_TX_PAYLOAD: [u8; 1024] = [0; 1024];
static mut TCP_RX_BUF: [u8; 1024] = [0; 1024];
static mut TCP_TX_BUF: [u8; 1024] = [0; 1024];
static mut NET: Option<NetStack<'static, EthernetDMA<'static, 'static>>> = None;

/// LED de latido del kernel (LD Red).
static mut HB_LED: Option<LedPin> = None;

// ---------------------------------------------------------------------------
// Tabla de sockets del servicio de red. La indexa el `handle` que devuelve
// `net_socket`. Cada slot referencia un socket smoltcp pre-creado y guarda el
// extremo remoto fijado por `net_connect`. La tocan tanto los hooks (en contexto
// de syscall, privilegiado, cuando corre la app) como `net_task` (cuando corre
// el servicio): nunca a la vez, por el scheduler cooperativo en estos puntos.
// ---------------------------------------------------------------------------
const MAX_SOCKS: usize = 2;

#[derive(Clone, Copy)]
struct SockSlot {
    used: bool,
    kind: u32,
    handle: SocketHandle,
    peer: Ipv4Address,
    port: u16,
    /// TCP: connect ya iniciado (handshake en curso/establecido).
    connecting: bool,
}

static mut SOCKS: [Option<SockSlot>; MAX_SOCKS] = [None; MAX_SOCKS];

#[entry]
fn main() -> ! {
    // DBGMCU: mantiene el reloj de debug vivo en sleep/stop/standby para que el
    // RTT siga emitiendo cuando el scheduler entra en WFI (todas las tareas
    // durmiendo). Sin esto, el WFI apaga el reloj de debug y probe-rs pierde RTT.
    // SAFETY: registro DBGMCU_CR, escritura única en arranque single-thread.
    unsafe {
        core::ptr::write_volatile(0xE004_2004 as *mut u32, 0b111);
    }
    let mut cp = cortex_m::Peripherals::take().expect("cortex-m peripherals");
    let dp = pac::Peripherals::take().expect("device peripherals");

    let clocks = rcc::init(&dp);
    rugus_runtime::enable_cycle_counter(&mut cp);

    defmt::info!(
        "rugus net-userland @ STM32F769I-DISCO, SYSCLK {} MHz, ABI {=u16}",
        clocks.sysclk_mhz(),
        syscall::ABI_VERSION
    );

    // FPU-context + fault handlers + layout MPU de sandbox. Luego la región MPU 6
    // (alta) se reprograma como ETH Non-Cacheable para la coherencia del DMA
    // (gana el solapamiento con KERNEL_RAM). Ver `cache::enable_with_eth_dma`.
    platform_init(&mut cp, &MpuLayout::STM32F769);
    cache::enable_with_eth_dma(&mut cp.SCB, &mut cp.CPUID, &mut cp.MPU);

    // Región MPU 5 (SERVICES, libre): mapea el pool de TX como App-RW para que la
    // tarea userland pueda escribir los payloads. No la toca el context switch
    // (solo programa APP_STACK=4 y STACK_GUARD=7), así que persiste en cada
    // cambio de tarea. Por número alto gana el solapamiento con KERNEL_RAM=2
    // (priv-only), de modo que la app accede SOLO a este pool, no al resto de la
    // RAM del kernel. Atributos cacheables WB: es memoria CPU↔CPU, sin DMA.
    unsafe {
        let base = addr_of_mut!(TX_POOL) as u32;
        let (rbar, rasr) = mpu_app_region_for(base, core::mem::size_of::<Pool>() as u32);
        cp.MPU.rnr.write(mpu_region::SERVICES as u32);
        cp.MPU.rbar.write(rbar);
        cp.MPU.rasr.write(rasr);
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
    }
    defmt::info!(
        "pool de TX compartido mapeado App-RW (region MPU 5, {} slots)",
        POOL_SLOTS
    );

    // Base de tiempo del kernel (SysTick 1 ms): preempción + `Instant` de smoltcp.
    time::init(&mut cp.SYST, clocks.hclk);

    // Bring-up de Ethernet: pines RMII, periférico, MAC+DMA, IRQ y PHY.
    configure_disco_pins(&dp);
    enable_peripheral();
    let parts = PartsIn::new(dp.ETHERNET_MAC, dp.ETHERNET_MMC, dp.ETHERNET_DMA);
    let (rx_ring, tx_ring) = eth_rings();
    let EthStack { mut dma, mac } = eth::init(parts, &clocks, rx_ring, tx_ring).expect("eth init");
    enable_eth_interrupt(&dma);
    defmt::info!("ETH MAC+DMA init OK; MAC {:02x}", DEFAULT_MAC);

    let mut phy = LAN8742A::new(mac, LAN8742_PHY_ADDR);
    init_phy(&mut phy);
    defmt::info!("esperando enlace PHY + autoneg (cable a CN10)...");
    while !link_established(&mut phy) {
        cortex_m::asm::delay(clocks.sysclk / 20);
    }
    sync_mac_speed_from_phy(&mut phy);
    dma.restart_after_link_up();
    defmt::info!("PHY link up (autoneg done)");

    let cfg = StaticConfig::home_lan();
    // SAFETY: arranque single-thread; estos statics solo se inicializan aquí y a
    // partir de `start()` los tocan en exclusiva el servicio de red y sus hooks.
    unsafe {
        ETH_DMA = Some(dma);
        let dma_ref: &'static mut EthernetDMA<'static, 'static> =
            (*addr_of_mut!(ETH_DMA)).as_mut().expect("dma");
        let storage: &'static mut [SocketStorage] = &mut *addr_of_mut!(SOCK_STORAGE);
        let mut net = NetStack::new_static(DEFAULT_MAC, cfg, dma_ref, storage);

        // Pre-crea un socket UDP y uno TCP en almacenamiento 'static; sus handles
        // quedan en la tabla SOCKS (used=false hasta que la app haga net_socket).
        let rx_meta: &mut [udp::PacketMetadata] = &mut *addr_of_mut!(UDP_RX_META);
        let rx_payload: &mut [u8] = &mut *addr_of_mut!(UDP_RX_PAYLOAD);
        let tx_meta: &mut [udp::PacketMetadata] = &mut *addr_of_mut!(UDP_TX_META);
        let tx_payload: &mut [u8] = &mut *addr_of_mut!(UDP_TX_PAYLOAD);
        let udp_rx = udp::PacketBuffer::new(rx_meta, rx_payload);
        let udp_tx = udp::PacketBuffer::new(tx_meta, tx_payload);
        let udp_handle = net.sockets_mut().add(udp::Socket::new(udp_rx, udp_tx));

        let tcp_rx_buf: &mut [u8] = &mut *addr_of_mut!(TCP_RX_BUF);
        let tcp_tx_buf: &mut [u8] = &mut *addr_of_mut!(TCP_TX_BUF);
        let tcp_rx = tcp::SocketBuffer::new(tcp_rx_buf);
        let tcp_tx = tcp::SocketBuffer::new(tcp_tx_buf);
        let tcp_handle = net.sockets_mut().add(tcp::Socket::new(tcp_rx, tcp_tx));

        SOCKS[0] = Some(SockSlot {
            used: false,
            kind: KIND_UDP,
            handle: udp_handle,
            peer: Ipv4Address::UNSPECIFIED,
            port: 0,
            connecting: false,
        });
        SOCKS[1] = Some(SockSlot {
            used: false,
            kind: KIND_TCP,
            handle: tcp_handle,
            peer: Ipv4Address::UNSPECIFIED,
            port: 0,
            connecting: false,
        });
        NET = Some(net);

        HB_LED = Some(LedPin::new(&dp.RCC, DiscoLed::Red));
    }

    defmt::info!(
        "IPv4 estatica {}.{}.{}.{}/{} — UDP hb a broadcast:{}, TCP a {}.{}.{}.{}:{}",
        cfg.address.octets()[0],
        cfg.address.octets()[1],
        cfg.address.octets()[2],
        cfg.address.octets()[3],
        cfg.prefix_len,
        HB_PORT,
        TCP_DEST.octets()[0],
        TCP_DEST.octets()[1],
        TCP_DEST.octets()[2],
        TCP_DEST.octets()[3],
        TCP_PORT
    );

    // SAFETY: arranque single-thread; pilas estáticas vivas para todo el kernel.
    unsafe {
        // Cablea los hooks de syscall del scheduler (yield/sleep/chan/ipc/...).
        rugus_kernel::install(None);
        // Registra el plano de control de red: ahora `net_socket/connect/close`
        // dejan de devolver Enosys y rutean a estos hooks.
        syscall::register_net(NetHooks {
            net_socket: hook_net_socket,
            net_connect: hook_net_connect,
            net_close: hook_net_close,
        });

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
        rugus_kernel::spawn_user(&mut (*addr_of_mut!(STACK_APP)).0, app_task, Priority::App)
            .expect("spawn app");
        defmt::info!("scheduler: 3 tareas (kernel + net + app userland), starting");
        rugus_kernel::start();
    }
}

// ===========================================================================
// Hooks del plano de control de red (corren en contexto de syscall, privilegiado,
// mientras la app está bloqueada en el SVC). Tocan SOCKS + NET; nunca concurren
// con `net_task` (scheduler cooperativo en los puntos de acceso).
// ===========================================================================

/// `net_socket(kind)`: reclama el primer slot libre de ese tipo. Devuelve el
/// handle (índice en SOCKS, ≥0) o `Errno::Ebusy`/`Einval` negativo.
fn hook_net_socket(kind: u32) -> i32 {
    if kind != KIND_UDP && kind != KIND_TCP {
        return rugus_core::Errno::Einval as i32;
    }
    // SAFETY: acceso cooperativo a SOCKS desde el dispatch del syscall.
    unsafe {
        for (i, slot) in SOCKS.iter_mut().enumerate() {
            if let Some(s) = slot {
                if !s.used && s.kind == kind {
                    s.used = true;
                    s.connecting = false;
                    return i as i32;
                }
            }
        }
    }
    rugus_core::Errno::Ebusy as i32
}

/// `net_connect(handle, ip_be, port)`: liga el socket a un remoto. UDP: bind del
/// puerto local + fija destino. TCP: inicia el connect asíncrono.
fn hook_net_connect(handle: u32, ip_be: u32, port: u32) -> i32 {
    let idx = handle as usize;
    let octets = ip_be.to_be_bytes();
    let peer = Ipv4Address::new(octets[0], octets[1], octets[2], octets[3]);
    let port = port as u16;
    // SAFETY: acceso cooperativo a SOCKS/NET desde el dispatch del syscall.
    unsafe {
        let slot = match SOCKS.get_mut(idx).and_then(|s| s.as_mut()) {
            Some(s) if s.used => s,
            _ => return rugus_core::Errno::Einval as i32,
        };
        let net = match (*addr_of_mut!(NET)).as_mut() {
            Some(n) => n,
            None => return rugus_core::Errno::Enosys as i32,
        };
        slot.peer = peer;
        slot.port = port;
        match slot.kind {
            KIND_UDP => {
                let sock = net.sockets_mut().get_mut::<udp::Socket>(slot.handle);
                if !sock.is_open() && sock.bind(UDP_LOCAL_PORT).is_err() {
                    return rugus_core::Errno::Ebusy as i32;
                }
                0
            }
            _ => match net.tcp_connect_start(
                slot.handle,
                IpAddress::Ipv4(peer),
                port,
                TCP_LOCAL_PORT,
            ) {
                Ok(()) => {
                    slot.connecting = true;
                    0
                }
                Err(()) => rugus_core::Errno::Ebusy as i32,
            },
        }
    }
}

/// `net_close(handle)`: libera el slot (y cierra el TCP si aplica).
fn hook_net_close(handle: u32) -> i32 {
    let idx = handle as usize;
    // SAFETY: acceso cooperativo a SOCKS/NET desde el dispatch del syscall.
    unsafe {
        let slot = match SOCKS.get_mut(idx).and_then(|s| s.as_mut()) {
            Some(s) if s.used => s,
            _ => return rugus_core::Errno::Einval as i32,
        };
        if slot.kind == KIND_TCP {
            if let Some(net) = (*addr_of_mut!(NET)).as_mut() {
                net.sockets_mut()
                    .get_mut::<tcp::Socket>(slot.handle)
                    .close();
            }
        }
        slot.used = false;
        slot.connecting = false;
        0
    }
}

// ===========================================================================
// Tareas
// ===========================================================================

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
        // Duerme en vez de ceder en busy-loop: con prioridades preemptivas, una
        // tarea de banda alta que nunca bloquea STARVA a las inferiores. Dormir
        // libera el CPU a `net` (Service) y a `app` (App).
        rugus_kernel::cpu_sleep_ms(250);
    }
}

/// Servicio de red: posee la `NetStack`. Drena el DMA, hace `poll`, atiende el
/// canal de uplink (plano de datos) y transmite por el socket correspondiente.
fn net_task() -> ! {
    loop {
        // SAFETY: `NET`/`SOCKS` los toca esta tarea (y los hooks, no concurrentes).
        unsafe {
            if let Some(net) = (*addr_of_mut!(NET)).as_mut() {
                net.device_mut().service_dma();
                net.poll(Instant::from_millis(time::now_ms() as i64));

                // Drena el canal de uplink: cada mensaje es un índice de slot del
                // pool compartido que la app rellenó. Lo leemos y transmitimos.
                let mut slot_idx = 0u32;
                while rugus_kernel::cpu_chan_recv(UPLINK_CHAN as usize, 0, &mut slot_idx) == 0 {
                    drain_slot(net, slot_idx);
                }

                // Empuja lo recién encolado (UDP/TCP) a la cola de TX del DMA.
                net.poll(Instant::from_millis(time::now_ms() as i64));
            }
        }
        // Sondeo a ~100 Hz cediendo el CPU a la app (banda App, prioridad menor):
        // dormir 10 ms evita la inanición de userland que provocaría un busy-loop
        // de esta tarea Service, y sigue drenando el DMA/atendiendo el canal con
        // holgura para un heartbeat de 1 s.
        rugus_kernel::cpu_sleep_ms(10);
    }
}

/// Lee el slot `idx` del pool compartido y transmite su payload por el socket
/// indicado en el propio slot (UDP a su peer, TCP por el stream establecido).
///
/// # Safety
/// Solo desde `net_task`/hooks (cooperativo); lee el pool App-RW y SOCKS.
unsafe fn drain_slot(net: &mut NetStack<'static, EthernetDMA<'static, 'static>>, idx: u32) {
    if idx as usize >= POOL_SLOTS {
        return;
    }
    // SAFETY: idx acotado; el pool vive en SRAM y solo esta ruta lo lee aquí.
    let slot = unsafe { &(*addr_of_mut!(TX_POOL)).slots[idx as usize] };
    let len = (slot.len as usize).min(SLOT_DATA);
    let payload = &slot.data[..len];
    let handle = slot.handle as usize;
    let sock_slot = match unsafe { SOCKS.get(handle).and_then(|s| s.as_ref()) } {
        Some(s) if s.used => *s,
        _ => return,
    };
    match sock_slot.kind {
        KIND_UDP => {
            let sock = net.sockets_mut().get_mut::<udp::Socket>(sock_slot.handle);
            let ep = IpEndpoint::new(IpAddress::Ipv4(sock_slot.peer), sock_slot.port);
            match sock.send_slice(payload, ep) {
                Ok(()) => defmt::info!(
                    "UDP TX {=usize}B -> {}:{=u16} (slot {=u32})",
                    len,
                    defmt::Debug2Format(&sock_slot.peer),
                    sock_slot.port,
                    idx
                ),
                Err(e) => defmt::warn!("UDP TX err: {}", defmt::Debug2Format(&e)),
            }
        }
        _ => {
            let sock = net.sockets_mut().get_mut::<tcp::Socket>(sock_slot.handle);
            if sock.can_send() {
                match sock.send_slice(payload) {
                    Ok(n) => defmt::info!("TCP TX {=usize}B (slot {=u32})", n, idx),
                    Err(e) => defmt::warn!("TCP TX err: {}", defmt::Debug2Format(&e)),
                }
            } else {
                defmt::debug!("TCP no establecido aun; slot {=u32} descartado", idx);
            }
        }
    }
}

/// Tarea USERLAND (nPRIV): solo usa syscalls + el pool compartido (region MPU 5).
/// No accede a smoltcp ni a periféricos — la MPU dispararía MemManage.
fn app_task() -> ! {
    // Plano de control: crea y liga ambos sockets vía syscall.
    let udp = svc_user::net_socket(KIND_UDP);
    let tcp = svc_user::net_socket(KIND_TCP);
    let _ = svc_user::net_connect(udp as u32, pack_be(HB_DEST), HB_PORT as u32);
    let _ = svc_user::net_connect(tcp as u32, pack_be(TCP_DEST), TCP_PORT as u32);

    let mut seq = 0u32;
    let mut next = 0usize; // round-robin de slots del pool
    let mut spin = 0u32;
    // Cadencia del heartbeat por conteo de cesiones. La app NO duerme (sleep)
    // a propósito: al ser la tarea de menor prioridad y permanecer SIEMPRE lista,
    // evita que el scheduler entre en WFI cuando `net`/`kernel` duermen — así el
    // reloj de debug no se apaga y el RTT sigue vivo para observar el TX. El valor
    // se calibró empíricamente para ~1 emisión/segundo a 216 MHz.
    const SPINS_PER_HB: u32 = 60_000;
    loop {
        spin = spin.wrapping_add(1);
        if spin >= SPINS_PER_HB {
            spin = 0;
            seq = seq.wrapping_add(1);
            // Plano de datos: escribe el heartbeat UDP en un slot del pool y
            // notifica al servicio por el canal (solo el índice viaja por registro).
            if udp >= 0 {
                let slot = next % POOL_SLOTS;
                next = next.wrapping_add(1);
                fill_slot(slot, udp as u32, b"RUGUS-USERLAND hb", seq);
                let _ = svc_user::chan_send(UPLINK_CHAN, slot as u32, 100);
            }
            // Cada 5 latidos intenta una línea TCP (si está establecida, el
            // servicio la transmite; si no, la descarta con un debug).
            if tcp >= 0 && seq % 5 == 0 {
                let slot = next % POOL_SLOTS;
                next = next.wrapping_add(1);
                fill_slot(slot, tcp as u32, b"RUGUS-USERLAND tcp\n", seq);
                let _ = svc_user::chan_send(UPLINK_CHAN, slot as u32, 100);
            }
            let _ = svc_user::checkin();
        }
        let _ = svc_user::yield_now();
    }
}

/// Rellena el slot `idx` del pool compartido (App-RW): destino + payload con el
/// número de secuencia anexado. Lo ejecuta la app userland (escritura directa a
/// la región MPU 5; sin syscall).
fn fill_slot(idx: usize, handle: u32, body: &[u8], seq: u32) {
    // SAFETY: idx < POOL_SLOTS; el pool está mapeado App-RW para esta tarea.
    unsafe {
        let slot = &mut (*addr_of_mut!(TX_POOL)).slots[idx];
        slot.handle = handle;
        let mut n = 0usize;
        for &b in body.iter().take(SLOT_DATA) {
            slot.data[n] = b;
            n += 1;
        }
        // Sufijo " #<seq>" en decimal sencillo, sin formato (no_std, sin alloc).
        for &b in b" #" {
            if n < SLOT_DATA {
                slot.data[n] = b;
                n += 1;
            }
        }
        n += write_u32(&mut slot.data, n, seq);
        slot.len = n as u32;
    }
}

/// Escribe `v` en decimal ASCII en `buf[at..]`; devuelve los bytes escritos.
fn write_u32(buf: &mut [u8; SLOT_DATA], at: usize, v: u32) -> usize {
    let mut tmp = [0u8; 10];
    let mut i = 0;
    let mut x = v;
    loop {
        tmp[i] = b'0' + (x % 10) as u8;
        x /= 10;
        i += 1;
        if x == 0 {
            break;
        }
    }
    let mut written = 0;
    while i > 0 && at + written < SLOT_DATA {
        i -= 1;
        buf[at + written] = tmp[i];
        written += 1;
    }
    written
}

/// Empaqueta una IPv4 en u32 big-endian (orden de red) para `net_connect`.
fn pack_be(ip: Ipv4Address) -> u32 {
    let o = ip.octets();
    u32::from_be_bytes([o[0], o[1], o[2], o[3]])
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
