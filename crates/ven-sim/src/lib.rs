//! A VEN participant for conservative host-managed virtual time.

use sim_kernel::{AdvanceOutcome, SimulationParticipant, TimeGrant, TimedOutput};
use sim_time::SimInstant;
use std::fmt;
use ven_core::{Frame, FrameArrival, PortId};
use ven_medium::{FullDuplexLink, LinkError, Transmission};
use ven_scheduler::{NetworkScheduler, ScheduleError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VenError {
    NoLink,
    PortAlreadyConnected,
    Link(LinkError),
    Schedule(ScheduleError),
    TimeRegression,
}

impl fmt::Display for VenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for VenError {}

/// This crate intentionally does not own the global time authority.
#[derive(Debug, Default)]
pub struct VenNetwork {
    now: SimInstant,
    links: Vec<FullDuplexLink>,
    scheduler: NetworkScheduler<FrameArrival>,
}

impl VenNetwork {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_link(&mut self, link: FullDuplexLink) -> Result<(), VenError> {
        let (a, b) = link.endpoints();
        if self.links.iter().any(|existing| {
            let (x, y) = existing.endpoints();
            x == a || x == b || y == a || y == b
        }) {
            return Err(VenError::PortAlreadyConnected);
        }
        self.links.push(link);
        Ok(())
    }

    /// Injects an already encoded frame at the participant's current virtual time.
    /// No transport/protocol interpretation takes place here.
    pub fn send(
        &mut self,
        from: PortId,
        to: PortId,
        frame: Frame,
    ) -> Result<Transmission, VenError> {
        let link = self
            .links
            .iter_mut()
            .find(|candidate| candidate.connects(from, to))
            .ok_or(VenError::NoLink)?;

        // Commit medium availability only if scheduling also succeeds.
        let previous = link.clone();
        let tx = link
            .transmit(from, to, frame.len(), self.now)
            .map_err(VenError::Link)?;
        if let Err(error) = self.scheduler.schedule_at(
            tx.arrives,
            FrameArrival { from, to, frame },
        ) {
            *link = previous;
            return Err(VenError::Schedule(error));
        }
        Ok(tx)
    }
}

impl SimulationParticipant for VenNetwork {
    type Output = FrameArrival;
    type Error = VenError;

    fn current_time(&self) -> SimInstant {
        self.now
    }

    fn next_wakeup(&self) -> Option<SimInstant> {
        self.scheduler.next_time()
    }

    fn advance(&mut self, grant: TimeGrant) -> Result<AdvanceOutcome<Self::Output>, Self::Error> {
        if grant.until < self.now {
            return Err(VenError::TimeRegression);
        }

        // Return early before any frame is externally visible to another model.
        if let Some(at) = self.scheduler.next_time().filter(|t| *t <= grant.until) {
            let mut outputs = Vec::new();
            while let Some(event) = self.scheduler.pop_through(at) {
                outputs.push(TimedOutput {
                    at: event.at,
                    value: event.event,
                });
            }
            self.now = at;
            return Ok(AdvanceOutcome {
                reached: at,
                outputs,
                next_wakeup: self.scheduler.next_time(),
            });
        }

        self.now = grant.until;
        Ok(AdvanceOutcome {
            reached: self.now,
            outputs: Vec::new(),
            next_wakeup: self.scheduler.next_time(),
        })
    }
}
