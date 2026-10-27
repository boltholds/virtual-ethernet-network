//! Minimal reusable simulation participant/time grant contract.
//! This is not yet a multi-participant conservative or optimistic orchestrator.

pub mod resources;

use sim_time::SimInstant;
use std::fmt;

/// Upper bound on how far one participant may advance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeGrant {
    pub until: SimInstant,
}

/// Externally observable event emitted at its virtual timestamp.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimedOutput<T> {
    pub at: SimInstant,
    pub value: T,
}

/// A participant may stop early to publish an event before the granted horizon.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdvanceOutcome<T> {
    pub reached: SimInstant,
    pub outputs: Vec<TimedOutput<T>>,
    pub next_wakeup: Option<SimInstant>,
}

pub trait SimulationParticipant {
    type Output;
    type Error;

    fn current_time(&self) -> SimInstant;
    fn next_wakeup(&self) -> Option<SimInstant>;
    fn advance(&mut self, grant: TimeGrant) -> Result<AdvanceOutcome<Self::Output>, Self::Error>;
}

/// Minimal single-participant time authority and result validator.
#[derive(Clone, Copy, Debug, Default)]
pub struct TimeCoordinator {
    now: SimInstant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeError {
    TimeRegression,
    PastOrFutureOutput,
    BeyondGrant,
    InvalidWakeup,
}

impl fmt::Display for TimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for TimeError {}

impl TimeCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn now(&self) -> SimInstant {
        self.now
    }

    pub fn grant(&self, until: SimInstant) -> Result<TimeGrant, TimeError> {
        if until < self.now {
            return Err(TimeError::TimeRegression);
        }
        Ok(TimeGrant { until })
    }

    /// Checks a participant's reported progress before committing time.
    pub fn commit<T>(
        &mut self,
        grant: TimeGrant,
        outcome: &AdvanceOutcome<T>,
    ) -> Result<(), TimeError> {
        if outcome.reached < self.now {
            return Err(TimeError::TimeRegression);
        }
        if outcome.reached > grant.until {
            return Err(TimeError::BeyondGrant);
        }
        if outcome
            .outputs
            .iter()
            .any(|event| event.at < self.now || event.at > outcome.reached)
        {
            return Err(TimeError::PastOrFutureOutput);
        }
        if outcome
            .next_wakeup
            .is_some_and(|next| next < outcome.reached)
        {
            return Err(TimeError::InvalidWakeup);
        }
        self.now = outcome.reached;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_early_return_and_rejects_future_output() {
        let mut clock = TimeCoordinator::new();
        let grant = clock.grant(SimInstant::from_nanos(10)).unwrap();
        let valid = AdvanceOutcome {
            reached: SimInstant::from_nanos(5),
            outputs: vec![TimedOutput {
                at: SimInstant::from_nanos(5),
                value: (),
            }],
            next_wakeup: Some(SimInstant::from_nanos(7)),
        };
        clock.commit(grant, &valid).unwrap();
        assert_eq!(clock.now(), SimInstant::from_nanos(5));
        assert_eq!(
            clock.grant(SimInstant::from_nanos(4)),
            Err(TimeError::TimeRegression)
        );
        let invalid = AdvanceOutcome {
            reached: SimInstant::from_nanos(6),
            outputs: vec![TimedOutput {
                at: SimInstant::from_nanos(8),
                value: (),
            }],
            next_wakeup: None,
        };
        assert_eq!(
            clock.commit(grant, &invalid),
            Err(TimeError::PastOrFutureOutput)
        );
        assert_eq!(clock.now(), SimInstant::from_nanos(5));
    }
}
