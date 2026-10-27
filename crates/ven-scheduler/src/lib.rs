//! Network-local ordered event queue; no independent simulation clock.

use sim_time::SimInstant;
use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct EventToken(u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduledEvent<E> {
    pub at: SimInstant,
    pub token: EventToken,
    pub event: E,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduleError {
    TokenExhausted,
}

/// Stable insertion order for events with equal timestamps is an implementation
/// tie-breaker, NOT a model of physical simultaneity or PLC task priorities.
#[derive(Debug)]
pub struct NetworkScheduler<E> {
    pending: BTreeMap<(SimInstant, u64), E>,
    by_token: HashMap<EventToken, (SimInstant, u64)>,
    next_token: u64,
}

impl<E> Default for NetworkScheduler<E> {
    fn default() -> Self {
        Self {
            pending: BTreeMap::new(),
            by_token: HashMap::new(),
            next_token: 0,
        }
    }
}

impl<E> NetworkScheduler<E> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn next_time(&self) -> Option<SimInstant> {
        self.pending.first_key_value().map(|(key, _)| key.0)
    }

    pub fn schedule_at(
        &mut self,
        at: SimInstant,
        event: E,
    ) -> Result<EventToken, ScheduleError> {
        let id = self.next_token;
        self.next_token = id.checked_add(1).ok_or(ScheduleError::TokenExhausted)?;
        let token = EventToken(id);
        let key = (at, id);
        self.pending.insert(key, event);
        self.by_token.insert(token, key);
        Ok(token)
    }

    pub fn cancel(&mut self, token: EventToken) -> bool {
        if let Some(key) = self.by_token.remove(&token) {
            self.pending.remove(&key);
            true
        } else {
            false
        }
    }

    /// Removes the first event only if it lies within the requested horizon.
    pub fn pop_through(&mut self, until: SimInstant) -> Option<ScheduledEvent<E>> {
        let (&(at, id), _) = self.pending.first_key_value()?;
        if at > until {
            return None;
        }
        let (_, event) = self.pending.pop_first()?;
        let token = EventToken(id);
        self.by_token.remove(&token);
        Some(ScheduledEvent { at, token, event })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_times_are_stable_and_future_events_are_held() {
        let mut q = NetworkScheduler::new();
        let at = SimInstant::from_nanos(12);
        q.schedule_at(at, "first").unwrap();
        q.schedule_at(at, "second").unwrap();
        q.schedule_at(SimInstant::from_nanos(20), "future").unwrap();
        assert!(q.pop_through(SimInstant::from_nanos(11)).is_none());
        assert_eq!(q.pop_through(at).unwrap().event, "first");
        assert_eq!(q.pop_through(at).unwrap().event, "second");
        assert!(q.pop_through(at).is_none());
        assert_eq!(q.pop_through(SimInstant::from_nanos(20)).unwrap().event, "future");
        assert!(q.is_empty());
    }

    #[test]
    fn cancellation_removes_pending_event() {
        let mut q = NetworkScheduler::new();
        let token = q.schedule_at(SimInstant::from_nanos(4), ()).unwrap();
        assert_eq!(q.len(), 1);
        assert!(q.cancel(token));
        assert!(!q.cancel(token));
        assert!(q.is_empty());
    }
}
