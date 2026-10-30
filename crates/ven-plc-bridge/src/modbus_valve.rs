//! PLC -> Modbus FC06 -> *slave-applied* valve command; valve limit
//! feedback -> slave holding registers -> Modbus FC03 -> PLC scan input.
//!
//! The bus is exclusively owned here. No second protocol/PHY participant runs
//! for the same BusId. All events use the coordinator's virtual time.

use crate::{BridgeError, IoMessage};
use sim_kernel::{AdvanceOutcome, SimulationParticipant, TimeGrant, TimedOutput};
use sim_kernel::conservative::{ActorError, CausalInput, CausalParticipant, DirectedEvent};
use sim_kernel::external_boundary::{SignalKey, SignalValue};
use sim_kernel::resources::ParticipantId;
use sim_time::SimInstant;
use std::collections::VecDeque;
use std::fmt;
use ven_core::PortId;
use ven_modbus::RtuAdu;
use ven_modbus::session::MasterEvent;
use ven_modbus_sim::{LineError, LineEvent, ModbusBus};
use ven_topology::BusId;

const COMMAND: u16 = 0;
const OPENED: u16 = 1;
const CLOSED: u16 = 2;
const FAULT: u16 = 3;
const MAX_PENDING: usize = 16;

#[derive(Clone, Debug)]
pub struct ValveModbusConfig {
    pub plc: ParticipantId,
    pub plant: ParticipantId,
    pub master_port: PortId,
    pub slave_port: PortId,
    pub slave_address: u8,
    pub command_key: SignalKey,
    pub opened_key: SignalKey,
    pub closed_key: SignalKey,
    pub fault_key: SignalKey,
    pub communication_fault_key: SignalKey,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayError {
    InvalidConfiguration,
    InvalidMessage,
    InvalidReply,
    QueueFull,
    UnexpectedMasterEvent,
    Protocol(LineError),
}

impl fmt::Display for GatewayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{self:?}") }
}
impl std::error::Error for GatewayError {}
impl From<LineError> for GatewayError {
    fn from(error: LineError) -> Self { Self::Protocol(error) }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Transaction { Write(bool), Read }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InFlight { Idle, Pending(Transaction) }

/// A single master serializes FC06 and FC03. No concurrent Modbus requests,
/// no fabricated sensor values, and no direct PLC-to-valve I/O shortcut.
pub struct ValveModbusGateway {
    bus: ModbusBus,
    cfg: ValveModbusConfig,
    queue: VecDeque<Transaction>,
    inflight: InFlight,
}

impl ValveModbusGateway {
    pub fn new(mut bus: ModbusBus, cfg: ValveModbusConfig) -> Result<Self, GatewayError> {
        if !(1..=247).contains(&cfg.slave_address)
            || cfg.plc == cfg.plant || cfg.master_port == cfg.slave_port
            || [
                &cfg.command_key, &cfg.opened_key, &cfg.closed_key,
                &cfg.fault_key, &cfg.communication_fault_key,
            ].iter().enumerate().any(|(i, left)| {
                [
                    &cfg.command_key, &cfg.opened_key, &cfg.closed_key,
                    &cfg.fault_key, &cfg.communication_fault_key,
                ][i + 1..].contains(left)
            })
        { return Err(GatewayError::InvalidConfiguration); }
        // Seed explicit register map for a standalone pilot device.
        for (register, value) in [(COMMAND, 0), (OPENED, 0), (CLOSED, 1), (FAULT, 0)] {
            bus.seed_register(cfg.slave_port, register, value)?;
        }
        Ok(Self { bus, cfg, queue: VecDeque::new(), inflight: InFlight::Idle })
    }

    pub fn claimed_buses(&self) -> Vec<BusId> { self.bus.claimed_buses() }

    pub fn slave_register(&self, address: u16) -> Option<u16> {
        self.bus.slave_register(self.cfg.slave_port, address)
    }

    fn queued(&mut self, tx: Transaction) -> Result<(), GatewayError> {
        if tx == Transaction::Read && (self.inflight == InFlight::Pending(Transaction::Read)
            || self.queue.contains(&Transaction::Read))
        {
            return Ok(());
        }
        if self.queue.len() == MAX_PENDING { return Err(GatewayError::QueueFull); }
        self.queue.push_back(tx);
        Ok(())
    }

    fn request(&mut self, tx: Transaction) -> Result<(), GatewayError> {
        let bytes = match tx {
            Transaction::Write(value) => {
                let mut data = COMMAND.to_be_bytes().to_vec();
                data.extend_from_slice(&u16::from(value).to_be_bytes());
                RtuAdu::new(self.cfg.slave_address, 6, data)
            }
            Transaction::Read => {
                let mut data = OPENED.to_be_bytes().to_vec();
                data.extend_from_slice(&3_u16.to_be_bytes());
                RtuAdu::new(self.cfg.slave_address, 3, data)
            }
        }.map_err(|_| GatewayError::InvalidConfiguration)?;
        self.bus.request(self.cfg.master_port, bytes)?;
        self.inflight = InFlight::Pending(tx);
        Ok(())
    }

    fn send_signal(&self, at: SimInstant, key: &SignalKey, value: bool,
        outputs: &mut Vec<TimedOutput<DirectedEvent<IoMessage>>>,
    ) {
        outputs.push(TimedOutput {
            at,
            value: DirectedEvent {
                target: self.cfg.plc,
                payload: IoMessage::PlcInput {
                    key: key.clone(), value: SignalValue::Bool(value),
                },
            },
        });
    }

    fn complete(
        &mut self, event: MasterEvent, at: SimInstant,
        outputs: &mut Vec<TimedOutput<DirectedEvent<IoMessage>>>,
    ) -> Result<(), GatewayError> {
        let active = std::mem::replace(&mut self.inflight, InFlight::Idle);
        match (active, event) {
            (InFlight::Pending(Transaction::Write(_)), MasterEvent::Reply(adu))
                if adu.function() == 6 => {
                self.send_signal(at, &self.cfg.communication_fault_key, false, outputs);
            }
            (InFlight::Pending(Transaction::Read), MasterEvent::Reply(adu))
                if adu.function() == 3 && adu.data().len() == 7 && adu.data()[0] == 6 => {
                let regs = adu.data();
                let values = [
                    u16::from_be_bytes([regs[1], regs[2]]),
                    u16::from_be_bytes([regs[3], regs[4]]),
                    u16::from_be_bytes([regs[5], regs[6]]),
                ];
                if values.iter().any(|v| *v > 1) { return Err(GatewayError::InvalidReply); }
                for (key, value) in [
                    (&self.cfg.opened_key, values[0] == 1),
                    (&self.cfg.closed_key, values[1] == 1),
                    (&self.cfg.fault_key, values[2] == 1),
                ] {
                    self.send_signal(at, key, value, outputs);
                }
                self.send_signal(at, &self.cfg.communication_fault_key, false, outputs);
            }
            (InFlight::Pending(_), MasterEvent::TimedOut { .. } | MasterEvent::Exception { .. }) => {
                self.send_signal(at, &self.cfg.communication_fault_key, true, outputs);
            }
            _ => return Err(GatewayError::UnexpectedMasterEvent),
        }
        Ok(())
    }
}

impl CausalParticipant<IoMessage> for ValveModbusGateway {
    fn current_time(&self) -> SimInstant { self.bus.current_time() }
    fn next_wakeup(&self) -> Option<SimInstant> {
        let protocol = self.bus.next_wakeup();
        if self.inflight == InFlight::Idle && !self.queue.is_empty() {
            Some(protocol.map_or(self.bus.current_time(), |p| p.min(self.bus.current_time())))
        } else {
            protocol
        }
    }

    fn advance(&mut self, grant: TimeGrant)
        -> Result<AdvanceOutcome<DirectedEvent<IoMessage>>, ActorError>
    {
        let result = self.bus.advance(grant).map_err(|e| Box::new(e) as ActorError)?;
        let mut outputs = Vec::new();
        for event in result.outputs {
            match event.value {
                LineEvent::SlaveWrite { port, register: COMMAND, value }
                    if port == self.cfg.slave_port => {
                    if value > 1 { return Err(Box::new(GatewayError::InvalidReply)); }
                    outputs.push(TimedOutput {
                        at: event.at,
                        value: DirectedEvent {
                            target: self.cfg.plant,
                            payload: IoMessage::PlcOutput {
                                key: self.cfg.command_key.clone(),
                                value: SignalValue::Bool(value == 1),
                            },
                        },
                    });
                }
                LineEvent::Master { port, event: master_event }
                    if port == self.cfg.master_port && master_event != MasterEvent::Ignored => {
                    self.complete(master_event, event.at, &mut outputs)
                        .map_err(|e| Box::new(e) as ActorError)?;
                }
                _ => {}
            }
        }
        if self.inflight == InFlight::Idle {
            if let Some(&tx) = self.queue.front() {
                self.request(tx).map_err(|e| Box::new(e) as ActorError)?;
                self.queue.pop_front();
            }
        }
        Ok(AdvanceOutcome {
            reached: result.reached, outputs, next_wakeup: self.next_wakeup(),
        })
    }
    fn receive(&mut self, input: CausalInput<IoMessage>) -> Result<(), ActorError> {
        if input.at != self.bus.current_time() {
            return Err(Box::new(BridgeError::TimeMismatch));
        }
        match input.payload {
            IoMessage::PlcOutput { key, value } if input.source == self.cfg.plc
                && key == self.cfg.command_key => {
                if let SignalValue::Bool(value) = value {
                    self.queued(Transaction::Write(value))
                        .map_err(|e| Box::new(e) as ActorError)
                } else { Err(Box::new(GatewayError::InvalidMessage)) }
            }
            IoMessage::PollValve if input.source == self.cfg.plc => {
                self.queued(Transaction::Read)
                    .map_err(|e| Box::new(e) as ActorError)
            }
            IoMessage::PlcInput { key, value } if input.source == self.cfg.plant => {
                let register = if key == self.cfg.opened_key { OPENED }
                    else if key == self.cfg.closed_key { CLOSED }
                    else if key == self.cfg.fault_key { FAULT }
                    else { return Err(Box::new(GatewayError::InvalidMessage)) };
                let boolean = match value {
                    SignalValue::Bool(value) => value,
                    _ => return Err(Box::new(GatewayError::InvalidMessage)),
                };
                self.bus.seed_register(self.cfg.slave_port, register, u16::from(boolean))
                    .map_err(|e| Box::new(e) as ActorError)
            }
            _ => Err(Box::new(GatewayError::InvalidMessage)),
        }
    }
}
