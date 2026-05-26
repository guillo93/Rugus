# eth-link-stm32f769-disco

G4 step 1: bring up RMII Ethernet on the STM32F769I-DISCO onboard LAN8742A,
configure a static IPv4 address, and log link + IP over defmt RTT.

## Hardware

- **Board:** STM32F769I-DISCO (STM32F769NIH6)
- **PHY:** LAN8742A @ address 0, RMII
- **Cable:** Ethernet to a LAN switch/router (DHCP server not required)

## Static IP (lab default)

| Field   | Value          |
|---------|----------------|
| IPv4    | 192.168.0.50   |
| Netmask | 255.255.255.0 (/24) |
| Gateway | 192.168.0.1    |
| MAC     | 02:00:52:55:47:01 |

Adjust `StaticConfig::home_lan()` in `crates/rugus-net/src/lib.rs` if your LAN uses another subnet.

## Build & flash

From the example directory (applies `.cargo/config.toml` link scripts):

```bash
cd examples/eth-link-stm32f769-disco
cargo build --release
cargo run --release
```

Dual ST-Link lab: this example sets `PROBE_RS_PROBE` to the F769 probe in `.cargo/config.toml`.
Override if needed:

```bash
PROBE_RS_PROBE=0483:374b:066EFF524853837267102836 cargo run --release
```

Automated verify:

```bash
./tools/verify-eth-link-stm32f769-disco.sh
```

## Expected RTT output

```
INFO  rugus eth-link @ STM32F769I-DISCO, SYSCLK 216 MHz
INFO  MAC 02:00:00:00:00:01
INFO  static IPv4 192.168.0.50/24 gw Some(192.168.0.1)
INFO  PHY link up
INFO  IPv4 address 192.168.0.50
```

## Optional: ping from PC (Windows direct link)

Set the PC NIC to **192.168.0.112/24** (no gateway needed on a direct cable). Flash
`eth-link`, then from Windows:

```cmd
ping 192.168.0.50
```

While pinging, RTT should show `ETH rx=… tx=…` counters incrementing (ARP + echo
reply). Fedora host cannot reach the Windows link; use RTT counters or Windows ping
as the L2/L3 check.

```bash
ping 192.168.0.50
```

## See also

- G4 HTTPS: [`https-get-stm32f769-disco`](../https-get-stm32f769-disco/README.md)
- DHCP: `NetStack::new_dhcp()` in `rugus-net`
