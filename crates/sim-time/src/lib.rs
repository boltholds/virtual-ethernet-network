//! Shared simulation time primitives. Independent of wall-clock time.

/// Virtual timestamp, represented as nanoseconds from an arbitrary simulation epoch.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SimInstant(u64);

/// Non-negative virtual duration in nanoseconds.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SimDuration(u64);

impl SimInstant {
    pub const ZERO: Self = Self(0);

    pub const fn from_nanos(value: u64) -> Self { Self(value) }

    pub const fn as_nanos(self) -> u64 { self.0 }

    /// A duration shifts an instant, but adding two instants is not meaningful.
    pub fn checked_add(self, duration: SimDuration) -> Option<Self> {
        self.0.checked_add(duration.0).map(Self)
    }

    pub fn checked_sub_duration(self, duration: SimDuration) -> Option<Self> {
        self.0.checked_sub(duration.0).map(Self)
    }

    /// Subtracting two instants yields a duration.
    pub fn checked_duration_since(self, earlier: Self) -> Option<SimDuration> {
        self.0.checked_sub(earlier.0).map(SimDuration)
    }
}

impl SimDuration {
    pub const ZERO: Self = Self(0);

    pub const fn from_nanos(value: u64) -> Self { Self(value) }

    pub const fn as_nanos(self) -> u64 { self.0 }

    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.0.checked_add(other.0).map(Self)
    }

    pub fn checked_sub(self, other: Self) -> Option<Self> {
        self.0.checked_sub(other.0).map(Self)
    }

    pub fn checked_mul(self, factor: u64) -> Option<Self> {
        self.0.checked_mul(factor).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_instants_and_durations_have_checked_arithmetic() {
        let start = SimInstant::from_nanos(100);
        let interval = SimDuration::from_nanos(20);
        let end = start.checked_add(interval).unwrap();
        assert_eq!(end, SimInstant::from_nanos(120));
        assert_eq!(end.checked_duration_since(start), Some(interval));
        assert_eq!(end.checked_sub_duration(interval), Some(start));
        assert_eq!(interval.checked_add(interval), Some(SimDuration::from_nanos(40)));
        assert_eq!(interval.checked_mul(3), Some(SimDuration::from_nanos(60)));
        assert_eq!(interval.checked_sub(SimDuration::from_nanos(21)), None);
    }

    #[test]
    fn time_is_checked_not_wrapping() {
        assert_eq!(SimInstant::from_nanos(u64::MAX).checked_add(SimDuration::from_nanos(1)), None);
        assert_eq!(SimInstant::from_nanos(1).checked_duration_since(SimInstant::from_nanos(2)), None);
        assert_eq!(SimInstant::ZERO.checked_sub_duration(SimDuration::from_nanos(1)), None);
        assert_eq!(SimDuration::from_nanos(u64::MAX).checked_add(SimDuration::from_nanos(1)), None);
        assert_eq!(SimDuration::from_nanos(u64::MAX).checked_mul(2), None);
    }
}
