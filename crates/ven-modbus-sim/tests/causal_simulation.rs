//! Three heterogeneous, in-process actors: PLC logic, VEN/Modbus and plant.
//! Demonstrates zero-delay causality, one global virtual clock and bus claims.
//! These are toy PLC/plant actors, NOT an integrated production PLC runtime.

use sim_kernel::{AdvanceOutcome, TimeGrant, TimedOutput};
use sim_kernel::conservative::{
    ActorError, CausalInput, CausalParticipant, Claims, ConservativeCoordinator,
    DirectedEvent,
};
use sim_kernel::resources::ParticipantId;
use sim_time::{SimDuration, SimInstant};
use std::sync::{Arc, Mutex};
use ven_core::PortId;
use ven_medium::rs485::UartFormat;
use ven_modbus::{RtuAdu, RtuTiming};
use ven_modbus::session::MasterEvent;
use ven_modbus_sim::{CausalModbusBridge, LineEvent, ModbusBus};
use ven_rs485_phy::{IdleLine, Rs485DigitalPhy, TransceiverProfile};
use ven_sim::VenNetwork;
use ven_topology::{BusId, InterfaceKind, NodeId, Port};

const PLC: ParticipantId = ParticipantId(1);
const NETWORK: ParticipantId = ParticipantId(2);
const PLANT: ParticipantId = ParticipantId(3);

#[derive(Debug)]
enum Event {
    Submit(RtuAdu),
    Observed(LineEvent),
    Actuate(u16),
    Acknowledged(u16),
}
struct Logic {
    now: SimInstant,
    pending: Vec<DirectedEvent<Event>>,
    confirmed: Arc<Mutex<Vec<(SimInstant, u16)>>>,
}
impl Logic {
    fn new(confirmed: Arc<Mutex<Vec<(SimInstant, u16)>>>) -> Self {
        Self {
            now: SimInstant::ZERO,
            pending: vec![DirectedEvent {
                target: NETWORK,
                payload: Event::Submit(RtuAdu::new(2, 3, vec![0, 7, 0, 1]).unwrap()),
            }],
            confirmed,
        }
    }
}
impl CausalParticipant<Event> for Logic {
    fn current_time(&self) -> SimInstant { self.now }
    fn next_wakeup(&self) -> Option<SimInstant> {
        if self.pending.is_empty() { None } else { Some(self.now) }
    }
    fn advance(&mut self, grant: TimeGrant)
        -> Result<AdvanceOutcome<DirectedEvent<Event>>, ActorError>
    {
        self.now = grant.until;
        let outputs = std::mem::take(&mut self.pending).into_iter()
            .map(|value| TimedOutput { at: self.now, value }).collect();
        Ok(AdvanceOutcome { reached: self.now, outputs, next_wakeup: self.next_wakeup() })
    }
    fn receive(&mut self, input: CausalInput<Event>) -> Result<(), ActorError> {
        match input.payload {
            Event::Observed(LineEvent::Master {
                port: PortId(1), event: MasterEvent::Reply(reply),
            }) if reply.data() == [2, 0x12, 0x34] => {
                self.pending.push(DirectedEvent { target: PLANT, payload: Event::Actuate(0x1234) });
            }
            Event::Acknowledged(value) => {
                self.confirmed.lock().unwrap().push((input.at, value));
            }
            Event::Observed(_) => {}
            _ => return Err("unexpected PLC input".into()),
        }
        Ok(())
    }
}
struct Equipment {
    now: SimInstant,
    pending: Vec<DirectedEvent<Event>>,
    actuated: Arc<Mutex<Vec<(SimInstant, u16)>>>,
}
impl CausalParticipant<Event> for Equipment {
    fn current_time(&self) -> SimInstant { self.now }
    fn next_wakeup(&self) -> Option<SimInstant> {
        if self.pending.is_empty() { None } else { Some(self.now) }
    }
    fn advance(&mut self, grant: TimeGrant)
        -> Result<AdvanceOutcome<DirectedEvent<Event>>, ActorError>
    {
        self.now = grant.until;
        let outputs = std::mem::take(&mut self.pending).into_iter()
            .map(|value| TimedOutput { at: self.now, value }).collect();
        Ok(AdvanceOutcome { reached: self.now, outputs, next_wakeup: self.next_wakeup() })
    }
    fn receive(&mut self, input: CausalInput<Event>) -> Result<(), ActorError> {
        match input.payload {
            Event::Actuate(value) => {
                self.actuated.lock().unwrap().push((input.at, value));
                self.pending.push(DirectedEvent { target: PLC, payload: Event::Acknowledged(value) });
                Ok(())
            }
            _ => Err("unexpected equipment input".into()),
        }
    }
}

fn digital_bus(id: BusId) -> ModbusBus {
    let mut phy = Rs485DigitalPhy::new(
        id, 9600, UartFormat::MODBUS_RTU_8E1, SimDuration::ZERO, IdleLine::High,
    ).unwrap();
    for port in 1..=3 {
        phy.add_port(PortId(port), TransceiverProfile::default()).unwrap();
    }
    let mut bus = ModbusBus::digital_bits(phy).unwrap();
    configure_modbus(&mut bus);
    bus
}
fn character_bus(id: BusId) -> ModbusBus {
    let mut network = VenNetwork::new();
    for port in 1..=3 {
        network.add_node(NodeId(port)).unwrap();
        network.add_port(PortId(port), Port {
            node: NodeId(port), kind: InterfaceKind::Rs485,
        }).unwrap();
    }
    network.create_rs485_bus(id, 9600, UartFormat::MODBUS_RTU_8E1, SimDuration::ZERO).unwrap();
    for port in 1..=3 { network.attach_rs485_port(id, PortId(port)).unwrap(); }
    let mut bus = ModbusBus::character_timed(
        network, RtuTiming::new(9600, UartFormat::MODBUS_RTU_8E1).unwrap(),
    );
    configure_modbus(&mut bus);
    bus
}
fn configure_modbus(bus: &mut ModbusBus) {
    bus.attach_master(PortId(1), SimDuration::from_nanos(100_000_000)).unwrap();
    bus.attach_slave(PortId(2), 2).unwrap();
    bus.attach_slave(PortId(3), 3).unwrap();
    bus.seed_register(PortId(2), 7, 0x1234).unwrap();
    bus.seed_register(PortId(3), 7, 0x9876).unwrap();
}

fn run(bus: ModbusBus) {
    let confirmed = Arc::new(Mutex::new(Vec::new()));
    let actuated = Arc::new(Mutex::new(Vec::new()));
    let mut c = ConservativeCoordinator::<Event, BusId>::new();
    c.register(PLC, Claims::NoExclusiveResources,
        Box::new(Logic::new(Arc::clone(&confirmed)))).unwrap();
    c.register(PLANT, Claims::NoExclusiveResources,
        Box::new(Equipment {
            now: SimInstant::ZERO, pending: Vec::new(), actuated: Arc::clone(&actuated),
        })).unwrap();

    let bridge = CausalModbusBridge::new(
        bus,
        |event| vec![DirectedEvent { target: PLC, payload: Event::Observed(event) }],
        |bus: &mut ModbusBus, input: CausalInput<Event>| match input.payload {
            Event::Submit(adu) => bus.request(PortId(1), adu)
                .map(|_| ()).map_err(|e| Box::new(e) as ActorError),
            _ => Err("unexpected network input".into()),
        },
    );
    let resources = bridge.claimed_buses();
    assert_eq!(resources, vec![BusId(88)]);
    c.register(NETWORK, Claims::Exclusive(resources), Box::new(bridge)).unwrap();
    assert_eq!(c.owner_of(BusId(88)), Some(NETWORK));

    let horizon = SimInstant::from_nanos(150_000_000);
    let report = c.advance_to(horizon).unwrap();
    assert_eq!(report.reached, horizon);
    assert_eq!(c.now(), horizon);
    assert!(!c.is_poisoned());

    let plc_to_network = report.deliveries.iter().find(|event|
        event.source == PLC && event.target == NETWORK).unwrap();
    assert_eq!(plc_to_network.at, SimInstant::ZERO);
    let network_to_plc = report.deliveries.iter().find(|event|
        event.source == NETWORK && event.target == PLC).unwrap();
    let plc_to_plant = report.deliveries.iter().find(|event|
        event.source == PLC && event.target == PLANT).unwrap();
    let plant_to_plc = report.deliveries.iter().find(|event|
        event.source == PLANT && event.target == PLC).unwrap();
    assert!(network_to_plc.at > SimInstant::ZERO);
    assert_eq!(network_to_plc.at, plc_to_plant.at);
    assert_eq!(plc_to_plant.at, plant_to_plc.at);
    assert_eq!(actuated.lock().unwrap().as_slice(), &[(network_to_plc.at, 0x1234)]);
    assert_eq!(confirmed.lock().unwrap().as_slice(), &[(network_to_plc.at, 0x1234)]);
    c.unregister(NETWORK).unwrap();
    assert_eq!(c.owner_of(BusId(88)), None);
}

#[test]
fn digital_modbus_plc_equipment_causal_pipeline() {
    run(digital_bus(BusId(88)));
}
#[test]
fn character_modbus_plc_equipment_causal_pipeline() {
    run(character_bus(BusId(88)));
}
