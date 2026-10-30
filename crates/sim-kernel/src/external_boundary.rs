//! Transport-neutral boundary for external PLC runtimes and CAD geometry.
//!
//! This is NOT an implementation of PLC Web's Python ScheduledRuntime, nor
//! an equipment dynamics solver. Both are external implementations which
//! must explicitly opt into this schema/contract.

use sim_time::{SimDuration, SimInstant};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FiniteReal(u64);

impl FiniteReal {
    pub fn new(value: f64) -> Result<Self, BoundaryError> {
        if !value.is_finite() { return Err(BoundaryError::NonFiniteReal); }
        // Canonicalize negative zero for deterministic compare and hashing.
        let normalized = if value == 0.0 { 0.0 } else { value };
        Ok(Self(normalized.to_bits()))
    }
    pub fn get(self) -> f64 { f64::from_bits(self.0) }
}

/// Absence is explicit, never conflated with BOOL=false or REAL=0.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SignalValue {
    Bool(bool),
    Integer(i64),
    Real(FiniteReal),
    Text(String),
    Unavailable,
}

/// Cross-boundary symbol identity, distinct from a rendered CAD entity id.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct SignalKey(String);

impl SignalKey {
    pub fn new(name: impl Into<String>) -> Result<Self, BoundaryError> {
        let name = name.into();
        if name.is_empty() || name.trim() != name || name.chars().any(char::is_control) {
            return Err(BoundaryError::InvalidSignalKey);
        }
        Ok(Self(name))
    }
    pub fn as_str(&self) -> &str { &self.0 }
}

/// Time conversion is exact for integer-millisecond PLC scans.
pub fn scan_period_ms(ms: u32) -> Result<SimDuration, BoundaryError> {
    if ms == 0 { return Err(BoundaryError::ZeroScanPeriod); }
    Ok(SimDuration::from_nanos(u64::from(ms) * 1_000_000))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlcScanInputs {
    pub apply_at: SimInstant,
    pub period: SimDuration,
    pub signals: BTreeMap<SignalKey, SignalValue>,
}

impl PlcScanInputs {
    /// Current Python ScheduledRuntime.scan accepts only BOOL, INT, REAL
    /// inputs; STR/Unavailable are not silently coerced.
    pub fn validate_legacy_python_inputs(&self) -> Result<(), BoundaryError> {
        if self.period == SimDuration::ZERO { return Err(BoundaryError::ZeroScanPeriod); }
        for value in self.signals.values() {
            if matches!(value, SignalValue::Text(_) | SignalValue::Unavailable) {
                return Err(BoundaryError::UnsupportedPlcInput);
            }
        }
        Ok(())
    }
}

/// A committed scan, not a changing signal halfway through a PLC task.
/// Engine is responsible for state retention and task execution order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlcScanCommit {
    pub scan_number: u64,
    pub completed_at: SimInstant,
    pub outputs: BTreeMap<SignalKey, SignalValue>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundaryError {
    NonFiniteReal,
    InvalidSignalKey,
    ZeroScanPeriod,
    UnsupportedPlcInput,
    ScanSequenceMismatch,
    ScanCompletionTimeMismatch,
    TimeOverflow,
    StaleGeometryRevision,
    InvalidGeometryIdentity,
}

/// Do not infer a millisecond PLC scan from nanosecond network wakeups.
/// A cycle is committed atomically at its *declared* scan deadline.
pub fn validate_scan_commit(
    previous_scan: u64,
    last_completed_at: SimInstant,
    period: SimDuration,
    committed: &PlcScanCommit,
) -> Result<(), BoundaryError> {
    if period == SimDuration::ZERO { return Err(BoundaryError::ZeroScanPeriod); }
    let expected_number = previous_scan.checked_add(1)
        .ok_or(BoundaryError::ScanSequenceMismatch)?;
    if committed.scan_number != expected_number {
        return Err(BoundaryError::ScanSequenceMismatch);
    }
    let expected_at = last_completed_at.checked_add(period)
        .ok_or(BoundaryError::TimeOverflow)?;
    if committed.completed_at != expected_at {
        return Err(BoundaryError::ScanCompletionTimeMismatch);
    }
    Ok(())
}

/// Gefest CAD's document revision is a *geometry snapshot* identity, not
/// a device process model and not a clock tick.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeometrySnapshotRef {
    pub project_id: String,
    pub scene_revision: u64,
    pub entity_id: String,
}

impl GeometrySnapshotRef {
    pub fn new(
        project_id: impl Into<String>, scene_revision: u64, entity_id: impl Into<String>,
    ) -> Result<Self, BoundaryError> {
        let project_id = project_id.into();
        let entity_id = entity_id.into();
        if project_id.trim().is_empty() || entity_id.trim().is_empty()
            || project_id.trim() != project_id || entity_id.trim() != entity_id
        {
            return Err(BoundaryError::InvalidGeometryIdentity);
        }
        Ok(Self { project_id, scene_revision, entity_id })
    }

    pub fn require_revision(&self, actual: u64) -> Result<(), BoundaryError> {
        if actual == self.scene_revision { Ok(()) }
        else { Err(BoundaryError::StaleGeometryRevision) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_duration_uses_integer_millisecond_mapping_without_drift() {
        assert_eq!(scan_period_ms(100).unwrap().as_nanos(), 100_000_000);
        assert_eq!(scan_period_ms(0), Err(BoundaryError::ZeroScanPeriod));
        let commit = PlcScanCommit {
            scan_number: 4,
            completed_at: SimInstant::from_nanos(400_000_000),
            outputs: BTreeMap::new(),
        };
        assert_eq!(validate_scan_commit(
            3, SimInstant::from_nanos(300_000_000), scan_period_ms(100).unwrap(), &commit
        ), Ok(()));
    }

    #[test]
    fn mismatched_scan_sequence_or_time_is_not_silently_accepted() {
        let mut commit = PlcScanCommit {
            scan_number: 2,
            completed_at: SimInstant::from_nanos(100_000_000),
            outputs: BTreeMap::new(),
        };
        assert_eq!(validate_scan_commit(
            0, SimInstant::ZERO, scan_period_ms(100).unwrap(), &commit
        ), Err(BoundaryError::ScanSequenceMismatch));
        commit.scan_number = 1;
        commit.completed_at = SimInstant::from_nanos(99_999_999);
        assert_eq!(validate_scan_commit(
            0, SimInstant::ZERO, scan_period_ms(100).unwrap(), &commit
        ), Err(BoundaryError::ScanCompletionTimeMismatch));
    }

    #[test]
    fn finite_real_preserves_actual_zero_and_rejects_nan() {
        assert_eq!(FiniteReal::new(-0.0), FiniteReal::new(0.0));
        assert_eq!(FiniteReal::new(f64::NAN), Err(BoundaryError::NonFiniteReal));
        assert_eq!(FiniteReal::new(f64::INFINITY), Err(BoundaryError::NonFiniteReal));
    }

    #[test]
    fn legacy_plc_input_subset_is_explicit() {
        let mut inputs = PlcScanInputs {
            apply_at: SimInstant::ZERO,
            period: scan_period_ms(100).unwrap(),
            signals: BTreeMap::new(),
        };
        let key = SignalKey::new("sensor.ready").unwrap();
        inputs.signals.insert(key.clone(), SignalValue::Bool(true));
        assert_eq!(inputs.validate_legacy_python_inputs(), Ok(()));
        inputs.signals.insert(key, SignalValue::Unavailable);
        assert_eq!(inputs.validate_legacy_python_inputs(), Err(BoundaryError::UnsupportedPlcInput));
        assert_eq!(SignalKey::new(" sensor"), Err(BoundaryError::InvalidSignalKey));
    }

    #[test]
    fn geometry_revision_is_not_equivalent_to_a_physics_tick() {
        let snapshot = GeometrySnapshotRef::new("project-A", 8, "valve-body").unwrap();
        assert_eq!(snapshot.require_revision(8), Ok(()));
        assert_eq!(snapshot.require_revision(9), Err(BoundaryError::StaleGeometryRevision));
    }
}
