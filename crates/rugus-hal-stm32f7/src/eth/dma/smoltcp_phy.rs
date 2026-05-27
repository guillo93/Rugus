use super::rx::RxRing;
use super::tx::TxRing;
use super::EthernetDMA;

use smoltcp::phy::{Device, DeviceCapabilities, RxToken, TxToken};
use smoltcp::time::Instant;

impl<'rx, 'tx> Device for &mut EthernetDMA<'rx, 'tx> {
    type RxToken<'token>
        = <EthernetDMA<'rx, 'tx> as Device>::RxToken<'token>
    where
        Self: 'token;

    type TxToken<'token>
        = <EthernetDMA<'rx, 'tx> as Device>::TxToken<'token>
    where
        Self: 'token;

    fn receive(&mut self, timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        <EthernetDMA<'rx, 'tx> as Device>::receive(self, timestamp)
    }

    fn transmit(&mut self, timestamp: Instant) -> Option<Self::TxToken<'_>> {
        <EthernetDMA<'rx, 'tx> as Device>::transmit(self, timestamp)
    }

    fn capabilities(&self) -> DeviceCapabilities {
        <EthernetDMA<'rx, 'tx> as Device>::capabilities(self)
    }
}

impl<'rx, 'tx> Device for EthernetDMA<'rx, 'tx> {
    type RxToken<'token>
        = EthRxToken<'token, 'rx>
    where
        Self: 'token;
    type TxToken<'token>
        = EthTxToken<'token, 'tx>
    where
        Self: 'token;

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = super::MTU;
        caps.max_burst_size = Some(1);
        caps
    }

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        // Clear sticky RBUS/TBUS and re-arm demand poll on every smoltcp tick
        // so a stalled DMA recovers without requiring the example main loop to
        // call `service_dma()` manually.
        self.service_dma();
        if !self.rx_available() {
            return None;
        }
        let EthernetDMA {
            rx_ring, tx_ring, ..
        } = self;
        Some((EthRxToken { rx_ring }, EthTxToken { tx_ring }))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        self.service_dma();
        if self.tx_available() {
            let EthernetDMA { tx_ring, .. } = self;
            Some(EthTxToken { tx_ring })
        } else {
            None
        }
    }
}

/// RX token for smoltcp.
pub struct EthRxToken<'a, 'rx> {
    rx_ring: &'a mut RxRing<'rx>,
}

impl<'dma, 'rx> RxToken for EthRxToken<'dma, 'rx> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        match self.rx_ring.recv_next(None) {
            Ok(v) => {
                let result = f(&v);
                v.free();
                result
            }
            Err(super::RxError::WouldBlock) => f(&[]),
            Err(_) => f(&[]),
        }
    }
}

/// TX token for smoltcp.
pub struct EthTxToken<'a, 'tx> {
    tx_ring: &'a mut TxRing<'tx>,
}

impl<'a, 'tx> TxToken for EthTxToken<'a, 'tx> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        // 802.3: minimum frame on the wire is 64 bytes including 4-byte FCS.
        // The MAC pads short frames automatically, but we also reserve a 60-byte
        // descriptor so that the DMA never transmits an undersized frame even
        // if the MAC's pad/CRC engine is misconfigured.
        let padded_len = len.max(60);
        let Some(mut tx_packet) = self.tx_ring.send_next(padded_len, None).ok() else {
            return f(&mut []);
        };
        if padded_len > len {
            tx_packet[len..padded_len].fill(0);
        }
        let res = f(&mut tx_packet[..len]);
        defmt::trace!("eth tx len={} (padded {})", len, padded_len);
        tx_packet.send();
        // Kick the DMA so the descriptor is picked up immediately even if the
        // engine was suspended on TBUS (transmit buffer unavailable).
        self.tx_ring.demand_poll();
        res
    }
}
