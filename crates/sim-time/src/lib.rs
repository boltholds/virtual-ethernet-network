//! Shared simulation time primitives. Independent of wall-clock time.

/// Virtual timestamp, represented as nanoseconds from an arbitrary simulation epoch.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SimInstant(u64);

/// Non-negative virtual duration in nanoseconds.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SimDuration(u64);

impl SimInstant {
    pub const ZERO: Self = Self(0);

    pub const fn from_nanos(value: u64) -> Self {
        Self(value)
    }

    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    pub fn checked_add(self, duration: SimDuration) -> Option<Self> {
        self.0.checked_add(duration.0).map(Self)
    }

    pub fn checked_duration_since(self, earlier: Self) -> Option<SimDuration> {
        self.0.checked_sub(earlier.0).map(SimDuration)
    }
}

impl SimDuration {
    pub const ZERO: Self = Self(0);

    pub const fn from_nanos(value: u64) -> Self {
        Self(value)
    }

    pub const fn as_nanos(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_is_checked_not_wrapping() {
        assert_eq!(
            SimInstant::from_nanos(10).checked_add(SimDuration::from_nanos(5)),
            Some(SimInstant::from_nanos(15))
        );
        assert_eq!(
            SimInstant::from_nanos(u64::MAX).checked_add(SimDuration::from_nanos(1)),
            None
        );
        assert_eq!(
            SimInstant::from_nanos(1).checked_duration_since(SimInstant::from_nanos(2)),
            None
        );
    }
}
