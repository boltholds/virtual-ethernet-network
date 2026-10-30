//! Wired media: a point-to-point full-duplex link and an RS-485 shared bus.

pub mod rs485;

use sim_time::{SimDuration, SimInstant};
use ven_core::PortId;

const NANOS_PER_SEC: u128 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkError {
    SameEndpoint,
    ZeroBitrate,
    UnknownDirection,
    EmptyFrame,
    TimeOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Transmission {
    pub starts: SimInstant,
    pub finishes: SimInstant,
    pub arrives: SimInstant,
}

/// Independent TX lanes: full-duplex means A->B and B->A can overlap.
#[derive(Clone, Debug)]
pub struct FullDuplexLink {
    a: PortId,
    b: PortId,
    bitrate_bps: u64,
    propagation: SimDuration,
    a_to_b_free: SimInstant,
    b_to_a_free: SimInstant,
}

impl FullDuplexLink {
    pub fn new(
        a: PortId,
        b: PortId,
        bitrate_bps: u64,
        propagation: SimDuration,
    ) -> Result<Self, LinkError> {
        if a == b {
            return Err(LinkError::SameEndpoint);
        }
        if bitrate_bps == 0 {
            return Err(LinkError::ZeroBitrate);
        }
        Ok(Self {
            a,
            b,
            bitrate_bps,
            propagation,
            a_to_b_free: SimInstant::ZERO,
            b_to_a_free: SimInstant::ZERO,
        })
    }

    pub fn endpoints(&self) -> (PortId, PortId) {
        (self.a, self.b)
    }

    pub fn connects(&self, from: PortId, to: PortId) -> bool {
        (from == self.a && to == self.b) || (from == self.b && to == self.a)
    }

    /// Transmission accounts for the **provided bytes only**. Callers must
    /// supply physical overhead bytes or gaps explicitly in a richer model.
    pub fn transmit(
        &mut self,
        from: PortId,
        to: PortId,
        bytes: usize,
        now: SimInstant,
    ) -> Result<Transmission, LinkError> {
        if bytes == 0 {
            return Err(LinkError::EmptyFrame);
        }

        let lane_free = if from == self.a && to == self.b {
            self.a_to_b_free
        } else if from == self.b && to == self.a {
            self.b_to_a_free
        } else {
            return Err(LinkError::UnknownDirection);
        };

        let bits = (bytes as u128)
            .checked_mul(8)
            .ok_or(LinkError::TimeOverflow)?;
        let numerator = bits
            .checked_mul(NANOS_PER_SEC)
            .ok_or(LinkError::TimeOverflow)?;
        let rate = self.bitrate_bps as u128;
        let duration = numerator / rate + u128::from(numerator % rate != 0);
        let nanos = u64::try_from(duration).map_err(|_| LinkError::TimeOverflow)?;
        let starts = std::cmp::max(now, lane_free);
        let finishes = starts
            .checked_add(SimDuration::from_nanos(nanos))
            .ok_or(LinkError::TimeOverflow)?;
        let arrives = finishes
            .checked_add(self.propagation)
            .ok_or(LinkError::TimeOverflow)?;

        if from == self.a {
            self.a_to_b_free = finishes;
        } else {
            self.b_to_a_free = finishes;
        }
        Ok(Transmission {
            starts,
            finishes,
            arrives,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> FullDuplexLink {
        FullDuplexLink::new(
            PortId(1),
            PortId(2),
            1_000_000,
            SimDuration::from_nanos(20),
        )
        .unwrap()
    }

    #[test]
    fn serializes_frame_and_queues_same_direction() {
        let mut medium = link();
        let a = medium.transmit(PortId(1), PortId(2), 125, SimInstant::ZERO).unwrap();
        let b = medium.transmit(PortId(1), PortId(2), 125, SimInstant::ZERO).unwrap();
        assert_eq!(a.finishes.as_nanos(), 1_000_000);
        assert_eq!(a.arrives.as_nanos(), 1_000_020);
        assert_eq!(b.starts.as_nanos(), 1_000_000);
        assert_eq!(b.arrives.as_nanos(), 2_000_020);
    }

    #[test]
    fn opposite_direction_is_independent() {
        let mut medium = link();
        let a = medium.transmit(PortId(1), PortId(2), 125, SimInstant::ZERO).unwrap();
        let b = medium.transmit(PortId(2), PortId(1), 125, SimInstant::ZERO).unwrap();
        assert_eq!(a.starts, b.starts);
        assert_eq!(a.arrives, b.arrives);
    }

    #[test]
    fn rejects_invalid_link_and_empty_frame() {
        assert!(matches!(
            FullDuplexLink::new(PortId(1), PortId(1), 1, SimDuration::ZERO),
            Err(LinkError::SameEndpoint)
        ));
        assert_eq!(
            link().transmit(PortId(1), PortId(2), 0, SimInstant::ZERO),
            Err(LinkError::EmptyFrame)
        );
    }
}
