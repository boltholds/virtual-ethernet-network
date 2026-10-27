//! A VEN participant for conservative host-managed virtual time.

use sim_kernel::{AdvanceOutcome, SimulationParticipant, TimeGrant, TimedOutput};
use sim_time::{SimDuration, SimInstant};
use std::collections::BTreeMap;
use std::fmt;
use ven_core::{Frame, FrameArrival, PortId};
use ven_medium::{FullDuplexLink, LinkError, Transmission};
use ven_scheduler::{NetworkScheduler, ScheduleError};
use ven_topology::{InterfaceKind, LinkId, NodeId, Port, Topology, TopologyError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VenError {
    NoLink,
    PortAlreadyConnected,
    Topology(TopologyError),
    Link(LinkError),
    Schedule(ScheduleError),
    TimeRegression,
}

impl fmt::Display for VenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for VenError {}

/// This crate intentionally does not own the global time authority.
#[derive(Debug, Default)]
pub struct VenNetwork {
    now: SimInstant,
    topology: Topology,
    links: BTreeMap<LinkId, FullDuplexLink>,
    scheduler: NetworkScheduler<FrameArrival>,
}

impl VenNetwork {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn topology(&self) -> &Topology {
        &self.topology
    }

    pub fn add_node(&mut self, id: NodeId) -> Result<(), VenError> {
        self.topology.add_node(id).map_err(VenError::Topology)
    }

    pub fn add_port(&mut self, id: PortId, port: Port) -> Result<(), VenError> {
        self.topology.add_port(id, port).map_err(VenError::Topology)
    }

    /// Adds a typed point-to-point Ethernet link. Other media deliberately
    /// require their own executors instead of reusing Ethernet timing.
    pub fn connect_ethernet(
        &mut self,
        id: LinkId,
        a: PortId,
        b: PortId,
        bitrate_bps: u64,
        propagation: SimDuration,
    ) -> Result<(), VenError> {
        let link = FullDuplexLink::new(a, b, bitrate_bps, propagation).map_err(VenError::Link)?;
        // Avoid modifying either inventory or runtime state on rejected links.
        let mut candidate = self.topology.clone();
        candidate.connect(id, a, b).map_err(VenError::Topology)?;
        if candidate.link(id).expect("link just inserted").kind != InterfaceKind::Ethernet {
            return Err(VenError::Topology(TopologyError::InvalidLinkKind));
        }
        self.topology = candidate;
        self.links.insert(id, link);
        Ok(())
    }

    /// Injects already encoded bytes at the current virtual time.
    pub fn send(
        &mut self,
        from: PortId,
        to: PortId,
        frame: Frame,
    ) -> Result<Transmission, VenError> {
        let (id, link) = self.topology.link_for_port(from).ok_or(VenError::NoLink)?;
        if !((link.endpoints.0 == from && link.endpoints.1 == to)
            || (link.endpoints.1 == from && link.endpoints.0 == to))
        {
            return Err(VenError::NoLink);
        }
        let medium = self.links.get_mut(&id).ok_or(VenError::NoLink)?;
        let previous = medium.clone();
        let tx = medium
            .transmit(from, to, frame.len(), self.now)
            .map_err(VenError::Link)?;
        if let Err(error) = self.scheduler.schedule_at(
            tx.arrives,
            FrameArrival { from, to, frame },
        ) {
            *medium = previous;
            return Err(VenError::Schedule(error));
        }
        Ok(tx)
    }
}

impl SimulationParticipant for VenNetwork {
    type Output = FrameArrival;
    type Error = VenError;

    fn current_time(&self) -> SimInstant {
        self.now
    }

    fn next_wakeup(&self) -> Option<SimInstant> {
        self.scheduler.next_time()
    }

    fn advance(&mut self, grant: TimeGrant) -> Result<AdvanceOutcome<Self::Output>, Self::Error> {
        if grant.until < self.now {
            return Err(VenError::TimeRegression);
        }

        if let Some(at) = self.scheduler.next_time().filter(|t| *t <= grant.until) {
            let mut outputs = Vec::new();
            while let Some(event) = self.scheduler.pop_through(at) {
                outputs.push(TimedOutput { at: event.at, value: event.event });
            }
            self.now = at;
            return Ok(AdvanceOutcome {
                reached: at,
                outputs,
                next_wakeup: self.scheduler.next_time(),
            });
        }

        self.now = grant.until;
        Ok(AdvanceOutcome {
            reached: self.now,
            outputs: Vec::new(),
            next_wakeup: self.scheduler.next_time(),
        })
    }
}
