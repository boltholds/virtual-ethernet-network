//! Minimal network-domain values. Protocol-specific frames live in protocol crates.

/// Interface endpoint within the virtual network.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct PortId(pub u32);

/// Opaque serialized frame bytes. No MAC or protocol semantics are assumed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub bytes: Vec<u8>,
}

impl Frame {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self { bytes: bytes.into() }
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// Arrival of a complete frame at a receiver port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameArrival {
    pub from: PortId,
    pub to: PortId,
    pub frame: Frame,
}
