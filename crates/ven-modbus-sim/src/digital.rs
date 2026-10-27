//! Bit-sampled Modbus RTU line backed by a *single* RS-485 digital PHY.
//! UART sampling events flow into the existing protocol codec/state machines.
//! No second frame/character network participant is run for this bus.

use crate::{Endpoint, LineError, LineEvent};
use sim_kernel::{AdvanceOutcome, SimulationParticipant, TimeGrant, TimedOutput};
use sim_time::{SimDuration, SimInstant};
use std::collections::BTreeMap;
use ven_core::PortId;
use ven_modbus::{ReceiveEvent, RtuAdu, RtuReceiver, RtuTiming};
use ven_modbus::session::{MasterEvent, RtuMaster, RtuSlave, SlaveEvent};
use ven_rs485_phy::{PhysicalEvent, PhyError, Rs485DigitalPhy, TransmissionTiming, UartSample};

/// High-fidelity digital RS-485 path. Shares no clocks or queues with
/// ven-sim::VenNetwork: one PHY is the sole source of RX events on this bus.
#[derive(Debug)]
pub struct DigitalModbusRtuLine {
    phy: Rs485DigitalPhy,
    timing: RtuTiming,
    endpoints: BTreeMap<PortId, Endpoint>,
}

impl DigitalModbusRtuLine {
    pub fn new(phy: Rs485DigitalPhy) -> Result<Self, LineError> {
        let timing = RtuTiming::new(phy.baud_rate(), phy.uart_format())
            .map_err(LineError::Protocol)?;
        Ok(Self { phy, timing, endpoints: BTreeMap::new() })
    }

    pub fn phy(&self) -> &Rs485DigitalPhy { &self.phy }

    /// Scenario-level fault injection or construction access. Does not
    /// provide a second event source; this PHY remains the only transport.
    pub fn phy_mut(&mut self) -> &mut Rs485DigitalPhy { &mut self.phy }

    fn require_port(&self, port: PortId) -> Result<(), LineError> {
        if self.endpoints.contains_key(&port) { return Err(LineError::DuplicatePort); }
        if !self.phy.has_port(port) { return Err(LineError::MissingPort); }
        Ok(())
    }

    pub fn attach_master(&mut self, port: PortId, timeout: SimDuration) -> Result<(), LineError> {
        self.require_port(port)?;
        let state = RtuMaster::new(timeout).map_err(LineError::Session)?;
        self.endpoints.insert(port, Endpoint::Master {
            rx: RtuReceiver::new(self.timing), state,
        });
        Ok(())
    }

    pub fn attach_slave(&mut self, port: PortId, address: u8) -> Result<(), LineError> {
        self.require_port(port)?;
        // Enforce unique Modbus slave addressing on this one bus; two slaves
        // with the same address are a setup error, not deterministic behavior.
        if self.endpoints.values().any(|endpoint| matches!(
            endpoint, Endpoint::Slave { state, .. } if state.address() == address
        )) { return Err(LineError::DuplicateSlaveAddress); }
        let state = RtuSlave::new(address).map_err(LineError::Session)?;
        self.endpoints.insert(port, Endpoint::Slave {
            rx: RtuReceiver::new(self.timing), state,
        });
        Ok(())
    }

    pub fn seed_register(&mut self, port: PortId, address: u16, value: u16) -> Result<(), LineError> {
        match self.endpoints.get_mut(&port).ok_or(LineError::MissingPort)? {
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

    /// Master request begins at the current simulation instant; the reply
    /// deadline starts at the *end of transmitted UART bits*.
    pub fn request(&mut self, from: PortId, request: RtuAdu) -> Result<TransmissionTiming, LineError> {
        match self.endpoints.get(&from).ok_or(LineError::MissingPort)? {
            Endpoint::Master { state, .. } => state.validate(&request).map_err(LineError::Session)?,
            Endpoint::Slave { .. } => return Err(LineError::NotMaster),
        }
        let timing = self.phy.transmit(from, request.encode()).map_err(LineError::Digital)?;
        if let Some(Endpoint::Master { state, .. }) = self.endpoints.get_mut(&from) {
            state.begin(&request, timing.data_finishes).map_err(LineError::Session)?;
        }
        Ok(timing)
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

    fn accept_frame(
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
            Endpoint::Slave { state, .. } => match state.receive(&frame) {
                SlaveEvent::Ignored => {}
                SlaveEvent::Reply(reply) => replies.push((port, reply)),
                SlaveEvent::BroadcastWriteApplied { register, value } =>
                    outputs.push(TimedOutput {
                        at, value: LineEvent::BroadcastWrite { port, register, value },
                    }),
            },
        }
    }

    fn received(
        &mut self, port: PortId, event: ReceiveEvent, at: SimInstant,
        outputs: &mut Vec<TimedOutput<LineEvent>>,
        replies: &mut Vec<(PortId, RtuAdu)>,
    ) {
        match event {
            ReceiveEvent::Pending => {}
            ReceiveEvent::Frame(frame) => self.accept_frame(port, frame, at, outputs, replies),
            ReceiveEvent::Discarded(reason) => outputs.push(TimedOutput {
                at, value: LineEvent::Discarded { port, reason },
            }),
        }
    }
}

impl SimulationParticipant for DigitalModbusRtuLine {
    type Output = LineEvent;
    type Error = LineError;

    fn current_time(&self) -> SimInstant { self.phy.current_time() }
    fn next_wakeup(&self) -> Option<SimInstant> {
        self.phy.next_wakeup().into_iter().chain(self.next_local_deadline()).min()
    }

    fn advance(&mut self, grant: TimeGrant) -> Result<AdvanceOutcome<Self::Output>, Self::Error> {
        if grant.until < self.phy.current_time() {
            return Err(LineError::Digital(PhyError::TimeRegression));
        }
        let until = self.next_wakeup()
            .map(|next| std::cmp::min(next, grant.until))
            .unwrap_or(grant.until);
        let result = self.phy.advance(TimeGrant { until }).map_err(LineError::Digital)?;
        let at = result.reached;
        let mut outputs = Vec::new();
        let mut replies = Vec::new();
        for event in result.outputs {
            match event.value {
                PhysicalEvent::Character { to, sample, .. } => {
                    if let Some(endpoint) = self.endpoints.get_mut(&to) {
                        match sample {
                            UartSample::Byte(byte) => {
                                let received = endpoint.receiver_mut()
                                    .push_character(byte, event.at).map_err(LineError::Protocol)?;
                                self.received(to, received, event.at, &mut outputs, &mut replies);
                            }
                            sample => {
                                // A damaged physical character is never converted
                                // into a guessed byte or an application exception.
                                endpoint.receiver_mut().invalidate(event.at);
                                outputs.push(TimedOutput {
                                    at: event.at,
                                    value: LineEvent::PhysicalReceiveError { port: to, sample },
                                });
                            }
                        }
                    }
                }
                PhysicalEvent::Contention { bus, first, .. } => {
                    // Collision is a simulation diagnostic; physical UART
                    // receives report their own byte errors as they arise.
                    for endpoint in self.endpoints.values_mut() {
                        endpoint.receiver_mut().invalidate(event.at);
                    }
                    outputs.push(TimedOutput {
                        at: event.at,
                        value: LineEvent::BusContention { bus, sender: first },
                    });
                }
                PhysicalEvent::DriverReleased { .. } | PhysicalEvent::SampledBit { .. } => {}
            }
        }
        let due: Vec<PortId> = self.endpoints.iter()
            .filter(|(_, endpoint)| endpoint.receiver().next_deadline().is_some_and(|d| d <= at))
            .map(|(&port, _)| port).collect();
        for port in due {
            let received = self.endpoints.get_mut(&port).expect("registered").receiver_mut()
                .flush(at).map_err(LineError::Protocol)?;
            self.received(port, received, at, &mut outputs, &mut replies);
        }
        for (from, reply) in replies {
            self.phy.transmit(from, reply.encode()).map_err(LineError::Digital)?;
        }
        // Valid completed responses take precedence over a timeout at the
        // same virtual instant; the host clock remains the only authority.
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
