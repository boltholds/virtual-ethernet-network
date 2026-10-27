//! Datasheet-backed *timing constraints*, not calibrated circuit models.
//!
//! IMPORTANT: do not equate tPZH/tPZL and tPHZ/tPLZ pin transitions with
//! controller firmware DE-hold times, line propagation or UART bit skew.
//! Sources are pinned in docs/TRANSCEIVER_PROFILES.md.

use sim_time::SimDuration;
use ven_medium::rs485::Rs485Turnaround;
use crate::TransceiverProfile;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Part {
    /// Analog Devices ADM3485E (Rev D), 3.3 V at 25 C for timing table.
    Adm3485e,
    /// Texas Instruments THVD1450 (SLLSEY3E, Rev E), VCC 5 V typical values.
    Thvd1450,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Case {
    /// Manufacturer's typical value; not a guaranteed bound.
    Typical,
    /// Manufacturer's maximum under its specified test conditions.
    DatasheetMax,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiverStart {
    /// Receiver stays in the active operating mode.
    Active,
    /// Receiver enable after the documented shutdown/DE-low mode.
    /// May be orders of magnitude slower than an active receiver.
    FromShutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimingRange {
    pub typical_ns: u64,
    pub maximum_ns: u64,
}

impl TimingRange {
    pub const fn new(typical_ns: u64, maximum_ns: u64) -> Self {
        Self { typical_ns, maximum_ns }
    }

    pub fn select(self, case: Case) -> SimDuration {
        SimDuration::from_nanos(match case {
            Case::Typical => self.typical_ns,
            Case::DatasheetMax => self.maximum_ns,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DatasheetProfile {
    pub part: Part,
    pub name: &'static str,
    pub document: &'static str,
    pub max_signaling_bps: u64,
    pub driver_enable_active: TimingRange,
    pub driver_disable: TimingRange,
    pub receiver_enable_active: TimingRange,
    pub receiver_enable_from_shutdown: TimingRange,
    /// Stored as metadata; current digital PHY does not model independent
    /// DI->bus propagation delay or RO output propagation.
    pub driver_propagation: TimingRange,
    pub receiver_propagation: TimingRange,
    pub conditions: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileError {
    BaudExceedsDatasheet,
    DurationOverflow,
}

impl Part {
    pub const fn datasheet(self) -> DatasheetProfile {
        match self {
            Self::Adm3485e => DatasheetProfile {
                part: self,
                name: "Analog Devices ADM3485E",
                document: "https://www.analog.com/media/en/technical-documentation/data-sheets/ADM3485E.pdf",
                max_signaling_bps: 12_000_000,
                driver_enable_active: TimingRange::new(42, 90),
                driver_disable: TimingRange::new(35, 80),
                receiver_enable_active: TimingRange::new(25, 50),
                receiver_enable_from_shutdown: TimingRange::new(720, 1400),
                driver_propagation: TimingRange::new(23, 35),
                receiver_propagation: TimingRange::new(62, 90),
                conditions: "Rev D Table 2, VCC=3.3V, TA=25C; enable/disable RL=110ohm; RO timing CL=15pF. From-shutdown is a DIFFERENT mode.",
            },
            Self::Thvd1450 => DatasheetProfile {
                part: self,
                name: "Texas Instruments THVD1450",
                document: "https://www.ti.com/lit/ds/symlink/thvd1451.pdf",
                max_signaling_bps: 50_000_000,
                driver_enable_active: TimingRange::new(20, 50),
                driver_disable: TimingRange::new(15, 25),
                receiver_enable_active: TimingRange::new(50, 110),
                receiver_enable_from_shutdown: TimingRange::new(4000, 14000),
                driver_propagation: TimingRange::new(10, 20),
                receiver_propagation: TimingRange::new(25, 40),
                conditions: "SLLSEY3E section 7.8, VCC=5V typical at 25C, driver RL=54ohm CL=50pF; receiver CL=15pF. Driver active enable is for RE=0, and receiver active enable is for DE=VCC; DE=0 may cause long wake-up delays.",
            },
        }
    }
}

/// A *planning approximation*: translates datasheet transition bounds into
/// current VEN's three effective time guards, without modeling GPIO edges,
/// DE/RE logic sequence, DI propagation, receiver output propagation or cable.
/// A real board needs calibrated traces and explicit controller pin timing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControllerTiming {
    /// Delay after last stop bit until controller deasserts DE.
    pub de_hold: SimDuration,
    /// Delay between driver-release and when firmware makes RE available.
    pub rx_reenable_hold: SimDuration,
}

impl Default for ControllerTiming {
    fn default() -> Self {
        Self {
            de_hold: SimDuration::ZERO,
            rx_reenable_hold: SimDuration::ZERO,
        }
    }
}

impl DatasheetProfile {
    pub fn plan_digital_guard(
        self,
        baud: u64,
        choice: Case,
        receiver_start: ReceiverStart,
        controller: ControllerTiming,
    ) -> Result<TransceiverProfile, ProfileError> {
        if baud == 0 || baud > self.max_signaling_bps {
            return Err(ProfileError::BaudExceedsDatasheet);
        }
        let rx = match receiver_start {
            ReceiverStart::Active => self.receiver_enable_active.select(choice),
            ReceiverStart::FromShutdown => self.receiver_enable_from_shutdown.select(choice),
        };
        let de_hold = controller.de_hold
            .checked_add(self.driver_disable.select(choice))
            .ok_or(ProfileError::DurationOverflow)?;
        let rx_delay = controller.rx_reenable_hold
            .checked_add(rx).ok_or(ProfileError::DurationOverflow)?;
        Ok(TransceiverProfile {
            turnaround: Rs485Turnaround {
                driver_enable: self.driver_enable_active.select(choice),
                driver_release: de_hold,
                receiver_enable: rx_delay,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datasheet_values_are_distinct_not_invented_one_size_fits_all() {
        let adi = Part::Adm3485e.datasheet();
        let ti = Part::Thvd1450.datasheet();
        assert_eq!(adi.max_signaling_bps, 12_000_000);
        assert_eq!(ti.max_signaling_bps, 50_000_000);
        assert_eq!(adi.driver_enable_active, TimingRange::new(42, 90));
        assert_eq!(ti.driver_enable_active, TimingRange::new(20, 50));
        assert_eq!(ti.receiver_enable_from_shutdown.maximum_ns, 14_000);
    }

    #[test]
    fn effective_guards_include_firmware_and_device_bounds() {
        let bound = Part::Adm3485e.datasheet()
            .plan_digital_guard(
                9600, Case::DatasheetMax, ReceiverStart::Active,
                ControllerTiming {
                    de_hold: SimDuration::from_nanos(100),
                    rx_reenable_hold: SimDuration::from_nanos(200),
                },
            ).unwrap().turnaround;
        assert_eq!(bound.driver_enable.as_nanos(), 90);
        assert_eq!(bound.driver_release.as_nanos(), 180);
        assert_eq!(bound.receiver_enable.as_nanos(), 250);
    }

    #[test]
    fn shutdown_mode_is_not_conflated_with_active_receiver() {
        let part = Part::Thvd1450.datasheet();
        let active = part.plan_digital_guard(
            115_200, Case::DatasheetMax, ReceiverStart::Active,
            ControllerTiming::default(),
        ).unwrap();
        let wake = part.plan_digital_guard(
            115_200, Case::DatasheetMax, ReceiverStart::FromShutdown,
            ControllerTiming::default(),
        ).unwrap();
        assert_eq!(active.turnaround.receiver_enable.as_nanos(), 110);
        assert_eq!(wake.turnaround.receiver_enable.as_nanos(), 14_000);
        assert_eq!(part.plan_digital_guard(
            60_000_000, Case::Typical, ReceiverStart::Active,
            ControllerTiming::default(),
        ), Err(ProfileError::BaudExceedsDatasheet));
    }
}
