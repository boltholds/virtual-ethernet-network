//! Session-scoped exclusive ownership of simulation resources.
//!
//! This registry owns *names*, not execution time. One registry belongs to
//! one simulation session; resource keys must be globally unique inside it.
//! No process-wide singleton, hidden wall-clock time, or implicit unlocking.

use std::collections::{BTreeMap, BTreeSet};

/// Stable, typed identity of a registered simulation participant.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ParticipantId(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryError<R> {
    AlreadyRegistered { participant: ParticipantId },
    EmptyClaim,
    RepeatedResource { resource: R },
    ResourceOccupied { resource: R, owner: ParticipantId },
    UnknownParticipant { participant: ParticipantId },
}

#[derive(Debug)]
pub struct ResourceRegistry<R: Copy + Ord> {
    owners: BTreeMap<R, ParticipantId>,
    by_participant: BTreeMap<ParticipantId, BTreeSet<R>>,
}

impl<R: Copy + Ord> Default for ResourceRegistry<R> {
    fn default() -> Self {
        Self { owners: BTreeMap::new(), by_participant: BTreeMap::new() }
    }
}

impl<R: Copy + Ord> ResourceRegistry<R> {
    pub fn new() -> Self { Self::default() }

    pub fn participant_count(&self) -> usize { self.by_participant.len() }
    pub fn resource_count(&self) -> usize { self.owners.len() }
    pub fn owner_of(&self, resource: R) -> Option<ParticipantId> {
        self.owners.get(&resource).copied()
    }
    pub fn resources_of(&self, participant: ParticipantId) -> Option<&BTreeSet<R>> {
        self.by_participant.get(&participant)
    }

    /// Atomic reservation of *all* resources. No state is changed on error.
    /// A participant cannot claim the same resource twice or register twice.
    pub fn claim<I>(&mut self, participant: ParticipantId, resources: I)
        -> Result<(), RegistryError<R>>
    where
        I: IntoIterator<Item = R>,
    {
        if self.by_participant.contains_key(&participant) {
            return Err(RegistryError::AlreadyRegistered { participant });
        }
        let mut new = BTreeSet::new();
        for resource in resources {
            if !new.insert(resource) {
                return Err(RegistryError::RepeatedResource { resource });
            }
        }
        if new.is_empty() { return Err(RegistryError::EmptyClaim); }
        for &resource in &new {
            if let Some(&owner) = self.owners.get(&resource) {
                return Err(RegistryError::ResourceOccupied { resource, owner });
            }
        }
        for &resource in &new {
            self.owners.insert(resource, participant);
        }
        self.by_participant.insert(participant, new);
        Ok(())
    }

    /// Return resources to the session when the participant is unregistered.
    /// The caller must stop driving the participant before release.
    pub fn release(&mut self, participant: ParticipantId)
        -> Result<BTreeSet<R>, RegistryError<R>>
    {
        let owned = self.by_participant.remove(&participant)
            .ok_or(RegistryError::UnknownParticipant { participant })?;
        for resource in &owned {
            let previous = self.owners.remove(resource);
            debug_assert_eq!(previous, Some(participant));
        }
        Ok(owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_are_atomic_for_multiple_resources() {
        let mut registry = ResourceRegistry::<u32>::new();
        registry.claim(ParticipantId(1), [7, 8]).unwrap();
        assert_eq!(
            registry.claim(ParticipantId(2), [9, 8, 10]),
            Err(RegistryError::ResourceOccupied {
                resource: 8, owner: ParticipantId(1),
            })
        );
        assert_eq!(registry.owner_of(9), None);
        assert_eq!(registry.owner_of(10), None);
        assert_eq!(registry.participant_count(), 1);
    }

    #[test]
    fn forbids_duplicate_owners_duplicate_ids_and_empty_claims() {
        let mut registry = ResourceRegistry::<u32>::new();
        assert_eq!(registry.claim(ParticipantId(1), []), Err(RegistryError::EmptyClaim));
        assert_eq!(registry.claim(ParticipantId(1), [5, 5]),
            Err(RegistryError::RepeatedResource { resource: 5 }));
        registry.claim(ParticipantId(1), [5]).unwrap();
        assert_eq!(registry.claim(ParticipantId(1), [6]),
            Err(RegistryError::AlreadyRegistered { participant: ParticipantId(1) }));
        assert_eq!(registry.claim(ParticipantId(2), [5]),
            Err(RegistryError::ResourceOccupied {
                resource: 5, owner: ParticipantId(1),
            }));
        assert_eq!(registry.owner_of(6), None);
    }

    #[test]
    fn release_is_explicit_and_reacquisition_safe() {
        let mut registry = ResourceRegistry::<u32>::new();
        registry.claim(ParticipantId(10), [3, 4]).unwrap();
        assert_eq!(registry.release(ParticipantId(10)).unwrap(), BTreeSet::from([3, 4]));
        assert_eq!(registry.release(ParticipantId(10)),
            Err(RegistryError::UnknownParticipant { participant: ParticipantId(10) }));
        registry.claim(ParticipantId(20), [3, 4]).unwrap();
        assert_eq!(registry.owner_of(3), Some(ParticipantId(20)));
    }

    #[test]
    fn one_session_does_not_claim_a_process_global_resource() {
        let mut a = ResourceRegistry::<u32>::new();
        let mut b = ResourceRegistry::<u32>::new();
        a.claim(ParticipantId(1), [5]).unwrap();
        b.claim(ParticipantId(1), [5]).unwrap();
        assert_eq!(a.resource_count(), 1);
        assert_eq!(b.resource_count(), 1);
    }
}
