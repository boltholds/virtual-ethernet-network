//! A generic adapter from the Modbus bus participant to a causal event domain.
//! The application chooses its event payload and routing via two closures;
//! PLC/plant models need not depend on Modbus-specific simulation events.

use crate::{LineEvent, ModbusBus};
use sim_kernel::{AdvanceOutcome, SimulationParticipant, TimeGrant, TimedOutput};
use sim_kernel::conservative::{ActorError, CausalInput, CausalParticipant, DirectedEvent};
use sim_time::SimInstant;
use ven_topology::BusId;

/// Takes exclusive ownership of ModbusBus; the coordinator separately owns
/// this bridge and atomically claims bus.claimed_buses() during registration.
pub struct CausalModbusBridge<E, MapOutput, HandleInput> {
    bus: ModbusBus,
    map_output: MapOutput,
    handle_input: HandleInput,
    _event: std::marker::PhantomData<fn(E)>,
}

impl<E, MapOutput, HandleInput> CausalModbusBridge<E, MapOutput, HandleInput>
where
    MapOutput: FnMut(LineEvent) -> Vec<DirectedEvent<E>>,
    HandleInput: FnMut(&mut ModbusBus, CausalInput<E>) -> Result<(), ActorError>,
{
    pub fn new(bus: ModbusBus, map_output: MapOutput, handle_input: HandleInput) -> Self {
        Self { bus, map_output, handle_input, _event: std::marker::PhantomData }
    }

    pub fn claimed_buses(&self) -> Vec<BusId> { self.bus.claimed_buses() }
}

impl<E, MapOutput, HandleInput> CausalParticipant<E>
    for CausalModbusBridge<E, MapOutput, HandleInput>
where
    MapOutput: FnMut(LineEvent) -> Vec<DirectedEvent<E>>,
    HandleInput: FnMut(&mut ModbusBus, CausalInput<E>) -> Result<(), ActorError>,
{
    fn current_time(&self) -> SimInstant { self.bus.current_time() }
    fn next_wakeup(&self) -> Option<SimInstant> { self.bus.next_wakeup() }

    fn advance(&mut self, grant: TimeGrant)
        -> Result<AdvanceOutcome<DirectedEvent<E>>, ActorError>
    {
        let out = self.bus.advance(grant).map_err(|e| Box::new(e) as ActorError)?;
        let mut outputs = Vec::new();
        for TimedOutput { at, value } in out.outputs {
            for directed in (self.map_output)(value) {
                outputs.push(TimedOutput { at, value: directed });
            }
        }
        Ok(AdvanceOutcome {
            reached: out.reached, outputs, next_wakeup: out.next_wakeup,
        })
    }

    fn receive(&mut self, input: CausalInput<E>) -> Result<(), ActorError> {
        if input.at != self.bus.current_time() {
            return Err("input violates current virtual-time barrier".into());
        }
        (self.handle_input)(&mut self.bus, input)
    }
}
