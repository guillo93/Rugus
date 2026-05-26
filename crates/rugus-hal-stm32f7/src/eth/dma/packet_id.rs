/// Optional packet metadata ID (PTP not enabled in Rugus G4 step 1).
#[derive(Debug, PartialEq, Clone, Copy, Default)]
pub struct PacketId(pub u32);

impl PacketId {
    /// Initial value for `Option<PacketId>`.
    pub const INIT: Option<Self> = None;
}

impl From<u32> for PacketId {
    fn from(value: u32) -> Self {
        Self(value)
    }
}
