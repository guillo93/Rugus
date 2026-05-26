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
| IPv4    | 192.168.1.50   |
| Netmask | 255.255.255.0 (/24) |
| Gateway | 192.168.1.1    |
| MAC     | 02:00:00:00:00:01 |

Adjust `StaticConfig::DISCO_LAN` in `crates/rugus-net/src/stack.rs` if your LAN uses another subnet.

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
INFO  static IPv4 192.168.1.50/24 gw Some(192.168.1.1)
INFO  PHY link up
INFO  IPv4 address 192.168.1.50
```

## Optional: ping from PC

If your PC is on `192.168.1.0/24`, add a host route or set a compatible address, then:

```bash
ping 192.168.1.50
```

## Next steps (G4)

- DHCP client
- `rugus-tls` + HTTPS GET example
