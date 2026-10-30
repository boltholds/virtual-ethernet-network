//! Modbus RTU codec and virtual-time UART framing.
//!
//! Pure protocol layer: NO RS-485 transceiver, physical collision model,
//! Modbus register map, automatic master/slave transactions or wall clocks.
//! Reference: MODBUS over Serial Line Specification V1.02, §§2.2, 2.5.1, 6.2.2.

pub mod session;

use sim_time::{SimDuration, SimInstant};
use ven_medium::rs485::{Rs485Error, UartFormat};

pub const MAX_RTU_ADU_LEN: usize = 256;
pub const MAX_RTU_DATA_LEN: usize = MAX_RTU_ADU_LEN - 4;
const NANOS_PER_SEC: u128 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RtuError {
    ReservedAddress,
    InvalidFunction,
    InvalidLength,
    CrcMismatch,
    InvalidBaudRate,
    InvalidUartFormat,
    TimeRegression,
    OverlappingCharacters,
    InterCharacterTimeout,
    TimeOverflow,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RtuAdu {
    address: u8,
    function: u8,
    data: Vec<u8>,
}

impl RtuAdu {
    /// Address zero means broadcast; addresses 1..=247 identify devices.
    /// Function 0 is reserved. Function-code semantics are not implemented.
    pub fn new(address: u8, function: u8, data: Vec<u8>) -> Result<Self, RtuError> {
        if address > 247 { return Err(RtuError::ReservedAddress); }
        if function == 0 { return Err(RtuError::InvalidFunction); }
        if data.len() > MAX_RTU_DATA_LEN { return Err(RtuError::InvalidLength); }
        Ok(Self { address, function, data })
    }

    pub fn address(&self) -> u8 { self.address }
    pub fn function(&self) -> u8 { self.function }
    pub fn data(&self) -> &[u8] { &self.data }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.data.len() + 4);
        bytes.push(self.address);
        bytes.push(self.function);
        bytes.extend_from_slice(&self.data);
        bytes.extend_from_slice(&crc16_modbus(&bytes).to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, RtuError> {
        if !(4..=MAX_RTU_ADU_LEN).contains(&bytes.len()) {
            return Err(RtuError::InvalidLength);
        }
        let checksum = u16::from_le_bytes([bytes[bytes.len() - 2], bytes[bytes.len() - 1]]);
        if crc16_modbus(&bytes[..bytes.len() - 2]) != checksum {
            return Err(RtuError::CrcMismatch);
        }
        Self::new(bytes[0], bytes[1], bytes[2..bytes.len() - 2].to_vec())
    }
}

/// MODBUS CRC16: initial FFFF, reflected polynomial A001, low-order CRC byte first.
pub fn crc16_modbus(bytes: &[u8]) -> u16 {
    let mut crc = 0xffff_u16;
    for &byte in bytes {
        crc ^= u16::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xa001 } else { crc >> 1 };
        }
    }
    crc
}

/// Durations from the Modbus Serial Line V1.02 recommendations.
/// At >19200 baud, fixed 750us / 1750us are recommended, not physical constants.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RtuTiming {
    pub character: SimDuration,
    pub inter_character: SimDuration,
    pub inter_frame: SimDuration,
}

impl RtuTiming {
    pub fn new(baud_rate: u64, uart: UartFormat) -> Result<Self, RtuError> {
        if baud_rate == 0 { return Err(RtuError::InvalidBaudRate); }
        let bits = uart.bits_per_character().map_err(|_: Rs485Error| RtuError::InvalidUartFormat)?;
        // Modbus RTU transmits one start bit, 8 data bits, plus parity/stop to 11 bits.
        if uart.data_bits != 8 || bits != 11 {
            return Err(RtuError::InvalidUartFormat);
        }
        let character = duration_ceil(u128::from(bits) * NANOS_PER_SEC, u128::from(baud_rate))?;
        let (inter_character, inter_frame) = if baud_rate > 19_200 {
            (SimDuration::from_nanos(750_000), SimDuration::from_nanos(1_750_000))
        } else {
            (
                duration_ceil(u128::from(bits) * 3 * NANOS_PER_SEC, u128::from(baud_rate) * 2)?,
                duration_ceil(u128::from(bits) * 7 * NANOS_PER_SEC, u128::from(baud_rate) * 2)?,
            )
        };
        Ok(Self { character, inter_character, inter_frame })
    }
}

fn duration_ceil(numerator: u128, denominator: u128) -> Result<SimDuration, RtuError> {
    let nanos = numerator / denominator + u128::from(numerator % denominator != 0);
    Ok(SimDuration::from_nanos(
        u64::try_from(nanos).map_err(|_| RtuError::TimeOverflow)?
    ))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReceiveEvent {
    Pending,
    Frame(RtuAdu),
    Discarded(RtuError),
}

// Explicit states rather than nullable/partially initialized receiver data.
#[derive(Clone, Debug)]
enum ReceiveState {
    Idle,
    Collecting { bytes: Vec<u8>, last_complete: SimInstant },
    Discarding { last_complete: SimInstant },
}

/// A **character-completion-time** RTU parser. Its caller must supply one UART
/// byte-completion event per byte; VEN's existing frame-level RS485 API is not
/// itself a source of those events. This layer cannot infer intra-frame gaps
/// from a complete already-delivered frame.
#[derive(Clone, Debug)]
pub struct RtuReceiver {
    timing: RtuTiming,
    state: ReceiveState,
}

impl RtuReceiver {
    pub fn new(timing: RtuTiming) -> Self {
        Self { timing, state: ReceiveState::Idle }
    }

    pub fn timing(&self) -> RtuTiming { self.timing }

    pub fn next_deadline(&self) -> Option<SimInstant> {
        match &self.state {
            ReceiveState::Idle => None,
            ReceiveState::Collecting { last_complete, .. }
            | ReceiveState::Discarding { last_complete } =>
                last_complete.checked_add(self.timing.inter_frame),
        }
    }

    /// Bus-level corruption invalidates partial data; no Modbus exception
    /// response is synthesized for damaged serial data.
    pub fn invalidate(&mut self, at: SimInstant) {
        self.state = ReceiveState::Discarding { last_complete: at };
    }

    /// `at` is the end of this byte's last stop bit (receiver-local virtual time).
    /// The silence is end-to-start, NOT end-to-end between consecutive bytes.
    pub fn push_character(&mut self, byte: u8, at: SimInstant) -> Result<ReceiveEvent, RtuError> {
        let silence = match &self.state {
            ReceiveState::Idle => SimDuration::ZERO,
            ReceiveState::Collecting { last_complete, .. }
            | ReceiveState::Discarding { last_complete } => {
                let delta = at.checked_duration_since(*last_complete)
                    .ok_or(RtuError::TimeRegression)?;
                // Cumulative ceil(bit_count * 1e9 / baud) can produce adjacent
                // character completions one nanosecond shorter than ceil(one
                // character). This is quantization, not a physical overlap.
                if delta < self.timing.character {
                    let deficit = self.timing.character.as_nanos() - delta.as_nanos();
                    if deficit > 1 { return Err(RtuError::OverlappingCharacters); }
                    SimDuration::ZERO
                } else {
                    delta.checked_sub(self.timing.character).expect("nonnegative")
                }
            }
        };
        let previous = std::mem::replace(&mut self.state, ReceiveState::Idle);
        match previous {
            ReceiveState::Idle => {
                self.state = ReceiveState::Collecting { bytes: vec![byte], last_complete: at };
                Ok(ReceiveEvent::Pending)
            }
            ReceiveState::Collecting { mut bytes, .. } if silence >= self.timing.inter_frame => {
                let previous = RtuAdu::decode(&bytes).map(ReceiveEvent::Frame)
                    .unwrap_or_else(ReceiveEvent::Discarded);
                self.state = ReceiveState::Collecting { bytes: vec![byte], last_complete: at };
                Ok(previous)
            }
            ReceiveState::Collecting { .. } if silence > self.timing.inter_character => {
                self.state = ReceiveState::Discarding { last_complete: at };
                Ok(ReceiveEvent::Discarded(RtuError::InterCharacterTimeout))
            }
            ReceiveState::Collecting { mut bytes, .. } => {
                if bytes.len() >= MAX_RTU_ADU_LEN {
                    self.state = ReceiveState::Discarding { last_complete: at };
                    return Ok(ReceiveEvent::Discarded(RtuError::InvalidLength));
                }
                bytes.push(byte);
                self.state = ReceiveState::Collecting { bytes, last_complete: at };
                Ok(ReceiveEvent::Pending)
            }
            ReceiveState::Discarding { .. } if silence >= self.timing.inter_frame => {
                self.state = ReceiveState::Collecting { bytes: vec![byte], last_complete: at };
                Ok(ReceiveEvent::Pending)
            }
            ReceiveState::Discarding { .. } => {
                self.state = ReceiveState::Discarding { last_complete: at };
                Ok(ReceiveEvent::Pending)
            }
        }
    }

    /// Publish a completed frame only after the required inter-frame silence.
    pub fn flush(&mut self, at: SimInstant) -> Result<ReceiveEvent, RtuError> {
        let last = match &self.state {
            ReceiveState::Idle => return Ok(ReceiveEvent::Pending),
            ReceiveState::Collecting { last_complete, .. }
            | ReceiveState::Discarding { last_complete } => *last_complete,
        };
        let elapsed = at.checked_duration_since(last).ok_or(RtuError::TimeRegression)?;
        if elapsed < self.timing.inter_frame { return Ok(ReceiveEvent::Pending); }
        match std::mem::replace(&mut self.state, ReceiveState::Idle) {
            ReceiveState::Collecting { bytes, .. } =>
                Ok(RtuAdu::decode(&bytes).map(ReceiveEvent::Frame)
                    .unwrap_or_else(ReceiveEvent::Discarded)),
            ReceiveState::Idle | ReceiveState::Discarding { .. } =>
                Ok(ReceiveEvent::Pending),
        }
    }

    pub fn reset(&mut self) { self.state = ReceiveState::Idle; }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ven_medium::rs485::UartFormat;

    fn timing() -> RtuTiming {
        RtuTiming::new(9600, UartFormat::MODBUS_RTU_8E1).unwrap()
    }

    #[test]
    fn known_crc_vector_and_byte_order() {
        let request = [0x01, 0x03, 0x00, 0x00, 0x00, 0x0a];
        assert_eq!(crc16_modbus(&request), 0xcdc5);
        let adu = RtuAdu::new(1, 3, vec![0, 0, 0, 10]).unwrap();
        assert_eq!(adu.encode(), [1, 3, 0, 0, 0, 10, 0xc5, 0xcd]);
        assert_eq!(RtuAdu::decode(&adu.encode()), Ok(adu));
    }

    #[test]
    fn crc_and_address_errors_are_not_silently_accepted() {
        let mut frame = RtuAdu::new(1, 3, vec![0, 0, 0, 10]).unwrap().encode();
        frame[4] ^= 0xff;
        assert_eq!(RtuAdu::decode(&frame), Err(RtuError::CrcMismatch));
        assert_eq!(RtuAdu::new(248, 3, vec![]), Err(RtuError::ReservedAddress));
        assert_eq!(RtuAdu::new(0, 0, vec![]), Err(RtuError::InvalidFunction));
        assert_eq!(RtuAdu::new(0, 6, vec![0; 253]), Err(RtuError::InvalidLength));
    }

    #[test]
    fn maximum_rtu_frame_length_is_256() {
        let frame = RtuAdu::new(247, 16, vec![0xab; 252]).unwrap().encode();
        assert_eq!(frame.len(), 256);
        assert_eq!(RtuAdu::decode(&frame).unwrap().data().len(), 252);
    }

    #[test]
    fn timings_follow_spec_with_explicit_ceil() {
        let t = timing();
        assert_eq!(t.character.as_nanos(), 1_145_834);
        assert_eq!(t.inter_character.as_nanos(), 1_718_750);
        assert_eq!(t.inter_frame.as_nanos(), 4_010_417);
        let high = RtuTiming::new(38_400, UartFormat::MODBUS_RTU_8N2).unwrap();
        assert_eq!(high.inter_character.as_nanos(), 750_000);
        assert_eq!(high.inter_frame.as_nanos(), 1_750_000);
        assert_eq!(RtuTiming::new(0, UartFormat::MODBUS_RTU_8E1), Err(RtuError::InvalidBaudRate));
    }

    #[test]
    fn receiver_releases_complete_frame_only_after_t35() {
        let timing = timing();
        let adu = RtuAdu::new(1, 3, vec![0, 0, 0, 10]).unwrap();
        let data = adu.encode();
        let mut rx = RtuReceiver::new(timing);
        let mut time = SimInstant::ZERO;
        for b in data {
            time = time.checked_add(timing.character).unwrap();
            assert_eq!(rx.push_character(b, time).unwrap(), ReceiveEvent::Pending);
        }
        let early = time.checked_add(timing.inter_frame.checked_sub(SimDuration::from_nanos(1)).unwrap()).unwrap();
        assert_eq!(rx.flush(early), Ok(ReceiveEvent::Pending));
        assert_eq!(rx.flush(time.checked_add(timing.inter_frame).unwrap()), Ok(ReceiveEvent::Frame(adu)));
    }

    #[test]
    fn incomplete_frame_dropped_after_inter_character_gap() {
        let t = timing();
        let mut rx = RtuReceiver::new(t);
        rx.push_character(1, SimInstant::from_nanos(t.character.as_nanos())).unwrap();
        let break_at = SimInstant::from_nanos(
            t.character.as_nanos() * 2 + t.inter_character.as_nanos() + 1
        );
        assert_eq!(rx.push_character(3, break_at),
            Ok(ReceiveEvent::Discarded(RtuError::InterCharacterTimeout)));
        assert_eq!(rx.flush(break_at.checked_add(t.inter_frame).unwrap()), Ok(ReceiveEvent::Pending));
    }

    #[test]
    fn boundary_t35_finishes_prior_frame_and_starts_new_one() {
        let t = timing();
        let adu = RtuAdu::new(1, 6, vec![]).unwrap();
        let mut rx = RtuReceiver::new(t);
        let mut at = SimInstant::ZERO;
        for b in adu.encode() {
            at = at.checked_add(t.character).unwrap();
            assert_eq!(rx.push_character(b, at), Ok(ReceiveEvent::Pending));
        }
        let next = at.checked_add(t.inter_frame).unwrap().checked_add(t.character).unwrap();
        assert_eq!(rx.push_character(2, next), Ok(ReceiveEvent::Frame(adu)));
    }
}
