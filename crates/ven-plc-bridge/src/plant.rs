//! Deterministic first-order binary valve pilot, independent of Gefest B-rep.
//!
//! A geometry snapshot reference may label the part, but no mechanical
//! properties are inferred from CAD. Complete/open/closed feedback is digital
//! and changes at explicitly scheduled virtual timestamps.

use crate::{IoMessage, BridgeError};
use sim_kernel::{AdvanceOutcome, TimeGrant, TimedOutput};
use sim_kernel::conservative::{ActorError, CausalInput, CausalParticipant, DirectedEvent};
use sim_kernel::external_boundary::{GeometrySnapshotRef, SignalKey, SignalValue};
use sim_kernel::resources::ParticipantId;
use sim_time::{SimDuration, SimInstant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValveFault {
    Healthy,
    /// Motor/valve cannot move; the requested state is never attained.
    Stuck,
    /// Opening completes but open-limit feedback cannot assert.
    OpenLimitFailed,
    /// Closing completes but closed-limit feedback cannot assert.
    ClosedLimitFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValveGeometry {
    Unbound,
    Snapshot(GeometrySnapshotRef),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValveMotion {
    Closed,
    Open,
    Opening { until: SimInstant },
    Closing { until: SimInstant },
    StuckOpening,
    StuckClosing,
}

#[derive(Clone, Debug)]
pub struct ValveConfig {
    pub plc: ParticipantId,
    pub command_key: SignalKey,
    pub opened_key: SignalKey,
    pub closed_key: SignalKey,
    pub fault_key: SignalKey,
    pub open_duration: SimDuration,
    pub close_duration: SimDuration,
    pub geometry: ValveGeometry,
}

impl ValveConfig {
    pub fn validate(&self) -> Result<(), BridgeError> {
        if self.open_duration == SimDuration::ZERO || self.close_duration == SimDuration::ZERO {
            return Err(BridgeError::InvalidProtocol);
        }
        if self.opened_key == self.closed_key || self.opened_key == self.fault_key
            || self.closed_key == self.fault_key
        {
            return Err(BridgeError::InvalidProtocol);
        }
        Ok(())
    }
}

/// Discrete equipment actor: consistent with a 2-limit-switch pilot valve,
/// not a continuous CFD/FEA, fluid pressure or servo control model.
pub struct ValvePlantActor {
    now: SimInstant,
    cfg: ValveConfig,
    motion: ValveMotion,
    fault: ValveFault,
    publish: bool,
}

impl ValvePlantActor {
    pub fn new(cfg: ValveConfig) -> Result<Self, BridgeError> {
        cfg.validate()?;
        Ok(Self {
            now: SimInstant::ZERO, cfg,
            motion: ValveMotion::Closed, fault: ValveFault::Healthy,
            publish: true, // startup CLOSED / !OPEN / !FAULT sensors at t=0
        })
    }
    pub fn motion(&self) -> ValveMotion { self.motion }
    pub fn fault(&self) -> ValveFault { self.fault }

    fn sensors(&self) -> [(SignalKey, bool); 3] {
        let is_open = matches!(self.motion, ValveMotion::Open);
        let is_closed = matches!(self.motion, ValveMotion::Closed);
        [
            (self.cfg.opened_key.clone(), is_open && self.fault != ValveFault::OpenLimitFailed),
            (self.cfg.closed_key.clone(), is_closed && self.fault != ValveFault::ClosedLimitFailed),
            (self.cfg.fault_key.clone(), self.fault != ValveFault::Healthy),
        ]
    }

    fn change_motion(&mut self, open: bool) -> Result<(), BridgeError> {
        let requested = if open {
            if self.fault == ValveFault::Stuck {
                ValveMotion::StuckOpening
            } else if self.motion == ValveMotion::Open {
                ValveMotion::Open
            } else {
                ValveMotion::Opening {
                    until: self.now.checked_add(self.cfg.open_duration)
                        .ok_or(BridgeError::TimeOverflow)?,
                }
            }
        } else if self.fault == ValveFault::Stuck {
            ValveMotion::StuckClosing
        } else if self.motion == ValveMotion::Closed {
            ValveMotion::Closed
        } else {
            ValveMotion::Closing {
                until: self.now.checked_add(self.cfg.close_duration)
                    .ok_or(BridgeError::TimeOverflow)?,
            }
        };
        if requested != self.motion {
            self.motion = requested;
            self.publish = true;
        }
        Ok(())
    }
}

impl CausalParticipant<IoMessage> for ValvePlantActor {
    fn current_time(&self) -> SimInstant { self.now }
    fn next_wakeup(&self) -> Option<SimInstant> {
        if self.publish { return Some(self.now); }
        match self.motion {
            ValveMotion::Opening { until } | ValveMotion::Closing { until } => Some(until),
            _ => None,
        }
    }
    fn advance(&mut self, grant: TimeGrant) -> Result<AdvanceOutcome<DirectedEvent<IoMessage>>, ActorError> {
        if grant.until < self.now { return Err(Box::new(BridgeError::TimeMismatch)); }
        match self.motion {
            ValveMotion::Opening { until } if until == grant.until => {
                self.motion = ValveMotion::Open;
                self.publish = true;
            }
            ValveMotion::Closing { until } if until == grant.until => {
                self.motion = ValveMotion::Closed;
                self.publish = true;
            }
            _ => {}
        }
        self.now = grant.until;
        let mut outputs = Vec::new();
        if self.publish {
            for (key, value) in self.sensors() {
                outputs.push(TimedOutput {
                    at: self.now,
                    value: DirectedEvent {
                        target: self.cfg.plc,
                        payload: IoMessage::PlcInput { key, value: SignalValue::Bool(value) },
                    },
                });
            }
            self.publish = false;
        }
        Ok(AdvanceOutcome { reached: self.now, outputs, next_wakeup: self.next_wakeup() })
    }
    fn receive(&mut self, event: CausalInput<IoMessage>) -> Result<(), ActorError> {
        if event.at != self.now { return Err(Box::new(BridgeError::TimeMismatch)); }
        match event.payload {
            IoMessage::PlcOutput { key, value } if key == self.cfg.command_key => {
                match value {
                    SignalValue::Bool(open) => self.change_motion(open).map_err(Into::into),
                    _ => Err(Box::new(BridgeError::UnsupportedValue)),
                }
            }
            IoMessage::ValveFault(fault) => {
                self.fault = fault;
                if fault == ValveFault::Stuck {
                    self.motion = match self.motion {
                        ValveMotion::Opening { .. } => ValveMotion::StuckOpening,
                        ValveMotion::Closing { .. } => ValveMotion::StuckClosing,
                        other => other,
                    };
                }
                self.publish = true;
                Ok(())
            }
            _ => Err(Box::new(BridgeError::InvalidMessage)),
        }
    }
}
