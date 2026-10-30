use sim_kernel::{AdvanceOutcome, TimeGrant, TimedOutput};
use sim_kernel::conservative::{
    ActorError, CausalError, CausalInput, CausalParticipant, Claims,
    ConservativeCoordinator, DirectedEvent,
};
use sim_kernel::resources::ParticipantId;
use sim_time::SimInstant;
use std::sync::{Arc, Mutex};

struct Source {
    now: SimInstant,
    wake: SimInstant,
    fired: bool,
    target: ParticipantId,
    payload: u8,
    early: bool,
}
impl CausalParticipant<u8> for Source {
    fn current_time(&self) -> SimInstant { self.now }
    fn next_wakeup(&self) -> Option<SimInstant> {
        if self.fired { None } else { Some(self.wake) }
    }
    fn advance(&mut self, grant: TimeGrant)
        -> Result<AdvanceOutcome<DirectedEvent<u8>>, ActorError>
    {
        if self.early {
            self.now = SimInstant::from_nanos(2);
            return Ok(AdvanceOutcome {
                reached: self.now, outputs: Vec::new(), next_wakeup: None,
            });
        }
        self.now = grant.until;
        let mut outputs = Vec::new();
        if !self.fired && self.now == self.wake {
            self.fired = true;
            outputs.push(TimedOutput {
                at: self.now,
                value: DirectedEvent { target: self.target, payload: self.payload },
            });
        }
        Ok(AdvanceOutcome { reached: self.now, outputs, next_wakeup: self.next_wakeup() })
    }
    fn receive(&mut self, _: CausalInput<u8>) -> Result<(), ActorError> {
        Err("sources do not accept input".into())
    }
}
struct Sink {
    now: SimInstant,
    received: Arc<Mutex<Vec<(SimInstant, ParticipantId, u8)>>>,
}
impl CausalParticipant<u8> for Sink {
    fn current_time(&self) -> SimInstant { self.now }
    fn next_wakeup(&self) -> Option<SimInstant> { None }
    fn advance(&mut self, grant: TimeGrant)
        -> Result<AdvanceOutcome<DirectedEvent<u8>>, ActorError>
    {
        self.now = grant.until;
        Ok(AdvanceOutcome { reached: self.now, outputs: Vec::new(), next_wakeup: None })
    }
    fn receive(&mut self, incoming: CausalInput<u8>) -> Result<(), ActorError> {
        self.received.lock().unwrap()
            .push((incoming.at, incoming.source, incoming.payload));
        Ok(())
    }
}

#[test]
fn equal_time_events_are_delivered_in_sorted_sender_order_after_barrier() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut c = ConservativeCoordinator::<u8, u32>::new();
    let sink = ParticipantId(3);
    c.register(sink, Claims::NoExclusiveResources,
        Box::new(Sink { now: SimInstant::ZERO, received: Arc::clone(&log) })).unwrap();
    // Intentionally register source #2 before source #1.
    for id in [2, 1] {
        c.register(ParticipantId(id), Claims::NoExclusiveResources,
            Box::new(Source {
                now: SimInstant::ZERO,
                wake: SimInstant::from_nanos(5),
                fired: false,
                target: sink, payload: id as u8,
                early: false,
            })).unwrap();
    }
    let run = c.advance_to(SimInstant::from_nanos(10)).unwrap();
    assert_eq!(run.reached, SimInstant::from_nanos(10));
    assert_eq!(run.barriers, 2);
    assert_eq!(run.deliveries.len(), 2);
    assert_eq!(run.deliveries[0].source, ParticipantId(1));
    assert_eq!(run.deliveries[1].source, ParticipantId(2));
    assert_eq!(
        log.lock().unwrap().as_slice(),
        &[
            (SimInstant::from_nanos(5), ParticipantId(1), 1),
            (SimInstant::from_nanos(5), ParticipantId(2), 2),
        ]
    );
}

#[test]
fn early_return_breaking_advertised_bound_poisoned() {
    let mut c = ConservativeCoordinator::<u8, u32>::new();
    c.register(ParticipantId(1), Claims::NoExclusiveResources,
        Box::new(Source {
            now: SimInstant::ZERO,
            wake: SimInstant::from_nanos(5),
            fired: false,
            target: ParticipantId(1), payload: 1, early: true,
        })).unwrap();
    assert!(matches!(
        c.advance_to(SimInstant::from_nanos(10)),
        Err(CausalError::InvalidProgress {
            participant: ParticipantId(1),
            reached, required
        }) if reached == SimInstant::from_nanos(2)
            && required == SimInstant::from_nanos(5)
    ));
    assert!(c.is_poisoned());
}

#[test]
fn external_input_is_delivered_at_current_barrier_only() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut c = ConservativeCoordinator::<u8, u32>::new();
    c.register(ParticipantId(1), Claims::NoExclusiveResources,
        Box::new(Sink { now: SimInstant::ZERO, received: Arc::clone(&log) })).unwrap();
    c.advance_to(SimInstant::from_nanos(10)).unwrap();
    c.inject_now(ParticipantId(1), ParticipantId(999), 42).unwrap();
    assert_eq!(
        log.lock().unwrap().as_slice(),
        &[(SimInstant::from_nanos(10), ParticipantId(999), 42)]
    );
    assert!(matches!(
        c.advance_to(SimInstant::from_nanos(9)),
        Err(CausalError::TimeRegression)
    ));
    assert!(!c.is_poisoned()); // incorrect user request did not mutate actors
}

#[test]
fn validate_all_targets_before_any_output_is_delivered() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut c = ConservativeCoordinator::<u8, u32>::new();
    c.register(ParticipantId(3), Claims::NoExclusiveResources,
        Box::new(Sink { now: SimInstant::ZERO, received: Arc::clone(&log) })).unwrap();
    c.register(ParticipantId(1), Claims::NoExclusiveResources,
        Box::new(Source {
            now: SimInstant::ZERO, wake: SimInstant::from_nanos(5),
            fired: false, target: ParticipantId(3), payload: 1, early: false,
        })).unwrap();
    c.register(ParticipantId(2), Claims::NoExclusiveResources,
        Box::new(Source {
            now: SimInstant::ZERO, wake: SimInstant::from_nanos(5),
            fired: false, target: ParticipantId(99), payload: 2, early: false,
        })).unwrap();
    assert!(matches!(
        c.advance_to(SimInstant::from_nanos(10)),
        Err(CausalError::UnknownRecipient {
            sender: ParticipantId(2), recipient: ParticipantId(99),
        })
    ));
    assert!(log.lock().unwrap().is_empty());
    assert!(c.is_poisoned());
}
