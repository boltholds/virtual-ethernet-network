//! Full PLC -> FC06 -> actual slave register write -> valve dynamics ->
//! holding-register feedback -> FC03 -> PLC input snapshot -> monitor.
//! Python fixture is explicitly NOT production ScheduledRuntime.

use sim_kernel::{AdvanceOutcome, TimeGrant};
use sim_kernel::conservative::{
    ActorError, CausalInput, CausalParticipant, Claims, ConservativeCoordinator,
    DirectedEvent,
};
use sim_kernel::external_boundary::{SignalKey, SignalValue};
use sim_kernel::resources::ParticipantId;
use sim_time::{SimDuration, SimInstant};
use serde_json::json;
use std::{collections::BTreeMap, process::Command, sync::{Arc, Mutex}};
use ven_core::PortId;
use ven_medium::rs485::UartFormat;
use ven_plc_bridge::{IoMessage, PythonPlcActor, PythonScheduledRuntime, ScanPolling};
use ven_plc_bridge::modbus_valve::{ValveModbusConfig, ValveModbusGateway};
use ven_plc_bridge::plant::{ValveConfig, ValveFault, ValveGeometry, ValvePlantActor};
use ven_modbus::{RtuTiming};
use ven_modbus_sim::{BusLod, ModbusBus};
use ven_rs485_phy::{IdleLine, Rs485DigitalPhy, TransceiverProfile};
use ven_sim::VenNetwork;
use ven_topology::{BusId, InterfaceKind, NodeId, Port};

const PLC: ParticipantId = ParticipantId(1);
const NET: ParticipantId = ParticipantId(2);
const VALVE: ParticipantId = ParticipantId(3);
const MONITOR: ParticipantId = ParticipantId(4);
fn key(s: &str) -> SignalKey { SignalKey::new(s).unwrap() }

fn process() -> PythonScheduledRuntime {
    let mut command = Command::new("python3");
    command.arg("-u").arg(format!(
        "{}/tests/fixtures/python_modbus_mock.py", env!("CARGO_MANIFEST_DIR")
    ));
    PythonScheduledRuntime::spawn_with_command(command, json!({}), 100).unwrap()
}
fn network(lod: BusLod) -> ModbusBus {
    let mut bus = match lod {
        BusLod::DigitalBits => {
            let mut phy = Rs485DigitalPhy::new(
                BusId(7), 9600, UartFormat::MODBUS_RTU_8E1,
                SimDuration::ZERO, IdleLine::High,
            ).unwrap();
            for id in 1..=2 {
                phy.add_port(PortId(id), TransceiverProfile::default()).unwrap();
            }
            ModbusBus::digital_bits(phy).unwrap()
        }
        BusLod::CharacterTimed => {
            let mut ven = VenNetwork::new();
            for id in 1..=2 {
                ven.add_node(NodeId(id)).unwrap();
                ven.add_port(PortId(id), Port {
                    node: NodeId(id), kind: InterfaceKind::Rs485,
                }).unwrap();
            }
            ven.create_rs485_bus(
                BusId(7), 9600, UartFormat::MODBUS_RTU_8E1, SimDuration::ZERO
            ).unwrap();
            for id in 1..=2 { ven.attach_rs485_port(BusId(7), PortId(id)).unwrap(); }
            ModbusBus::character_timed(
                ven, RtuTiming::new(9600, UartFormat::MODBUS_RTU_8E1).unwrap()
            )
        }
    };
    bus.attach_master(PortId(1), SimDuration::from_nanos(100_000_000)).unwrap();
    bus.attach_slave(PortId(2), 2).unwrap();
    bus
}
struct Monitor {
    now: SimInstant,
    observed: Arc<Mutex<Vec<(SimInstant, bool)>>>,
}
impl CausalParticipant<IoMessage> for Monitor {
    fn current_time(&self) -> SimInstant { self.now }
    fn next_wakeup(&self) -> Option<SimInstant> { None }
    fn advance(&mut self, grant: TimeGrant)
        -> Result<AdvanceOutcome<DirectedEvent<IoMessage>>, ActorError>
    {
        self.now = grant.until;
        Ok(AdvanceOutcome { reached: self.now, outputs: Vec::new(), next_wakeup: None })
    }
    fn receive(&mut self, input: CausalInput<IoMessage>) -> Result<(), ActorError> {
        match input.payload {
            IoMessage::PlcOutput { key, value: SignalValue::Bool(opened) }
                if key == key_fn("monitor.open") => {
                self.observed.lock().unwrap().push((input.at, opened));
                Ok(())
            }
            _ => Err("unexpected monitor event".into()),
        }
    }
}
fn key_fn(s: &str) -> SignalKey { SignalKey::new(s).unwrap() }

fn run_case(lod: BusLod, stuck: bool) -> Vec<(SimInstant, bool)> {
    let mut routes = BTreeMap::new();
    routes.insert(key("valve_cmd"), NET);
    routes.insert(key("monitor.open"), MONITOR);
    let plc = PythonPlcActor::new(process(), routes).unwrap()
        .with_polling(ScanPolling::EveryScan { target: NET });
    let cfg = ValveModbusConfig {
        plc: PLC, plant: VALVE, master_port: PortId(1), slave_port: PortId(2),
        slave_address: 2,
        command_key: key("valve_cmd"),
        opened_key: key("valve.opened"),
        closed_key: key("valve.closed"),
        fault_key: key("valve.fault"),
        communication_fault_key: key("valve.comm_fault"),
    };
    let gateway = ValveModbusGateway::new(network(lod), cfg).unwrap();
    assert_eq!(gateway.claimed_buses(), vec![BusId(7)]);
    assert_eq!(gateway.slave_register(2), Some(1));
    let valve = ValvePlantActor::new(ValveConfig {
        plc: NET, // only Modbus slave sees the physical limit switches
        command_key: key("valve_cmd"),
        opened_key: key("valve.opened"),
        closed_key: key("valve.closed"),
        fault_key: key("valve.fault"),
        open_duration: SimDuration::from_nanos(150_000_000),
        close_duration: SimDuration::from_nanos(190_000_000),
        geometry: ValveGeometry::Unbound,
    }).unwrap();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let mut host = ConservativeCoordinator::<IoMessage, BusId>::new();
    host.register(PLC, Claims::NoExclusiveResources, Box::new(plc)).unwrap();
    host.register(NET, Claims::Exclusive(vec![BusId(7)]), Box::new(gateway)).unwrap();
    host.register(VALVE, Claims::NoExclusiveResources, Box::new(valve)).unwrap();
    host.register(MONITOR, Claims::NoExclusiveResources,
        Box::new(Monitor { now: SimInstant::ZERO, observed: Arc::clone(&observed) })).unwrap();
    assert_eq!(host.owner_of(BusId(7)), Some(NET));
    host.inject_now(PLC, ParticipantId(0), IoMessage::PlcInput {
        key: key("cmd"), value: SignalValue::Bool(true),
    }).unwrap();
    if stuck {
        host.inject_now(VALVE, ParticipantId(0), IoMessage::ValveFault(ValveFault::Stuck)).unwrap();
    }
    let report = host.advance_to(SimInstant::from_nanos(600_000_000)).unwrap();
    assert_eq!(report.reached, SimInstant::from_nanos(600_000_000));
    // Both physical acceptance and polled sensor feedback route through NET.
    assert!(report.deliveries.iter().any(|r| r.source == PLC && r.target == NET &&
        r.at == SimInstant::from_nanos(100_000_000)));
    assert!(report.deliveries.iter().any(|r| r.source == NET && r.target == VALVE &&
        r.at > SimInstant::from_nanos(100_000_000)));
    if stuck {
        // A mechanically stuck valve has no completion event; its fault
        // feedback is published at the initial virtual timestamp instead.
        assert!(report.deliveries.iter().any(|r| r.source == VALVE && r.target == NET
            && r.at == SimInstant::ZERO));
        assert!(!report.deliveries.iter().any(|r| r.source == VALVE && r.target == NET
            && r.at > SimInstant::from_nanos(150_000_000)));
    } else {
        assert!(report.deliveries.iter().any(|r| r.source == VALVE && r.target == NET
            && r.at > SimInstant::from_nanos(150_000_000)));
    }
    assert!(report.deliveries.iter().any(|r| r.source == NET && r.target == PLC &&
        r.at > SimInstant::from_nanos(100_000_000)));
    assert!(!host.is_poisoned());
    let record = observed.lock().unwrap().clone();
    record
}
#[test]
fn both_lods_poll_real_modbus_registers_after_valve_motion() {
    for lod in [BusLod::CharacterTimed, BusLod::DigitalBits] {
        let signals = run_case(lod, false);
        assert!(signals.iter().any(|(_, open)| !open));
        assert!(signals.iter().any(|(_, open)| *open));
        let first_open = signals.iter().find(|(_,open)| *open).unwrap().0;
        assert!(first_open >= SimInstant::from_nanos(300_000_000));
        assert!(first_open <= SimInstant::from_nanos(500_000_000));
    }
}
#[test]
fn stuck_valve_never_reports_open_despite_valid_fc06_and_fc03() {
    for lod in [BusLod::CharacterTimed, BusLod::DigitalBits] {
        let signals = run_case(lod, true);
        assert!(signals.iter().any(|(_, open)| !open));
        assert!(signals.iter().all(|(_, open)| !open));
    }
}
