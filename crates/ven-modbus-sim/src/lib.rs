//! Explicit Modbus RTU protocol adapter on top of VEN's UART events.
//! Time is owned by the calling Simulation Kernel, NOT by this adapter.
mod causal;
pub use causal::CausalModbusBridge;
mod digital;
mod host;
pub use host::{HostError, ModbusBusHost};
mod lod;
pub use digital::DigitalModbusRtuLine;
pub use lod::{BusLod, ModbusBus, SendTiming};

use sim_kernel::{AdvanceOutcome, SimulationParticipant, TimeGrant, TimedOutput};
use sim_time::{SimDuration, SimInstant};
use std::collections::BTreeMap;
use std::fmt;
use ven_core::{Frame, PortId};
use ven_modbus::{ReceiveEvent, RtuAdu, RtuError, RtuReceiver, RtuTiming};
use ven_modbus::session::{MasterEvent, RtuMaster, RtuSlave, SessionError, SlaveEvent};
use ven_sim::{NetworkOutput, UartFault, VenError, VenNetwork};
use ven_topology::BusId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineError {
    Network(VenError),
    Protocol(RtuError),
    Session(SessionError),
    MissingPort,
    DuplicatePort,
    NotMaster,
    NotSlave,
    DuplicateSlaveAddress,
    Digital(ven_rs485_phy::PhyError),
}

impl fmt::Display for LineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{self:?}") }
}
impl std::error::Error for LineError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LineEvent {
    Master { port: PortId, event: MasterEvent },
    Discarded { port: PortId, reason: RtuError },
    BroadcastWrite { port: PortId, register: u16, value: u16 },
    /// Applied by an addressed slave after a valid FC06 frame, before master ACK.
    SlaveWrite { port: PortId, register: u16, value: u16 },
    BusContention { bus: BusId, sender: PortId },
    PhysicalReceiveError { port: PortId, sample: ven_rs485_phy::UartSample },
}

#[derive(Debug)]
enum Endpoint {
    Master { rx: RtuReceiver, state: RtuMaster },
    Slave { rx: RtuReceiver, state: RtuSlave },
}

impl Endpoint {
    fn receiver(&self) -> &RtuReceiver {
        match self {
            Self::Master { rx, .. } | Self::Slave { rx, .. } => rx,
        }
    }
    fn receiver_mut(&mut self) -> &mut RtuReceiver {
        match self {
            Self::Master { rx, .. } | Self::Slave { rx, .. } => rx,
        }
    }
}

#[derive(Debug)]
pub struct ModbusRtuLine {
    network: VenNetwork,
    timing: RtuTiming,
    endpoints: BTreeMap<PortId, Endpoint>,
    response_fault: UartFault,
}

impl ModbusRtuLine {
    pub fn new(network: VenNetwork, timing: RtuTiming) -> Self {
        Self { network, timing, endpoints: BTreeMap::new(), response_fault: UartFault::None }
    }

    pub fn network(&self) -> &VenNetwork { &self.network }

    /// Scenario-only access for adversarial senders and injected faults.
    pub fn network_mut(&mut self) -> &mut VenNetwork { &mut self.network }

    pub fn set_response_fault(&mut self, fault: UartFault) { self.response_fault = fault; }

    pub fn attach_master(&mut self, port: PortId, timeout: SimDuration) -> Result<(), LineError> {
        self.require_available_port(port)?;
        let master = RtuMaster::new(timeout).map_err(LineError::Session)?;
        self.endpoints.insert(port, Endpoint::Master { rx: RtuReceiver::new(self.timing), state: master });
        Ok(())
    }

    pub fn attach_slave(&mut self, port: PortId, address: u8) -> Result<(), LineError> {
        self.require_available_port(port)?;
        let slave = RtuSlave::new(address).map_err(LineError::Session)?;
        self.endpoints.insert(port, Endpoint::Slave { rx: RtuReceiver::new(self.timing), state: slave });
        Ok(())
    }

    fn require_available_port(&self, port: PortId) -> Result<(), LineError> {
        if self.endpoints.contains_key(&port) { return Err(LineError::DuplicatePort); }
        if self.network.topology().bus_for_port(port).is_none() { return Err(LineError::MissingPort); }
        Ok(())
    }

    pub fn seed_register(&mut self, port: PortId, address: u16, value: u16) -> Result<(), LineError> {
        let endpoint = self.endpoints.get_mut(&port).ok_or(LineError::MissingPort)?;
        match endpoint {
            Endpoint::Slave { state, .. } => { state.seed_register(address, value); Ok(()) }
            Endpoint::Master { .. } => Err(LineError::NotSlave),
        }
    }

    pub fn slave_register(&self, port: PortId, address: u16) -> Option<u16> {
        match self.endpoints.get(&port) {
            Some(Endpoint::Slave { state, .. }) => state.read_register(address),
            _ => None,
        }
    }

    /// Sends at the participant's CURRENT virtual instant. Master timeout
    /// starts at end of transmission, not at the beginning of serialization.
    pub fn request(&mut self, from: PortId, request: RtuAdu, fault: UartFault)
        -> Result<ven_medium::rs485::Rs485TransmissionWindow, LineError>
    {
        match self.endpoints.get(&from).ok_or(LineError::MissingPort)? {
            Endpoint::Master { state, .. } => state.validate(&request).map_err(LineError::Session)?,
            Endpoint::Slave { .. } => return Err(LineError::NotMaster),
        }
        let tx = self.network.send_rs485_uart(from, Frame::new(request.encode()), fault)
            .map_err(LineError::Network)?;
        if let Some(Endpoint::Master { state, .. }) = self.endpoints.get_mut(&from) {
            state.begin(&request, tx.finishes).map_err(LineError::Session)?;
        }
        Ok(tx)
    }

    fn next_local_deadline(&self) -> Option<SimInstant> {
        self.endpoints.values().flat_map(|endpoint| {
            let rx = endpoint.receiver().next_deadline();
            let master = match endpoint {
                Endpoint::Master { state, .. } => state.deadline(),
                Endpoint::Slave { .. } => None,
            };
            rx.into_iter().chain(master)
        }).min()
    }

    fn push_frame(
        &mut self, port: PortId, frame: RtuAdu, at: SimInstant,
        outputs: &mut Vec<TimedOutput<LineEvent>>,
        replies: &mut Vec<(PortId, RtuAdu)>,
    ) {
        let Some(endpoint) = self.endpoints.get_mut(&port) else { return };
        match endpoint {
            Endpoint::Master { state, .. } => {
                let event = state.receive(&frame);
                if event != MasterEvent::Ignored {
                    outputs.push(TimedOutput { at, value: LineEvent::Master { port, event } });
                }
            }
            Endpoint::Slave { state, .. } => {
                match state.receive(&frame) {
                    SlaveEvent::Ignored => {}
                    SlaveEvent::Reply(response) => {
                        if response.function() == 6 && response.data().len() == 4 {
                            let data = response.data();
                            outputs.push(TimedOutput {
                                at,
                                value: LineEvent::SlaveWrite {
                                    port,
                                    register: u16::from_be_bytes([data[0], data[1]]),
                                    value: u16::from_be_bytes([data[2], data[3]]),
                                },
                            });
                        }
                        replies.push((port, response));
                    },
                    SlaveEvent::BroadcastWriteApplied { register, value } =>
                        outputs.push(TimedOutput {
                            at, value: LineEvent::BroadcastWrite { port, register, value },
                        }),
                }
            }
        }
    }

    fn deliver_receive_event(
        &mut self, port: PortId, received: ReceiveEvent, at: SimInstant,
        outputs: &mut Vec<TimedOutput<LineEvent>>,
        replies: &mut Vec<(PortId, RtuAdu)>,
    ) {
        match received {
            ReceiveEvent::Pending => {}
            ReceiveEvent::Frame(frame) => self.push_frame(port, frame, at, outputs, replies),
            ReceiveEvent::Discarded(reason) => outputs.push(TimedOutput {
                at, value: LineEvent::Discarded { port, reason },
            }),
        }
    }
}

impl SimulationParticipant for ModbusRtuLine {
    type Output = LineEvent;
    type Error = LineError;

    fn current_time(&self) -> SimInstant { self.network.current_time() }

    fn next_wakeup(&self) -> Option<SimInstant> {
        self.network.next_wakeup().into_iter().chain(self.next_local_deadline()).min()
    }

    fn advance(&mut self, grant: TimeGrant) -> Result<AdvanceOutcome<Self::Output>, Self::Error> {
        if grant.until < self.network.current_time() {
            return Err(LineError::Network(VenError::TimeRegression));
        }
        let until = self.next_wakeup()
            .map(|next| std::cmp::min(grant.until, next))
            .unwrap_or(grant.until);
        let net = self.network.advance(TimeGrant { until }).map_err(LineError::Network)?;
        let at = net.reached;
        let mut outputs = Vec::new();
        let mut replies = Vec::new();
        for event in net.outputs {
            match event.value {
                NetworkOutput::UartCharacter { to, byte, .. } => {
                    if let Some(endpoint) = self.endpoints.get_mut(&to) {
                        let received = endpoint.receiver_mut().push_character(byte, event.at)
                            .map_err(LineError::Protocol)?;
                        self.deliver_receive_event(to, received, event.at, &mut outputs, &mut replies);
                    }
                }
                NetworkOutput::Rs485Collision { bus, first, second: _ } => {
                    // Poison partially assembled data at the FIRST overlapping
                    // driver instant, rather than the eventual frame completion.
                    for (port, endpoint) in self.endpoints.iter_mut() {
                        if self.network.topology().bus_for_port(*port).is_some_and(|(id, _)| id == bus) {
                            endpoint.receiver_mut().invalidate(event.at);
                        }
                    }
                    outputs.push(TimedOutput {
                        at: event.at,
                        value: LineEvent::BusContention { bus, sender: first },
                    });
                }
                NetworkOutput::Rs485Contention { .. } => {
                    // Summary at TX completion; immediate collision was already
                    // published and receive state already invalidated.
                }
                NetworkOutput::Rs485DriverReleased { .. } => {
                    // Physical-direction diagnostic; protocol timer owns its
                    // own t3.5 silence, independent of hardware release.
                }
                NetworkOutput::Frame(_) => {
                    // Legacy whole-frame RS-485/Ethernet is not a source of
                    // Modbus RTU character samples.
                }
            }
        }
        let due: Vec<_> = self.endpoints.iter()
            .filter(|(_, endpoint)| endpoint.receiver().next_deadline().is_some_and(|d| d <= at))
            .map(|(&port, _)| port).collect();
        for port in due {
            let received = self.endpoints.get_mut(&port).expect("registered").receiver_mut()
                .flush(at).map_err(LineError::Protocol)?;
            self.deliver_receive_event(port, received, at, &mut outputs, &mut replies);
        }
        // A response begins only after the request receiver has observed t3.5.
        for (from, response) in replies {
            self.network.send_rs485_uart(from, Frame::new(response.encode()), self.response_fault)
                .map_err(LineError::Network)?;
        }
        // Process completed responses BEFORE the timeout at the same timestamp.
        for (&port, endpoint) in self.endpoints.iter_mut() {
            if let Endpoint::Master { state, .. } = endpoint {
                let event = state.poll_timeout(at);
                if event != MasterEvent::Ignored {
                    outputs.push(TimedOutput {
                        at, value: LineEvent::Master { port, event },
                    });
                }
            }
        }
        Ok(AdvanceOutcome { reached: at, outputs, next_wakeup: self.next_wakeup() })
    }
}
