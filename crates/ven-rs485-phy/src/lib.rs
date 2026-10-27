//! Opt-in RS-485 **digital** PHY at UART-bit granularity.
//!
//! A logic-level model, NOT an EIA/TIA-485 electrical circuit solver:
//! there is no impedance, voltage, common-mode, reflections or overload.
//! Multiple enabled drivers conservatively resolve to Indeterminate even
//! when they happen to drive the same logic value.

pub mod profiles;
pub mod trace;

use sim_kernel::{AdvanceOutcome, SimulationParticipant, TimeGrant, TimedOutput};
use sim_time::{SimDuration, SimInstant};
use std::collections::BTreeMap;
use std::fmt;
use ven_core::PortId;
use ven_medium::rs485::{Parity, Rs485Turnaround, UartFormat};
use ven_scheduler::{NetworkScheduler, ScheduleError};
use ven_topology::BusId;

const NANOS_PER_SEC: u128 = 1_000_000_000;
const MAX_UART_BYTES: usize = 256;
const MAX_BAUD: u64 = 100_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdleLine {
    /// A configured effective bias/failsafe assumption, not proof of a resistor network.
    High,
    /// No driver, no guaranteed failsafe behavior.
    Floating,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogicLevel {
    High,
    Low,
    Indeterminate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UartSample {
    Byte(u8),
    ParityError,
    FramingError,
    Indeterminate,
    ReceiverDisabled,
}

/// Deterministic fault at a single receiver bit-sampling instant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SampleFault {
    Flip,
    ForceIndeterminate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PhysicalEvent {
    /// Digital contention diagnostic; physical devices are not assumed to detect this.
    Contention { bus: BusId, first: PortId, second: PortId },
    DriverReleased { bus: BusId, port: PortId },
    SampledBit {
        from: PortId,
        to: PortId,
        character: usize,
        bit: usize,
        level: LogicLevel,
    },
    Character {
        from: PortId,
        to: PortId,
        character: usize,
        sample: UartSample,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyError {
    DuplicatePort,
    UnknownPort,
    InvalidBaudRate,
    UnsupportedBaudRate,
    InvalidUartFormat,
    EmptyFrame,
    TooManyBytes,
    InvalidBitFault,
    TransmitterBusy,
    TimeOverflow,
    TimeRegression,
    TransmissionIdExhausted,
    Schedule(ScheduleError),
}

impl fmt::Display for PhyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for PhyError {}

/// One *generic* transceiver's timing parameters. No vendor defaults inferred.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TransceiverProfile {
    pub turnaround: Rs485Turnaround,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransmissionTiming {
    pub data_starts: SimInstant,
    pub data_finishes: SimInstant,
    pub driver_released: SimInstant,
    pub receiver_ready: SimInstant,
}

#[derive(Clone, Debug)]
struct Transmission {
    sender: PortId,
    asserted: SimInstant,
    timing: TransmissionTiming,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BitReading {
    Level(LogicLevel),
    Muted,
}

#[derive(Clone, Debug)]
enum LocalEvent {
    Contention { first: PortId, second: PortId },
    Sample {
        txid: u64,
        to: PortId,
        character: usize,
        bit: usize,
    },
    CharacterComplete {
        txid: u64,
        to: PortId,
        character: usize,
    },
    DriverReleased { port: PortId },
    Retire(u64),
}

/// A single multidrop bus as one host-time participant.
/// This is an opt-in alternative to VEN's faster character/frame-level RS-485.
#[derive(Debug)]
pub struct Rs485DigitalPhy {
    bus: BusId,
    now: SimInstant,
    baud: u64,
    uart: UartFormat,
    bits_per_char: usize,
    propagation: SimDuration,
    idle: IdleLine,
    ports: BTreeMap<PortId, TransceiverProfile>,
    driver_busy_until: BTreeMap<PortId, SimInstant>,
    receiver_ready_at: BTreeMap<PortId, SimInstant>,
    transmissions: BTreeMap<u64, Transmission>,
    samples: BTreeMap<(u64, PortId, usize), Vec<BitReading>>,
    bit_faults: BTreeMap<(PortId, usize, usize), SampleFault>,
    next_txid: u64,
    events: NetworkScheduler<LocalEvent>,
    trace_bits: bool,
}

impl Rs485DigitalPhy {
    pub fn new(
        bus: BusId,
        baud: u64,
        uart: UartFormat,
        propagation: SimDuration,
        idle: IdleLine,
    ) -> Result<Self, PhyError> {
        if baud == 0 { return Err(PhyError::InvalidBaudRate); }
        // At the current 1 ns resolution, reject bit cells too narrow to sample.
        if baud > MAX_BAUD { return Err(PhyError::UnsupportedBaudRate); }
        if uart.data_bits != 8 { return Err(PhyError::InvalidUartFormat); }
        let bits = uart.bits_per_character().map_err(|_| PhyError::InvalidUartFormat)? as usize;
        Ok(Self {
            bus, now: SimInstant::ZERO, baud, uart, bits_per_char: bits,
            propagation, idle, ports: BTreeMap::new(),
            driver_busy_until: BTreeMap::new(),
            receiver_ready_at: BTreeMap::new(),
            transmissions: BTreeMap::new(),
            samples: BTreeMap::new(), next_txid: 0,
            events: NetworkScheduler::new(), trace_bits: false,
            bit_faults: BTreeMap::new(),
        })
    }

    pub fn add_port(&mut self, port: PortId, profile: TransceiverProfile) -> Result<(), PhyError> {
        if self.ports.contains_key(&port) { return Err(PhyError::DuplicatePort); }
        self.ports.insert(port, profile);
        Ok(())
    }

    pub fn set_trace_bits(&mut self, enabled: bool) {
        self.trace_bits = enabled;
    }

    /// Inject a one-shot *receiver sampling fault*, not a voltage/waveform model.
    /// Triggered at the next matching (port, character index, bit index).
    pub fn inject_sample_fault(
        &mut self, receiver: PortId, character: usize, bit: usize, fault: SampleFault,
    ) -> Result<(), PhyError> {
        if !self.ports.contains_key(&receiver) { return Err(PhyError::UnknownPort); }
        if bit >= self.bits_per_char { return Err(PhyError::InvalidBitFault); }
        self.bit_faults.insert((receiver, character, bit), fault);
        Ok(())
    }

    pub fn bus_id(&self) -> BusId { self.bus }
    pub fn baud_rate(&self) -> u64 { self.baud }
    pub fn uart_format(&self) -> UartFormat { self.uart }
    pub fn has_port(&self, port: PortId) -> bool { self.ports.contains_key(&port) }

    pub fn port_count(&self) -> usize { self.ports.len() }
    pub fn active_transmission_count(&self) -> usize { self.transmissions.len() }
    pub fn driver_busy_until(&self, port: PortId) -> Option<SimInstant> {
        self.driver_busy_until.get(&port).copied()
    }

    /// An idealized logical sample at the source-wire time, before propagation.
    pub fn observe_wire(&self, at: SimInstant) -> LogicLevel {
        let mut driver = None;
        for tx in self.transmissions.values() {
            if tx.asserted <= at && at < tx.timing.driver_released {
                let level = self.transmission_bit_at(tx, at);
                if driver.is_some() { return LogicLevel::Indeterminate; }
                driver = Some(level);
            }
        }
        driver.unwrap_or(match self.idle {
            IdleLine::High => LogicLevel::High,
            IdleLine::Floating => LogicLevel::Indeterminate,
        })
    }

    fn transmission_bit_at(&self, tx: &Transmission, at: SimInstant) -> LogicLevel {
        if at < tx.timing.data_starts || at >= tx.timing.data_finishes {
            return LogicLevel::High; // DE active, UART idle mark
        }
        let elapsed = u128::from(at.as_nanos() - tx.timing.data_starts.as_nanos());
        let global_bit = (elapsed * u128::from(self.baud) / NANOS_PER_SEC) as usize;
        let byte = tx.bytes[global_bit / self.bits_per_char];
        let pos = global_bit % self.bits_per_char;
        if pos == 0 { return LogicLevel::Low; } // UART start
        if pos <= 8 {
            return if byte & (1 << (pos - 1)) != 0 {
                LogicLevel::High
            } else {
                LogicLevel::Low
            };
        }
        if self.uart.parity != Parity::None && pos == 9 {
            let odd_data = byte.count_ones() % 2 == 1;
            let parity_high = match self.uart.parity {
                Parity::Even => odd_data,
                Parity::Odd => !odd_data,
                Parity::None => false,
            };
            return if parity_high { LogicLevel::High } else { LogicLevel::Low };
        }
        LogicLevel::High // stop bits
    }

    fn ceil_ratio(numerator: u128, denominator: u128) -> Result<u64, PhyError> {
        let ns = numerator / denominator + u128::from(numerator % denominator != 0);
        u64::try_from(ns).map_err(|_| PhyError::TimeOverflow)
    }

    fn boundary_duration(&self, bit_index: usize) -> Result<SimDuration, PhyError> {
        let numerator = (bit_index as u128).checked_mul(NANOS_PER_SEC)
            .ok_or(PhyError::TimeOverflow)?;
        Ok(SimDuration::from_nanos(Self::ceil_ratio(numerator, u128::from(self.baud))?))
    }

    fn sample_duration(&self, bit_index: usize) -> Result<SimDuration, PhyError> {
        // A center-of-bit sample. Both halves are represented in integer ns.
        let numerator = (bit_index as u128).checked_mul(2)
            .and_then(|two| two.checked_add(1))
            .and_then(|odd| odd.checked_mul(NANOS_PER_SEC))
            .ok_or(PhyError::TimeOverflow)?;
        Ok(SimDuration::from_nanos(
            Self::ceil_ratio(numerator, 2 * u128::from(self.baud))?
        ))
    }

    /// Schedule every receiver's bit samples and UART byte-completion reports.
    /// Input is intentionally bounded to prevent unbounded O(ports * bits) work.
    pub fn transmit(&mut self, from: PortId, bytes: Vec<u8>) -> Result<TransmissionTiming, PhyError> {
        let profile = *self.ports.get(&from).ok_or(PhyError::UnknownPort)?;
        if bytes.is_empty() { return Err(PhyError::EmptyFrame); }
        if bytes.len() > MAX_UART_BYTES { return Err(PhyError::TooManyBytes); }
        if self.driver_busy_until.get(&from).is_some_and(|until| *until > self.now) {
            return Err(PhyError::TransmitterBusy);
        }

        let data_starts = self.now.checked_add(profile.turnaround.driver_enable)
            .ok_or(PhyError::TimeOverflow)?;
        let total_bits = bytes.len().checked_mul(self.bits_per_char)
            .ok_or(PhyError::TimeOverflow)?;
        let data_finishes = data_starts.checked_add(self.boundary_duration(total_bits)?)
            .ok_or(PhyError::TimeOverflow)?;
        let driver_released = data_finishes.checked_add(profile.turnaround.driver_release)
            .ok_or(PhyError::TimeOverflow)?;
        let receiver_ready = driver_released.checked_add(profile.turnaround.receiver_enable)
            .ok_or(PhyError::TimeOverflow)?;
        let timing = TransmissionTiming {
            data_starts, data_finishes, driver_released, receiver_ready,
        };
        let id = self.next_txid;
        let next = id.checked_add(1).ok_or(PhyError::TransmissionIdExhausted)?;

        // Transactional scheduling: no partial event insertion if an offset overflows.
        let mut pending = self.events.clone();
        for active in self.transmissions.values() {
            if active.asserted <= self.now && self.now < active.timing.driver_released {
                pending.schedule_at(self.now, LocalEvent::Contention {
                    first: active.sender, second: from,
                }).map_err(PhyError::Schedule)?;
            }
        }

        for &to in self.ports.keys() {
            if to == from { continue; } // local echo is not assumed
            for byte_index in 0..bytes.len() {
                for bit in 0..self.bits_per_char {
                    let global_bit = byte_index * self.bits_per_char + bit;
                    let sample = data_starts.checked_add(self.sample_duration(global_bit)?)
                        .and_then(|at| at.checked_add(self.propagation))
                        .ok_or(PhyError::TimeOverflow)?;
                    pending.schedule_at(sample, LocalEvent::Sample {
                        txid: id, to, character: byte_index, bit,
                    }).map_err(PhyError::Schedule)?;
                }
                let end_bit = (byte_index + 1) * self.bits_per_char;
                let finished = data_starts.checked_add(self.boundary_duration(end_bit)?)
                    .and_then(|at| at.checked_add(self.propagation))
                    .ok_or(PhyError::TimeOverflow)?;
                pending.schedule_at(finished, LocalEvent::CharacterComplete {
                    txid: id, to, character: byte_index,
                }).map_err(PhyError::Schedule)?;
            }
        }
        pending.schedule_at(driver_released, LocalEvent::DriverReleased { port: from })
            .map_err(PhyError::Schedule)?;
        let retire_at = driver_released.checked_add(self.propagation)
            .ok_or(PhyError::TimeOverflow)?;
        pending.schedule_at(retire_at, LocalEvent::Retire(id))
            .map_err(PhyError::Schedule)?;
        self.events = pending;
        self.next_txid = next;
        self.driver_busy_until.insert(from, driver_released);
        self.receiver_ready_at.insert(from, receiver_ready);
        self.transmissions.insert(id, Transmission {
            sender: from, asserted: self.now, timing, bytes,
        });
        Ok(timing)
    }

    fn decode_character(&self, bits: &[BitReading]) -> UartSample {
        if bits.iter().any(|bit| matches!(bit, BitReading::Muted)) {
            return UartSample::ReceiverDisabled;
        }
        if bits.len() != self.bits_per_char {
            return UartSample::Indeterminate;
        }
        let mut decoded = Vec::with_capacity(bits.len());
        for bit in bits {
            match bit {
                BitReading::Level(LogicLevel::High) => decoded.push(true),
                BitReading::Level(LogicLevel::Low) => decoded.push(false),
                _ => return UartSample::Indeterminate,
            }
        }
        let parity_pos = if self.uart.parity == Parity::None { 0 } else { 1 };
        let stop_start = 9 + parity_pos;
        if decoded[0] || decoded[stop_start..].iter().any(|&value| !value) {
            return UartSample::FramingError;
        }
        let mut byte = 0u8;
        for index in 0..8 {
            if decoded[index + 1] { byte |= 1 << index; }
        }
        if parity_pos == 1 {
            let data_odd = byte.count_ones() % 2 == 1;
            let expected = match self.uart.parity {
                Parity::Even => data_odd,
                Parity::Odd => !data_odd,
                Parity::None => false,
            };
            if decoded[9] != expected { return UartSample::ParityError; }
        }
        UartSample::Byte(byte)
    }
}

impl SimulationParticipant for Rs485DigitalPhy {
    type Output = PhysicalEvent;
    type Error = PhyError;

    fn current_time(&self) -> SimInstant { self.now }
    fn next_wakeup(&self) -> Option<SimInstant> { self.events.next_time() }

    fn advance(&mut self, grant: TimeGrant) -> Result<AdvanceOutcome<Self::Output>, Self::Error> {
        if grant.until < self.now { return Err(PhyError::TimeRegression); }
        let at = self.events.next_time()
            .filter(|&next| next <= grant.until).unwrap_or(grant.until);
        let mut outputs = Vec::new();
        while let Some(event) = self.events.pop_through(at) {
            match event.event {
                LocalEvent::Contention { first, second } => outputs.push(TimedOutput {
                    at: event.at,
                    value: PhysicalEvent::Contention { bus: self.bus, first, second },
                }),
                LocalEvent::DriverReleased { port } => outputs.push(TimedOutput {
                    at: event.at,
                    value: PhysicalEvent::DriverReleased { bus: self.bus, port },
                }),
                LocalEvent::Sample { txid, to, character, bit } => {
                    let sender = self.transmissions.get(&txid).expect("transmission retained").sender;
                    let muted = self.receiver_ready_at.get(&to)
                        .is_some_and(|&ready| event.at < ready);
                    let mut level = if muted {
                        BitReading::Muted
                    } else {
                        let wire_at = event.at.checked_sub_duration(self.propagation)
                            .ok_or(PhyError::TimeRegression)?;
                        BitReading::Level(self.observe_wire(wire_at))
                    };
                    if let Some(fault) = self.bit_faults.remove(&(to, character, bit)) {
                        level = match (level, fault) {
                            (BitReading::Level(LogicLevel::High), SampleFault::Flip) =>
                                BitReading::Level(LogicLevel::Low),
                            (BitReading::Level(LogicLevel::Low), SampleFault::Flip) =>
                                BitReading::Level(LogicLevel::High),
                            (BitReading::Level(_), SampleFault::ForceIndeterminate) =>
                                BitReading::Level(LogicLevel::Indeterminate),
                            (original, _) => original,
                        };
                    }
                    if self.trace_bits {
                        outputs.push(TimedOutput {
                            at: event.at,
                            value: PhysicalEvent::SampledBit {
                                from: sender, to, character, bit,
                                level: match level {
                                    BitReading::Muted => LogicLevel::Indeterminate,
                                    BitReading::Level(l) => l,
                                },
                            },
                        });
                    }
                    self.samples.entry((txid, to, character)).or_default().push(level);
                }
                LocalEvent::CharacterComplete { txid, to, character } => {
                    let sender = self.transmissions.get(&txid).expect("transmission retained").sender;
                    let samples = self.samples.remove(&(txid, to, character)).unwrap_or_default();
                    let sample = self.decode_character(&samples);
                    outputs.push(TimedOutput {
                        at: event.at,
                        value: PhysicalEvent::Character { from: sender, to, character, sample },
                    });
                }
                LocalEvent::Retire(id) => { self.transmissions.remove(&id); }
            }
        }
        self.now = at;
        Ok(AdvanceOutcome {
            reached: self.now, outputs, next_wakeup: self.events.next_time(),
        })
    }
}
