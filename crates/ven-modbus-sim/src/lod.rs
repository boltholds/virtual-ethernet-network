//! Exactly one RS-485 transport level per Modbus bus.
//!
//! No SimClock or second PHY is created by this enum. Both backends satisfy
//! the same SimulationParticipant contract and share Modbus master/slave types.

use crate::{DigitalModbusRtuLine, LineError, LineEvent, ModbusRtuLine};
use sim_kernel::{AdvanceOutcome, SimulationParticipant, TimeGrant};
use sim_time::{SimDuration, SimInstant};
use ven_core::PortId;
use ven_medium::rs485::Rs485TransmissionWindow;
use ven_modbus::{RtuAdu, RtuTiming};
use ven_rs485_phy::{Rs485DigitalPhy, TransmissionTiming};
use ven_sim::{UartFault, VenNetwork};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusLod {
    /// Fast timestamped characters, no wire bit sampling.
    CharacterTimed,
    /// Digital bit-center sampling with UART parity/framing results.
    DigitalBits,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendTiming {
    Character(Rs485TransmissionWindow),
    Digital(TransmissionTiming),
}

#[derive(Debug)]
pub enum ModbusBus {
    CharacterTimed(ModbusRtuLine),
    DigitalBits(DigitalModbusRtuLine),
}

impl ModbusBus {
    pub fn character_timed(network: VenNetwork, timing: RtuTiming) -> Self {
        Self::CharacterTimed(ModbusRtuLine::new(network, timing))
    }

    pub fn digital_bits(phy: Rs485DigitalPhy) -> Result<Self, LineError> {
        DigitalModbusRtuLine::new(phy).map(Self::DigitalBits)
    }

    pub fn lod(&self) -> BusLod {
        match self {
            Self::CharacterTimed(_) => BusLod::CharacterTimed,
            Self::DigitalBits(_) => BusLod::DigitalBits,
        }
    }

    pub fn attach_master(&mut self, port: PortId, timeout: SimDuration) -> Result<(), LineError> {
        match self {
            Self::CharacterTimed(line) => line.attach_master(port, timeout),
            Self::DigitalBits(line) => line.attach_master(port, timeout),
        }
    }

    pub fn attach_slave(&mut self, port: PortId, address: u8) -> Result<(), LineError> {
        match self {
            Self::CharacterTimed(line) => line.attach_slave(port, address),
            Self::DigitalBits(line) => line.attach_slave(port, address),
        }
    }

    pub fn seed_register(&mut self, port: PortId, address: u16, value: u16) -> Result<(), LineError> {
        match self {
            Self::CharacterTimed(line) => line.seed_register(port, address, value),
            Self::DigitalBits(line) => line.seed_register(port, address, value),
        }
    }

    pub fn slave_register(&self, port: PortId, address: u16) -> Option<u16> {
        match self {
            Self::CharacterTimed(line) => line.slave_register(port, address),
            Self::DigitalBits(line) => line.slave_register(port, address),
        }
    }

    /// Common clean request across either LOD. Fault injection stays at the
    /// explicit transport API, so a bit fault cannot silently become a byte fault.
    pub fn request(&mut self, from: PortId, adu: RtuAdu) -> Result<SendTiming, LineError> {
        match self {
            Self::CharacterTimed(line) => line.request(from, adu, UartFault::None)
                .map(SendTiming::Character),
            Self::DigitalBits(line) => line.request(from, adu).map(SendTiming::Digital),
        }
    }

    pub fn digital_mut(&mut self) -> Option<&mut DigitalModbusRtuLine> {
        match self {
            Self::DigitalBits(line) => Some(line),
            Self::CharacterTimed(_) => None,
        }
    }
}

impl SimulationParticipant for ModbusBus {
    type Output = LineEvent;
    type Error = LineError;

    fn current_time(&self) -> SimInstant {
        match self {
            Self::CharacterTimed(line) => line.current_time(),
            Self::DigitalBits(line) => line.current_time(),
        }
    }
    fn next_wakeup(&self) -> Option<SimInstant> {
        match self {
            Self::CharacterTimed(line) => line.next_wakeup(),
            Self::DigitalBits(line) => line.next_wakeup(),
        }
    }
    fn advance(&mut self, grant: TimeGrant) -> Result<AdvanceOutcome<Self::Output>, Self::Error> {
        match self {
            Self::CharacterTimed(line) => line.advance(grant),
            Self::DigitalBits(line) => line.advance(grant),
        }
    }
}
