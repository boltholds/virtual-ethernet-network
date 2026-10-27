//! Canonical oscilloscope CSV ingestion and *sample-point* comparison.
//!
//! Requires real measured waveform input for validation. Synthetic fixtures
//! only validate this comparator; they do not validate hardware fidelity.
//!
//! Canonical CSV: time_ns,channel,millivolts
//! Possible channels: DE, RE, DI, RO, A, B. Differential A-B must be
//! precomputed explicitly or imported as a DIFF channel.

use sim_time::{SimDuration, SimInstant};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Channel { De, Re, Di, Ro, A, B, Diff }

impl Channel {
    fn parse(value: &str) -> Result<Self, TraceError> {
        match value {
            "DE" => Ok(Self::De),
            "RE" => Ok(Self::Re),
            "DI" => Ok(Self::Di),
            "RO" => Ok(Self::Ro),
            "A" => Ok(Self::A),
            "B" => Ok(Self::B),
            "DIFF" => Ok(Self::Diff),
            _ => Err(TraceError::UnknownChannel),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VoltageSample {
    pub at: SimInstant,
    pub channel: Channel,
    pub millivolts: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Thresholds {
    pub low_mv: i64,
    pub high_mv: i64,
}

impl Thresholds {
    pub fn logic(self, mv: i64) -> Level {
        if mv <= self.low_mv { Level::Low }
        else if mv >= self.high_mv { Level::High }
        else { Level::Uncertain }
    }

    pub fn validate(self) -> Result<Self, TraceError> {
        if self.high_mv <= self.low_mv { return Err(TraceError::InvalidThresholds); }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Level { Low, High, Uncertain }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BitExpectation {
    /// Relative to a separately captured DE assert trigger, NOT first RO edge.
    pub after_trigger: SimDuration,
    pub level: Level,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SampleFinding {
    Match,
    Mismatch { expected: Level, observed: Level },
    MissingMeasurement,
    UncertainMeasurement,
    UncertainPrediction,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Comparison {
    pub matched: usize,
    pub mismatched: usize,
    pub missing: usize,
    pub uncertain_measured: usize,
    pub uncertain_predicted: usize,
}

impl Comparison {
    pub fn all_matched(self, expected_count: usize) -> bool {
        expected_count != 0 && self.matched == expected_count
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceError {
    Empty,
    BadHeader,
    BadRecord,
    UnknownChannel,
    NonmonotonicSamples,
    InvalidThresholds,
    MissingTrigger,
    TriggerNotRising,
    TimeOverflow,
}

#[derive(Clone, Debug)]
pub struct ScopeTrace {
    samples: Vec<VoltageSample>,
}

impl ScopeTrace {
    pub fn from_canonical_csv(csv: &str) -> Result<Self, TraceError> {
        let mut records = csv.lines().map(str::trim).filter(|s| !s.is_empty());
        if records.next() != Some("time_ns,channel,millivolts") {
            return Err(TraceError::BadHeader);
        }
        let mut samples: Vec<VoltageSample> = Vec::new();
        let mut last_per_channel = std::collections::BTreeMap::<Channel, SimInstant>::new();
        for row in records {
            let parts: Vec<_> = row.split(',').collect();
            if parts.len() != 3 { return Err(TraceError::BadRecord); }
            let time = SimInstant::from_nanos(parts[0].trim().parse()
                .map_err(|_| TraceError::BadRecord)?);
            let channel = Channel::parse(parts[1].trim())?;
            let voltage = parts[2].trim().parse().map_err(|_| TraceError::BadRecord)?;
            if last_per_channel.get(&channel).is_some_and(|&last| time <= last) {
                return Err(TraceError::NonmonotonicSamples);
            }
            last_per_channel.insert(channel, time);
            samples.push(VoltageSample { at: time, channel, millivolts: voltage });
        }
        if samples.is_empty() { return Err(TraceError::Empty); }
        samples.sort_by_key(|x| x.at);
        Ok(Self { samples })
    }

    pub fn trigger_rising(&self, channel: Channel, thresholds: Thresholds) -> Result<SimInstant, TraceError> {
        let thresholds = thresholds.validate()?;
        let mut previous = Level::Uncertain;
        for entry in &self.samples {
            if entry.channel != channel { continue; }
            let next = thresholds.logic(entry.millivolts);
            if next == Level::High && previous == Level::Low {
                return Ok(entry.at);
            }
            previous = next;
        }
        Err(TraceError::MissingTrigger)
    }

    /// Sample-and-hold at a virtual offset after the shared DE trigger.
    /// max_sample_age accounts for actual oscilloscope sampling resolution.
    pub fn sample_at(
        &self, channel: Channel, at: SimInstant, max_sample_age: SimDuration,
        thresholds: Thresholds,
    ) -> Result<SampleReading, TraceError> {
        let thresholds = thresholds.validate()?;
        let sample = self.samples.iter().rev()
            .find(|row| row.channel == channel && row.at <= at);
        match sample {
            Some(row) => {
                let age = at.checked_duration_since(row.at).ok_or(TraceError::TimeOverflow)?;
                if age > max_sample_age { return Ok(SampleReading::Missing); }
                Ok(SampleReading::Level(thresholds.logic(row.millivolts)))
            }
            None => Ok(SampleReading::Missing),
        }
    }

    /// Compare at *known bit-center timestamps*, retaining the DE trigger
    /// as the common origin. We do not independently align receiver edges.
    pub fn compare_bits(
        &self,
        trigger: SimInstant,
        channel: Channel,
        thresholds: Thresholds,
        max_sample_age: SimDuration,
        expected: &[BitExpectation],
    ) -> Result<(Comparison, Vec<SampleFinding>), TraceError> {
        let mut summary = Comparison::default();
        let mut findings = Vec::with_capacity(expected.len());
        for point in expected {
            let at = trigger.checked_add(point.after_trigger)
                .ok_or(TraceError::TimeOverflow)?;
            let finding = match (point.level,
                self.sample_at(channel, at, max_sample_age, thresholds)?
            ) {
                (Level::Uncertain, _) => {
                    summary.uncertain_predicted += 1;
                    SampleFinding::UncertainPrediction
                }
                (_, SampleReading::Missing) => {
                    summary.missing += 1;
                    SampleFinding::MissingMeasurement
                }
                (_, SampleReading::Level(Level::Uncertain)) => {
                    summary.uncertain_measured += 1;
                    SampleFinding::UncertainMeasurement
                }
                (expected, SampleReading::Level(observed)) if expected == observed => {
                    summary.matched += 1;
                    SampleFinding::Match
                }
                (expected, SampleReading::Level(observed)) => {
                    summary.mismatched += 1;
                    SampleFinding::Mismatch { expected, observed }
                }
            };
            findings.push(finding);
        }
        Ok((summary, findings))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SampleReading {
    Missing,
    Level(Level),
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYNTHETIC: &str = "time_ns,channel,millivolts\n\
        0,DE,0\n\
        0,RO,3300\n\
        100,DE,3300\n\
        200,RO,0\n\
        400,RO,3300\n\
        600,RO,0\n";

    fn th() -> Thresholds { Thresholds { low_mv: 800, high_mv: 2000 } }

    #[test]
    fn trigger_is_de_not_a_fitted_receiver_transition() {
        let trace = ScopeTrace::from_canonical_csv(SYNTHETIC).unwrap();
        let trigger = trace.trigger_rising(Channel::De, th()).unwrap();
        assert_eq!(trigger.as_nanos(), 100);
        let points = [
            BitExpectation { after_trigger: SimDuration::from_nanos(150), level: Level::Low },
            BitExpectation { after_trigger: SimDuration::from_nanos(350), level: Level::High },
        ];
        let (summary, _) = trace.compare_bits(
            trigger, Channel::Ro, th(), SimDuration::from_nanos(150), &points,
        ).unwrap();
        assert_eq!(summary.matched, 2);
        assert!(summary.all_matched(2));
    }

    #[test]
    fn a_wrong_hardware_delay_is_reported_not_auto_aligned_away() {
        let trace = ScopeTrace::from_canonical_csv(SYNTHETIC).unwrap();
        let trigger = trace.trigger_rising(Channel::De, th()).unwrap();
        let point = [BitExpectation {
            after_trigger: SimDuration::from_nanos(150), level: Level::High,
        }];
        let (report, _) = trace.compare_bits(
            trigger, Channel::Ro, th(), SimDuration::from_nanos(150), &point,
        ).unwrap();
        assert_eq!(report.mismatched, 1);
        assert_eq!(report.matched, 0);
    }

    #[test]
    fn rejects_nonmonotonic_and_unknown_channels() {
        assert_eq!(
            ScopeTrace::from_canonical_csv("time_ns,channel,millivolts\n2,RO,1\n1,RO,0\n")
                .unwrap_err(),
            TraceError::NonmonotonicSamples
        );
        assert_eq!(
            ScopeTrace::from_canonical_csv("time_ns,channel,millivolts\n0,NOT_A_CHANNEL,123\n")
                .unwrap_err(),
            TraceError::UnknownChannel
        );
    }

    #[test]
    fn absent_measurement_and_indeterminate_voltage_are_not_passes() {
        let trace = ScopeTrace::from_canonical_csv(
            "time_ns,channel,millivolts\n0,DE,0\n100,DE,3300\n100,RO,1300\n"
        ).unwrap();
        let trigger = trace.trigger_rising(Channel::De, th()).unwrap();
        let points = [
            BitExpectation { after_trigger: SimDuration::ZERO, level: Level::High },
            BitExpectation { after_trigger: SimDuration::from_nanos(500), level: Level::Low },
        ];
        let (report, _) = trace.compare_bits(
            trigger, Channel::Ro, th(), SimDuration::from_nanos(100), &points,
        ).unwrap();
        assert_eq!(report.uncertain_measured, 1);
        assert_eq!(report.missing, 1);
        assert!(!report.all_matched(2));
    }
}
