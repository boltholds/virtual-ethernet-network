//! Minimal deterministic Modbus RTU session semantics.
//! Supported functions: 0x03 (read holding registers), 0x06 (write single).
use crate::RtuAdu;
use sim_time::{SimDuration, SimInstant};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionError {
    Busy,
    ZeroTimeout,
    InvalidRequest,
    UnsupportedFunction,
    InvalidSlaveAddress,
    TimeOverflow,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MasterEvent {
    Ignored,
    BroadcastComplete,
    Reply(RtuAdu),
    Exception { address: u8, function: u8, code: u8 },
    TimedOut { address: u8, function: u8 },
}

#[derive(Clone, Debug)]
enum MasterState {
    Ready,
    Awaiting {
        address: u8,
        function: u8,
        expected_data: Vec<u8>,
        deadline: SimInstant,
    },
}

#[derive(Clone, Debug)]
pub struct RtuMaster {
    state: MasterState,
    timeout: SimDuration,
}

impl RtuMaster {
    pub fn new(timeout: SimDuration) -> Result<Self, SessionError> {
        if timeout == SimDuration::ZERO { return Err(SessionError::ZeroTimeout); }
        Ok(Self { timeout, state: MasterState::Ready })
    }

    pub fn ready(&self) -> bool { matches!(self.state, MasterState::Ready) }

    pub fn deadline(&self) -> Option<SimInstant> {
        match &self.state {
            MasterState::Ready => None,
            MasterState::Awaiting { deadline, .. } => Some(*deadline),
        }
    }

    pub fn validate(&self, request: &RtuAdu) -> Result<(), SessionError> {
        if !self.ready() { return Err(SessionError::Busy); }
        if !matches!(request.function(), 3 | 6) { return Err(SessionError::UnsupportedFunction); }
        if request.data().len() != 4 { return Err(SessionError::InvalidRequest); }
        if request.address() == 0 && request.function() != 6 {
            return Err(SessionError::InvalidRequest);
        }
        if request.function() == 3 {
            let count = u16::from_be_bytes([request.data()[2], request.data()[3]]);
            if count == 0 || count > 125 {
                return Err(SessionError::InvalidRequest);
            }
        }
        Ok(())
    }

    /// The clock starts at the **end of local transmission**, not at request creation.
    pub fn begin(&mut self, request: &RtuAdu, transmit_end: SimInstant) -> Result<MasterEvent, SessionError> {
        self.validate(request)?;
        if request.address() == 0 { return Ok(MasterEvent::BroadcastComplete); }
        let deadline = transmit_end.checked_add(self.timeout).ok_or(SessionError::TimeOverflow)?;
        self.state = MasterState::Awaiting {
            address: request.address(),
            function: request.function(),
            expected_data: request.data().to_vec(),
            deadline,
        };
        Ok(MasterEvent::Ignored)
    }

    pub fn receive(&mut self, response: &RtuAdu) -> MasterEvent {
        let (address, function, expected) = match &self.state {
            MasterState::Ready => return MasterEvent::Ignored,
            MasterState::Awaiting { address, function, expected_data, .. } =>
                (*address, *function, expected_data.clone()),
        };
        if response.address() != address { return MasterEvent::Ignored; }
        if response.function() == (function | 0x80) && response.data().len() == 1 {
            self.state = MasterState::Ready;
            return MasterEvent::Exception {
                address, function, code: response.data()[0],
            };
        }
        if response.function() != function { return MasterEvent::Ignored; }
        let valid = match function {
            3 => {
                let quantity = u16::from_be_bytes([expected[2], expected[3]]) as usize;
                response.data().len() == 1 + 2 * quantity
                    && response.data()[0] as usize == 2 * quantity
            }
            6 => response.data() == expected,
            _ => false,
        };
        if !valid { return MasterEvent::Ignored; }
        self.state = MasterState::Ready;
        MasterEvent::Reply(response.clone())
    }

    pub fn poll_timeout(&mut self, now: SimInstant) -> MasterEvent {
        match self.state {
            MasterState::Awaiting { address, function, deadline, .. } if now >= deadline => {
                self.state = MasterState::Ready;
                MasterEvent::TimedOut { address, function }
            }
            _ => MasterEvent::Ignored,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SlaveEvent {
    Ignored,
    BroadcastWriteApplied { register: u16, value: u16 },
    Reply(RtuAdu),
}

#[derive(Clone, Debug)]
pub struct RtuSlave {
    address: u8,
    holding: BTreeMap<u16, u16>,
}

impl RtuSlave {
    pub fn new(address: u8) -> Result<Self, SessionError> {
        if !(1..=247).contains(&address) { return Err(SessionError::InvalidSlaveAddress); }
        Ok(Self { address, holding: BTreeMap::new() })
    }

    pub fn address(&self) -> u8 { self.address }
    pub fn seed_register(&mut self, register: u16, value: u16) {
        self.holding.insert(register, value);
    }
    pub fn read_register(&self, register: u16) -> Option<u16> {
        self.holding.get(&register).copied()
    }

    pub fn receive(&mut self, request: &RtuAdu) -> SlaveEvent {
        let broadcast = request.address() == 0;
        if request.address() != self.address && !broadcast {
            return SlaveEvent::Ignored;
        }
        // Broadcast is write-only; it must never trigger a response.
        if broadcast && request.function() != 6 { return SlaveEvent::Ignored; }
        let data = request.data();
        let response = match request.function() {
            3 => {
                if data.len() != 4 { self.exception(3, 3) }
                else {
                    let base = u16::from_be_bytes([data[0], data[1]]);
                    let count = u16::from_be_bytes([data[2], data[3]]);
                    if count == 0 || count > 125 { self.exception(3, 3) }
                    else {
                        let mut output = Vec::with_capacity(1 + (count as usize) * 2);
                        output.push((count * 2) as u8);
                        let mut missing = false;
                        for offset in 0..count {
                            match base.checked_add(offset).and_then(|key| self.holding.get(&key)) {
                                Some(value) => output.extend_from_slice(&value.to_be_bytes()),
                                None => { missing = true; break; }
                            }
                        }
                        if missing { self.exception(3, 2) }
                        else { RtuAdu::new(self.address, 3, output).expect("bounded registers") }
                    }
                }
            }
            6 => {
                if data.len() != 4 { self.exception(6, 3) }
                else {
                    let register = u16::from_be_bytes([data[0], data[1]]);
                    let value = u16::from_be_bytes([data[2], data[3]]);
                    if let std::collections::btree_map::Entry::Occupied(mut entry) = self.holding.entry(register) {
                        entry.insert(value);
                        if broadcast {
                            return SlaveEvent::BroadcastWriteApplied { register, value };
                        }
                        RtuAdu::new(self.address, 6, data.to_vec()).expect("write echo")
                    } else {
                        self.exception(6, 2)
                    }
                }
            }
            function => self.exception(function, 1),
        };
        if broadcast { SlaveEvent::Ignored } else { SlaveEvent::Reply(response) }
    }

    fn exception(&self, function: u8, code: u8) -> RtuAdu {
        RtuAdu::new(self.address, function | 0x80, vec![code])
            .expect("valid function exception")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_rejects_overlapping_requests_and_times_out() {
        let mut master = RtuMaster::new(SimDuration::from_nanos(500)).unwrap();
        let request = RtuAdu::new(2, 3, vec![0, 0, 0, 1]).unwrap();
        master.begin(&request, SimInstant::from_nanos(100)).unwrap();
        assert_eq!(master.validate(&request), Err(SessionError::Busy));
        assert_eq!(master.poll_timeout(SimInstant::from_nanos(599)), MasterEvent::Ignored);
        assert_eq!(master.poll_timeout(SimInstant::from_nanos(600)),
            MasterEvent::TimedOut { address: 2, function: 3 });
        assert!(master.ready());
    }

    #[test]
    fn slave_filters_address_and_handles_read_write_broadcast() {
        let mut slave = RtuSlave::new(2).unwrap();
        slave.seed_register(7, 0x1234);
        let ignored = RtuAdu::new(3, 3, vec![0, 7, 0, 1]).unwrap();
        assert_eq!(slave.receive(&ignored), SlaveEvent::Ignored);
        let read = RtuAdu::new(2, 3, vec![0, 7, 0, 1]).unwrap();
        let SlaveEvent::Reply(reply) = slave.receive(&read) else { panic!("expected reply") };
        assert_eq!(reply.data(), &[2, 0x12, 0x34]);
        let broadcast = RtuAdu::new(0, 6, vec![0, 7, 0x43, 0x21]).unwrap();
        assert_eq!(slave.receive(&broadcast),
            SlaveEvent::BroadcastWriteApplied { register: 7, value: 0x4321 });
        assert_eq!(slave.read_register(7), Some(0x4321));
    }

    #[test]
    fn modbus_exception_is_reported_by_master() {
        let mut master = RtuMaster::new(SimDuration::from_nanos(1000)).unwrap();
        let req = RtuAdu::new(2, 3, vec![0, 4, 0, 1]).unwrap();
        master.begin(&req, SimInstant::ZERO).unwrap();
        let mut slave = RtuSlave::new(2).unwrap();
        let SlaveEvent::Reply(exception) = slave.receive(&req) else { panic!("expected exception") };
        assert_eq!(master.receive(&exception),
            MasterEvent::Exception { address: 2, function: 3, code: 2 });
    }

    #[test]
    fn master_broadcast_does_not_wait_for_response() {
        let mut master = RtuMaster::new(SimDuration::from_nanos(500)).unwrap();
        let broadcast = RtuAdu::new(0, 6, vec![0, 1, 0, 5]).unwrap();
        assert_eq!(master.begin(&broadcast, SimInstant::ZERO), Ok(MasterEvent::BroadcastComplete));
        assert!(master.ready());
        assert_eq!(master.validate(&RtuAdu::new(0, 3, vec![0, 1, 0, 1]).unwrap()), Err(SessionError::InvalidRequest));
    }
}
