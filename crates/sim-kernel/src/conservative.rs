//! Conservative causal barriers over a shared virtual timestamp.
//! Participants must report the earliest possible externally relevant wakeup.
//! An actor that emits earlier than the announced barrier violates causality.

use crate::resources::{ParticipantId, RegistryError, ResourceRegistry};
use crate::{AdvanceOutcome, TimeGrant, TimedOutput};
use sim_time::SimInstant;
use std::{collections::BTreeMap, error::Error};

pub type ActorError = Box<dyn Error + Send + Sync + 'static>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectedEvent<E> {
    pub target: ParticipantId,
    pub payload: E,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CausalInput<E> {
    pub source: ParticipantId,
    pub at: SimInstant,
    pub payload: E,
}

/// Actors emit directed messages only at their advertised next wakeup.
/// receive() must schedule any caused work at or after the input timestamp.
pub trait CausalParticipant<E> {
    fn current_time(&self) -> SimInstant;
    fn next_wakeup(&self) -> Option<SimInstant>;
    fn advance(&mut self, grant: TimeGrant)
        -> Result<AdvanceOutcome<DirectedEvent<E>>, ActorError>;
    fn receive(&mut self, input: CausalInput<E>) -> Result<(), ActorError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Claims<R> {
    NoExclusiveResources,
    Exclusive(Vec<R>),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeliveryReceipt {
    pub at: SimInstant,
    pub source: ParticipantId,
    pub target: ParticipantId,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunReport {
    pub reached: SimInstant,
    pub deliveries: Vec<DeliveryReceipt>,
    pub barriers: usize,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActorPhase { Advance, Delivery }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CausalError<R> {
    Poisoned,
    TimeRegression,
    DuplicateParticipant(ParticipantId),
    Resource(RegistryError<R>),
    RegistrationTimeMismatch { participant: ParticipantId, found: SimInstant, expected: SimInstant },
    InvalidWakeup { participant: ParticipantId },
    ActorFailure { participant: ParticipantId, phase: ActorPhase },
    InvalidProgress { participant: ParticipantId, reached: SimInstant, required: SimInstant },
    RetroactiveOutput { participant: ParticipantId, at: SimInstant, barrier: SimInstant },
    WakeupMismatch { participant: ParticipantId },
    UnknownRecipient { sender: ParticipantId, recipient: ParticipantId },
    UnknownParticipant(ParticipantId),
    BarrierLimit { at: SimInstant },
}

/// Owns all participants: no mutable actor or topology handle escapes.
/// Resource exclusivity is session-local and is not a process-global lock.
/// A failed run poisons this coordinator: participants may have changed
/// state and no snapshot/rollback contract has yet been implemented.
pub struct ConservativeCoordinator<E, R: Copy + Ord> {
    now: SimInstant,
    actors: BTreeMap<ParticipantId, Box<dyn CausalParticipant<E>>>,
    claimed: BTreeMap<ParticipantId, bool>,
    resources: ResourceRegistry<R>,
    max_barriers: usize,
    poisoned: bool,
}

impl<E, R: Copy + Ord> Default for ConservativeCoordinator<E, R> {
    fn default() -> Self {
        Self {
            now: SimInstant::ZERO,
            actors: BTreeMap::new(),
            claimed: BTreeMap::new(),
            resources: ResourceRegistry::new(),
            max_barriers: 100_000,
            poisoned: false,
        }
    }
}

impl<E, R: Copy + Ord> ConservativeCoordinator<E, R> {
    pub fn new() -> Self { Self::default() }
    pub fn now(&self) -> SimInstant { self.now }
    pub fn participant_count(&self) -> usize { self.actors.len() }
    pub fn owner_of(&self, resource: R) -> Option<ParticipantId> {
        self.resources.owner_of(resource)
    }
    pub fn is_poisoned(&self) -> bool { self.poisoned }
    pub fn set_max_barriers(&mut self, max: usize) { self.max_barriers = max.max(1); }

    pub fn register(
        &mut self, id: ParticipantId, claims: Claims<R>, actor: Box<dyn CausalParticipant<E>>,
    ) -> Result<(), CausalError<R>> {
        if self.poisoned { return Err(CausalError::Poisoned); }
        if self.actors.contains_key(&id) { return Err(CausalError::DuplicateParticipant(id)); }
        let actual = actor.current_time();
        if actual != self.now {
            return Err(CausalError::RegistrationTimeMismatch {
                participant: id, found: actual, expected: self.now,
            });
        }
        if actor.next_wakeup().is_some_and(|t| t < self.now) {
            return Err(CausalError::InvalidWakeup { participant: id });
        }
        let owned = match claims {
            Claims::NoExclusiveResources => false,
            Claims::Exclusive(resources) => {
                self.resources.claim(id, resources).map_err(CausalError::Resource)?;
                true
            }
        };
        self.actors.insert(id, actor);
        self.claimed.insert(id, owned);
        Ok(())
    }

    pub fn unregister(&mut self, id: ParticipantId) -> Result<(), CausalError<R>> {
        if self.poisoned { return Err(CausalError::Poisoned); }
        if self.actors.remove(&id).is_none() {
            return Err(CausalError::UnknownParticipant(id));
        }
        if self.claimed.remove(&id).unwrap_or(false) {
            self.resources.release(id).map_err(CausalError::Resource)?;
        }
        Ok(())
    }

    /// Only inject at the CURRENT global time. Future external signals must
    /// enter through a scheduled participant, never via a backdated input.
    pub fn inject_now(
        &mut self, target: ParticipantId, source: ParticipantId, payload: E,
    ) -> Result<(), CausalError<R>> {
        if self.poisoned { return Err(CausalError::Poisoned); }
        let actor = match self.actors.get_mut(&target) {
            Some(actor) => actor,
            None => return Err(CausalError::UnknownRecipient { sender: source, recipient: target }),
        };
        if actor.receive(CausalInput { source, at: self.now, payload }).is_err() {
            self.poisoned = true;
            return Err(CausalError::ActorFailure { participant: target, phase: ActorPhase::Delivery });
        }
        Ok(())
    }

    fn fail<T>(&mut self, error: CausalError<R>) -> Result<T, CausalError<R>> {
        self.poisoned = true;
        Err(error)
    }

    fn next_barrier(&self, horizon: SimInstant) -> Result<SimInstant, CausalError<R>> {
        let mut barrier = horizon;
        for (&id, actor) in &self.actors {
            if actor.current_time() != self.now {
                return Err(CausalError::RegistrationTimeMismatch {
                    participant: id, found: actor.current_time(), expected: self.now,
                });
            }
            if let Some(next) = actor.next_wakeup() {
                if next < self.now {
                    return Err(CausalError::InvalidWakeup { participant: id });
                }
                barrier = barrier.min(next);
            }
        }
        Ok(barrier)
    }

    /// Lockstep advances all actors to a shared lower-bound barrier.
    /// Delivery order: timestamp, sender id, sender local emission order.
    /// Zero-time reactions are executed as bounded microsteps at that same
    /// instant. An unexpectedly early return/retroactive event poisons the
    /// session instead of silently violating causality.
    pub fn advance_to(&mut self, horizon: SimInstant)
        -> Result<RunReport, CausalError<R>>
    {
        if self.poisoned { return Err(CausalError::Poisoned); }
        if horizon < self.now { return Err(CausalError::TimeRegression); }
        let mut result = RunReport { reached: self.now, deliveries: Vec::new(), barriers: 0 };
        loop {
            if result.barriers >= self.max_barriers {
                return self.fail(CausalError::BarrierLimit { at: self.now });
            }
            let at = match self.next_barrier(horizon) {
                Ok(at) => at,
                Err(error) => return self.fail(error),
            };
            let mut messages: Vec<(ParticipantId, DirectedEvent<E>)> = Vec::new();
            for (&id, actor) in self.actors.iter_mut() {
                let outcome = match actor.advance(TimeGrant { until: at }) {
                    Ok(o) => o,
                    Err(_) => {
                        self.poisoned = true;
                        return Err(CausalError::ActorFailure {
                            participant: id, phase: ActorPhase::Advance,
                        });
                    }
                };
                if outcome.reached != at {
                    self.poisoned = true;
                    return Err(CausalError::InvalidProgress {
                        participant: id, reached: outcome.reached, required: at,
                    });
                }
                for TimedOutput { at: event_at, value } in outcome.outputs {
                    if event_at != at {
                        self.poisoned = true;
                        return Err(CausalError::RetroactiveOutput {
                            participant: id, at: event_at, barrier: at,
                        });
                    }
                    messages.push((id, value));
                }
                if outcome.next_wakeup != actor.next_wakeup() {
                    self.poisoned = true;
                    return Err(CausalError::WakeupMismatch { participant: id });
                }
                if outcome.next_wakeup.is_some_and(|t| t < at) {
                    self.poisoned = true;
                    return Err(CausalError::InvalidWakeup { participant: id });
                }
            }
            // Every actor has now reached the barrier, so delivery is safe.
            self.now = at;
            result.reached = at;
            result.barriers += 1;
            if let Some((source, target)) = messages.iter()
                .find(|(_, message)| !self.actors.contains_key(&message.target))
                .map(|(source, event)| (*source, event.target))
            {
                return self.fail(CausalError::UnknownRecipient { sender: source, recipient: target });
            }
            for (source, message) in messages {
                let target = message.target;
                let input = CausalInput { source, at, payload: message.payload };
                if self.actors.get_mut(&target).expect("prevalidated").receive(input).is_err() {
                    return self.fail(CausalError::ActorFailure {
                        participant: target, phase: ActorPhase::Delivery,
                    });
                }
                result.deliveries.push(DeliveryReceipt { at, source, target });
            }
            if self.now == horizon && !self.actors.values().any(|a| a.next_wakeup() == Some(at)) {
                return Ok(result);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Pulse {
        now: SimInstant,
        pending: bool,
        target: ParticipantId,
    }
    impl CausalParticipant<u8> for Pulse {
        fn current_time(&self) -> SimInstant { self.now }
        fn next_wakeup(&self) -> Option<SimInstant> {
            if self.pending { Some(self.now) } else { None }
        }
        fn advance(&mut self, grant: TimeGrant)
            -> Result<AdvanceOutcome<DirectedEvent<u8>>, ActorError>
        {
            self.now = grant.until;
            let outputs = if self.pending {
                self.pending = false;
                vec![TimedOutput {
                    at: self.now,
                    value: DirectedEvent { target: self.target, payload: 7 },
                }]
            } else { Vec::new() };
            Ok(AdvanceOutcome { reached: self.now, outputs, next_wakeup: self.next_wakeup() })
        }
        fn receive(&mut self, _: CausalInput<u8>) -> Result<(), ActorError> {
            self.pending = true;
            Ok(())
        }
    }

    #[test]
    fn same_time_cycle_is_detected_and_session_poisoned() {
        let mut c = ConservativeCoordinator::<u8, u32>::new();
        c.register(ParticipantId(1), Claims::NoExclusiveResources,
            Box::new(Pulse {
                now: SimInstant::ZERO, pending: true, target: ParticipantId(2),
            })
        ).unwrap();
        c.register(ParticipantId(2), Claims::NoExclusiveResources,
            Box::new(Pulse {
                now: SimInstant::ZERO, pending: false, target: ParticipantId(1),
            })
        ).unwrap();
        c.set_max_barriers(4);
        assert!(matches!(
            c.advance_to(SimInstant::from_nanos(100)),
            Err(CausalError::BarrierLimit { at }) if at == SimInstant::ZERO
        ));
        assert!(c.is_poisoned());
        assert!(matches!(c.advance_to(SimInstant::from_nanos(101)), Err(CausalError::Poisoned)));
    }

    #[test]
    fn exclusive_claims_are_session_scoped() {
        let mut c = ConservativeCoordinator::<u8, u32>::new();
        c.register(ParticipantId(1), Claims::Exclusive(vec![3, 4]),
            Box::new(Pulse {
                now: SimInstant::ZERO, pending: false, target: ParticipantId(1),
            })
        ).unwrap();
        assert!(matches!(
            c.register(ParticipantId(2), Claims::Exclusive(vec![4, 5]),
                Box::new(Pulse {
                    now: SimInstant::ZERO, pending: false, target: ParticipantId(2),
                })
            ),
            Err(CausalError::Resource(RegistryError::ResourceOccupied {
                resource: 4, owner: ParticipantId(1)
            }))
        ));
        assert_eq!(c.owner_of(5), None);
        c.unregister(ParticipantId(1)).unwrap();
        assert_eq!(c.owner_of(3), None);
    }

    struct LyingActor { now: SimInstant }
    impl CausalParticipant<u8> for LyingActor {
        fn current_time(&self) -> SimInstant { self.now }
        fn next_wakeup(&self) -> Option<SimInstant> { None }
        fn advance(&mut self, grant: TimeGrant)
            -> Result<AdvanceOutcome<DirectedEvent<u8>>, ActorError>
        {
            self.now = grant.until;
            Ok(AdvanceOutcome {
                reached: grant.until,
                outputs: vec![TimedOutput {
                    at: SimInstant::from_nanos(1),
                    value: DirectedEvent { target: ParticipantId(1), payload: 0 },
                }],
                next_wakeup: None,
            })
        }
        fn receive(&mut self, _: CausalInput<u8>) -> Result<(), ActorError> { Ok(()) }
    }

    #[test]
    fn retroactive_output_fails_closed() {
        let mut c = ConservativeCoordinator::<u8, u32>::new();
        c.register(ParticipantId(1), Claims::NoExclusiveResources,
            Box::new(LyingActor { now: SimInstant::ZERO })).unwrap();
        assert!(matches!(
            c.advance_to(SimInstant::from_nanos(10)),
            Err(CausalError::RetroactiveOutput { participant: ParticipantId(1), .. })
        ));
        assert!(c.is_poisoned());
    }
}
