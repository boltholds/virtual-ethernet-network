//! VEN participant: host-granted time, point-to-point Ethernet and shared RS-485.

use sim_kernel::{AdvanceOutcome, SimulationParticipant, TimeGrant, TimedOutput};
use sim_time::{SimDuration, SimInstant};
use std::collections::BTreeMap;
use std::fmt;
use ven_core::{Frame, FrameArrival, PortId};
use ven_medium::{FullDuplexLink, LinkError, Transmission};
use ven_medium::rs485::{Rs485Error, Rs485Medium, Rs485TransmissionWindow, UartFormat};
use ven_scheduler::{NetworkScheduler, ScheduleError};
use ven_topology::{BusId, InterfaceKind, LinkId, NodeId, Port, Topology, TopologyError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VenError {
    NoLink,
    MissingBus,
    TransmitterBusy,
    TransmissionIdExhausted,
    Topology(TopologyError),
    Link(LinkError),
    Rs485(Rs485Error),
    Schedule(ScheduleError),
    TimeRegression,
}

impl fmt::Display for VenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for VenError {}

/// A contention report is simulator diagnostic evidence, NOT evidence that
/// physical RS-485 transceivers necessarily detect collisions themselves.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NetworkOutput {
    Frame(FrameArrival),
    Rs485Contention { bus: BusId, sender: PortId },
}

#[derive(Clone, Debug)]
enum LocalEvent {
    Ethernet(FrameArrival),
    Rs485Complete(u64),
}

#[derive(Clone, Debug)]
struct Rs485InFlight {
    bus: BusId,
    sender: PortId,
    starts: SimInstant,
    finishes: SimInstant,
    frame: Frame,
    contended: bool,
}

/// This participant never advances the coordinator's global time.
#[derive(Debug, Default)]
pub struct VenNetwork {
    now: SimInstant,
    topology: Topology,
    links: BTreeMap<LinkId, FullDuplexLink>,
    buses: BTreeMap<BusId, Rs485Medium>,
    in_flight: BTreeMap<u64, Rs485InFlight>,
    next_transmission_id: u64,
    scheduler: NetworkScheduler<LocalEvent>,
}

impl VenNetwork {
    pub fn new() -> Self { Self::default() }

    pub fn topology(&self) -> &Topology { &self.topology }

    pub fn add_node(&mut self, id: NodeId) -> Result<(), VenError> {
        self.topology.add_node(id).map_err(VenError::Topology)
    }

    pub fn add_port(&mut self, id: PortId, port: Port) -> Result<(), VenError> {
        self.topology.add_port(id, port).map_err(VenError::Topology)
    }

    pub fn connect_ethernet(
        &mut self, id: LinkId, a: PortId, b: PortId,
        bitrate_bps: u64, propagation: SimDuration,
    ) -> Result<(), VenError> {
        let link = FullDuplexLink::new(a, b, bitrate_bps, propagation).map_err(VenError::Link)?;
        let mut candidate = self.topology.clone();
        candidate.connect(id, a, b).map_err(VenError::Topology)?;
        if candidate.link(id).expect("just inserted").kind != InterfaceKind::Ethernet {
            return Err(VenError::Topology(TopologyError::InvalidLinkKind));
        }
        self.topology = candidate;
        self.links.insert(id, link);
        Ok(())
    }

    /// Creates a separate shared medium rather than a point-to-point link.
    pub fn create_rs485_bus(
        &mut self, bus: BusId, baud_rate: u64,
        uart: UartFormat, propagation: SimDuration,
    ) -> Result<(), VenError> {
        let medium = Rs485Medium::new(baud_rate, uart, propagation).map_err(VenError::Rs485)?;
        self.topology.create_rs485_bus(bus).map_err(VenError::Topology)?;
        self.buses.insert(bus, medium);
        Ok(())
    }

    pub fn attach_rs485_port(&mut self, bus: BusId, port: PortId) -> Result<(), VenError> {
        self.topology.attach_bus(bus, port).map_err(VenError::Topology)
    }

    /// Transmit an opaque frame directly to a wired Ethernet link neighbor.
    pub fn send(&mut self, from: PortId, to: PortId, frame: Frame)
        -> Result<Transmission, VenError>
    {
        let (id, link) = self.topology.link_for_port(from).ok_or(VenError::NoLink)?;
        if !((link.endpoints.0 == from && link.endpoints.1 == to)
            || (link.endpoints.1 == from && link.endpoints.0 == to))
        {
            return Err(VenError::NoLink);
        }
        let medium = self.links.get_mut(&id).ok_or(VenError::NoLink)?;
        let previous = medium.clone();
        let tx = medium.transmit(from, to, frame.len(), self.now).map_err(VenError::Link)?;
        if let Err(error) = self.scheduler.schedule_at(
            tx.arrives, LocalEvent::Ethernet(FrameArrival { from, to, frame })
        ) {
            *medium = previous;
            return Err(VenError::Schedule(error));
        }
        Ok(tx)
    }

    /// Starts transmitting immediately at the current virtual instant.
    /// The medium does not silently queue competing drivers; the overlap is
    /// observable as a contention report. There is no automatic retransmission.
    pub fn send_rs485(
        &mut self, from: PortId, frame: Frame,
    ) -> Result<Rs485TransmissionWindow, VenError> {
        let (bus, _) = self.topology.bus_for_port(from).ok_or(VenError::MissingBus)?;
        let medium = self.buses.get(&bus).ok_or(VenError::MissingBus)?;
        let window = medium.transmission(frame.len(), self.now).map_err(VenError::Rs485)?;

        if self.in_flight.values().any(|tx| tx.bus == bus
            && tx.sender == from && tx.finishes > self.now)
        {
            return Err(VenError::TransmitterBusy);
        }

        let txid = self.next_transmission_id;
        let next = txid.checked_add(1).ok_or(VenError::TransmissionIdExhausted)?;

        // Reserve in the queue before changing the in-flight state.
        self.scheduler.schedule_at(window.arrives, LocalEvent::Rs485Complete(txid))
            .map_err(VenError::Schedule)?;
        self.next_transmission_id = next;

        let mut contended = false;
        for existing in self.in_flight.values_mut() {
            if existing.bus == bus
                && existing.starts < window.finishes
                && self.now < existing.finishes
            {
                existing.contended = true;
                contended = true;
            }
        }
        self.in_flight.insert(txid, Rs485InFlight {
            bus, sender: from, starts: window.starts,
            finishes: window.finishes, frame, contended,
        });
        Ok(window)
    }
}

impl SimulationParticipant for VenNetwork {
    type Output = NetworkOutput;
    type Error = VenError;

    fn current_time(&self) -> SimInstant { self.now }
    fn next_wakeup(&self) -> Option<SimInstant> { self.scheduler.next_time() }

    fn advance(&mut self, grant: TimeGrant) -> Result<AdvanceOutcome<Self::Output>, Self::Error> {
        if grant.until < self.now { return Err(VenError::TimeRegression); }

        if let Some(at) = self.scheduler.next_time().filter(|t| *t <= grant.until) {
            let mut outputs = Vec::new();
            while let Some(event) = self.scheduler.pop_through(at) {
                match event.event {
                    LocalEvent::Ethernet(arrival) => {
                        outputs.push(TimedOutput {
                            at: event.at,
                            value: NetworkOutput::Frame(arrival),
                        });
                    }
                    LocalEvent::Rs485Complete(id) => {
                        // Every event was created together with its transmission.
                        let tx = self.in_flight.remove(&id).expect("scheduled transmission");
                        if tx.contended {
                            // Collision detection is a model diagnostic, not an
                            // automatic UART/MODBUS notification to the sender.
                            outputs.push(TimedOutput {
                                at: event.at,
                                value: NetworkOutput::Rs485Contention {
                                    bus: tx.bus, sender: tx.sender,
                                },
                            });
                        } else {
                            let members = &self.topology.bus(tx.bus).expect("bus exists").members;
                            for &receiver in members {
                                if receiver != tx.sender {
                                    outputs.push(TimedOutput {
                                        at: event.at,
                                        value: NetworkOutput::Frame(FrameArrival {
                                            from: tx.sender,
                                            to: receiver,
                                            frame: tx.frame.clone(),
                                        }),
                                    });
                                }
                            }
                        }
                    }
                }
            }
            self.now = at;
            return Ok(AdvanceOutcome {
                reached: at, outputs, next_wakeup: self.scheduler.next_time(),
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
