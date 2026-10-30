//! RS-485 two-wire shared segment at byte-frame LOD.
//! RS-485 itself does NOT specify UART data format, collision detection or Modbus.

use sim_time::{SimDuration, SimInstant};

const NANOS_PER_SECOND: u128 = 1_000_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rs485Error {
    InvalidBaudRate,
    InvalidUartFormat,
    EmptyTransmission,
    TimeOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Parity {
    None,
    Even,
    Odd,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UartFormat {
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: u8,
}

impl UartFormat {
    /// Typical MODBUS RTU byte format with 11 wire bits per character.
    pub const MODBUS_RTU_8E1: Self = Self { data_bits: 8, parity: Parity::Even, stop_bits: 1 };
    pub const MODBUS_RTU_8N2: Self = Self { data_bits: 8, parity: Parity::None, stop_bits: 2 };

    pub fn bits_per_character(self) -> Result<u8, Rs485Error> {
        if !(5..=9).contains(&self.data_bits) || !(1..=2).contains(&self.stop_bits) {
            return Err(Rs485Error::InvalidUartFormat);
        }
        let parity_bits = u8::from(self.parity != Parity::None);
        Ok(1 + self.data_bits + parity_bits + self.stop_bits)
    }
}

/// Explicit per-port RS-485 direction-switching delays (not Modbus timings).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rs485Turnaround {
    pub driver_enable: SimDuration,
    pub driver_release: SimDuration,
    pub receiver_enable: SimDuration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rs485TransmissionWindow {
    pub starts: SimInstant,
    pub finishes: SimInstant,
    pub arrives: SimInstant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rs485Medium {
    baud_rate: u64,
    uart: UartFormat,
    propagation: SimDuration,
}

impl Rs485Medium {
    pub fn new(baud_rate: u64, uart: UartFormat, propagation: SimDuration) -> Result<Self, Rs485Error> {
        if baud_rate == 0 {
            return Err(Rs485Error::InvalidBaudRate);
        }
        uart.bits_per_character()?;
        Ok(Self { baud_rate, uart, propagation })
    }

    /// Completion of the Nth character, using cumulative bit arithmetic so
    /// rounding does not drift by one nanosecond per byte.
    pub fn character_arrival(&self, count: usize, now: SimInstant) -> Result<SimInstant, Rs485Error> {
        if count == 0 { return Err(Rs485Error::EmptyTransmission); }
        let bits = (count as u128)
            .checked_mul(self.uart.bits_per_character()? as u128)
            .ok_or(Rs485Error::TimeOverflow)?;
        let numerator = bits.checked_mul(NANOS_PER_SECOND).ok_or(Rs485Error::TimeOverflow)?;
        let denominator = self.baud_rate as u128;
        let ns = numerator / denominator + u128::from(numerator % denominator != 0);
        let ns = u64::try_from(ns).map_err(|_| Rs485Error::TimeOverflow)?;
        now.checked_add(SimDuration::from_nanos(ns))
            .and_then(|time| time.checked_add(self.propagation))
            .ok_or(Rs485Error::TimeOverflow)
    }

    /// Enable driver at `now`; start data after driver_enable; release
    /// following the final stop bit; re-enable RX after receiver_enable.
    pub fn with_turnaround(
        &self, bytes: usize, now: SimInstant, delay: Rs485Turnaround,
    ) -> Result<(Rs485TransmissionWindow, SimInstant, SimInstant), Rs485Error> {
        let starts = now.checked_add(delay.driver_enable).ok_or(Rs485Error::TimeOverflow)?;
        let tx = self.transmission(bytes, starts)?;
        let released = tx.finishes.checked_add(delay.driver_release).ok_or(Rs485Error::TimeOverflow)?;
        let ready = released.checked_add(delay.receiver_enable).ok_or(Rs485Error::TimeOverflow)?;
        Ok((tx, released, ready))
    }

    /// Models one uninterrupted UART character stream starting NOW. No bus
    /// arbitration or automatic queueing: simultaneous drivers are possible.
    pub fn transmission(&self, bytes: usize, now: SimInstant) -> Result<Rs485TransmissionWindow, Rs485Error> {
        if bytes == 0 {
            return Err(Rs485Error::EmptyTransmission);
        }
        let bits = (bytes as u128)
            .checked_mul(self.uart.bits_per_character()? as u128)
            .ok_or(Rs485Error::TimeOverflow)?;
        let numerator = bits.checked_mul(NANOS_PER_SECOND).ok_or(Rs485Error::TimeOverflow)?;
        let denominator = self.baud_rate as u128;
        let nanos = numerator / denominator + u128::from(numerator % denominator != 0);
        let nanos = u64::try_from(nanos).map_err(|_| Rs485Error::TimeOverflow)?;
        let finishes = now.checked_add(SimDuration::from_nanos(nanos)).ok_or(Rs485Error::TimeOverflow)?;
        let arrives = finishes.checked_add(self.propagation).ok_or(Rs485Error::TimeOverflow)?;
        Ok(Rs485TransmissionWindow { starts: now, finishes, arrives })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uart_overhead_is_counted_not_eight_bits_per_byte() {
        let bus = Rs485Medium::new(9600, UartFormat::MODBUS_RTU_8E1, SimDuration::ZERO).unwrap();
        let window = bus.transmission(1, SimInstant::ZERO).unwrap();
        assert_eq!(window.finishes.as_nanos(), 1_145_834);
        assert_eq!(UartFormat::MODBUS_RTU_8N2.bits_per_character(), Ok(11));
    }

    #[test]
    fn cumulative_character_arrivals_are_exactly_rounded() {
        let bus = Rs485Medium::new(9600, UartFormat::MODBUS_RTU_8E1, SimDuration::ZERO).unwrap();
        assert_eq!(bus.character_arrival(1, SimInstant::ZERO).unwrap().as_nanos(), 1_145_834);
        assert_eq!(bus.character_arrival(3, SimInstant::ZERO).unwrap().as_nanos(), 3_437_500);
        assert_eq!(bus.character_arrival(3, SimInstant::ZERO).unwrap(),
                   bus.transmission(3, SimInstant::ZERO).unwrap().arrives);
    }

    #[test]
    fn rejects_invalid_configuration_and_overflow() {
        assert_eq!(Rs485Medium::new(0, UartFormat::MODBUS_RTU_8E1, SimDuration::ZERO), Err(Rs485Error::InvalidBaudRate));
        assert_eq!(UartFormat { data_bits: 8, parity: Parity::Even, stop_bits: 0 }.bits_per_character(), Err(Rs485Error::InvalidUartFormat));
        let bus = Rs485Medium::new(1, UartFormat::MODBUS_RTU_8N2, SimDuration::ZERO).unwrap();
        assert_eq!(bus.transmission(1, SimInstant::from_nanos(u64::MAX)), Err(Rs485Error::TimeOverflow));
        assert_eq!(bus.transmission(0, SimInstant::ZERO), Err(Rs485Error::EmptyTransmission));
    }
}
