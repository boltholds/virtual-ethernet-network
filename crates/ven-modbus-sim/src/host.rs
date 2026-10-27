//! Session-scoped authoritative BusId ownership for registered Modbus participants.
//!
//! Moves participant executors into the host so the topology cannot be
//! mutated through an outside handle while its claims are live.
//! The host DOES NOT coordinate multi-participant clock advancement.

use crate::{LineError, LineEvent, ModbusBus, SendTiming};
use sim_kernel::{AdvanceOutcome, SimulationParticipant, TimeGrant};
use sim_kernel::resources::{ParticipantId, RegistryError, ResourceRegistry};
use sim_time::{SimDuration, SimInstant};
use std::collections::BTreeMap;
use ven_core::PortId;
use ven_modbus::RtuAdu;
use ven_topology::BusId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostError {
    Registry(RegistryError<BusId>),
    UnknownParticipant { participant: ParticipantId },
    Line(LineError),
}

/// One registry and one transport owner per BusId within this simulation
/// session. All participant access is deliberately restricted to explicit
/// non-topology-mutating operations.
#[derive(Debug, Default)]
pub struct ModbusBusHost {
    ownership: ResourceRegistry<BusId>,
    executors: BTreeMap<ParticipantId, ModbusBus>,
}

impl ModbusBusHost {
    pub fn new() -> Self { Self::default() }

    pub fn participant_count(&self) -> usize { self.executors.len() }

    pub fn owner_of(&self, bus: BusId) -> Option<ParticipantId> {
        self.ownership.owner_of(bus)
    }

    /// Claims all RS-485 buses advertised by a participant *atomically*.
    /// The host takes ownership of the whole ModbusBus only on success.
    /// If registration fails, the supplied bus is dropped (no runner escapes
    /// into this host); callers needing retry should construct a new one.
    pub fn register(
        &mut self, participant: ParticipantId, bus: ModbusBus,
    ) -> Result<(), HostError> {
        self.ownership.claim(participant, bus.claimed_buses())
            .map_err(HostError::Registry)?;
        let old = self.executors.insert(participant, bus);
        debug_assert!(old.is_none());
        Ok(())
    }

    /// Removal drops the executor and frees all its BusIds as one operation.
    /// Call only after the global coordinator stops scheduling the participant.
    pub fn unregister(&mut self, participant: ParticipantId) -> Result<(), HostError> {
        if !self.executors.contains_key(&participant) {
            return Err(HostError::UnknownParticipant { participant });
        }
        let removed = self.executors.remove(&participant).expect("checked participant");
        drop(removed);
        self.ownership.release(participant)
            .map_err(HostError::Registry)?;
        Ok(())
    }

    fn executor(&mut self, participant: ParticipantId) -> Result<&mut ModbusBus, HostError> {
        self.executors.get_mut(&participant).ok_or(HostError::UnknownParticipant { participant })
    }

    pub fn current_time(&self, participant: ParticipantId) -> Result<SimInstant, HostError> {
        self.executors.get(&participant)
            .map(SimulationParticipant::current_time)
            .ok_or(HostError::UnknownParticipant { participant })
    }

    pub fn next_wakeup(&self, participant: ParticipantId) -> Result<Option<SimInstant>, HostError> {
        self.executors.get(&participant)
            .map(SimulationParticipant::next_wakeup)
            .ok_or(HostError::UnknownParticipant { participant })
    }

    pub fn attach_master(
        &mut self, participant: ParticipantId, port: PortId, timeout: SimDuration,
    ) -> Result<(), HostError> {
        self.executor(participant)?.attach_master(port, timeout).map_err(HostError::Line)
    }

    pub fn attach_slave(
        &mut self, participant: ParticipantId, port: PortId, address: u8,
    ) -> Result<(), HostError> {
        self.executor(participant)?.attach_slave(port, address).map_err(HostError::Line)
    }

    pub fn seed_register(
        &mut self, participant: ParticipantId, port: PortId, address: u16, value: u16,
    ) -> Result<(), HostError> {
        self.executor(participant)?.seed_register(port, address, value).map_err(HostError::Line)
    }

    pub fn slave_register(
        &self, participant: ParticipantId, port: PortId, address: u16,
    ) -> Result<Option<u16>, HostError> {
        self.executors.get(&participant)
            .map(|executor| executor.slave_register(port, address))
            .ok_or(HostError::UnknownParticipant { participant })
    }

    pub fn request(
        &mut self, participant: ParticipantId, port: PortId, adu: RtuAdu,
    ) -> Result<SendTiming, HostError> {
        self.executor(participant)?.request(port, adu).map_err(HostError::Line)
    }

    /// Advances *one* registered participant under a grant from the caller.
    /// Not a multi-participant conservative coordinator; it does NOT resolve
    /// cross-participant same-time causality or implement rollback.
    pub fn advance_one(
        &mut self, participant: ParticipantId, grant: TimeGrant,
    ) -> Result<AdvanceOutcome<LineEvent>, HostError> {
        self.executor(participant)?.advance(grant).map_err(HostError::Line)
    }
}
