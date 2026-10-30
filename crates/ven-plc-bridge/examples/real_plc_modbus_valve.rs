//! Actual PLC Web ScheduledRuntime -> Modbus RTU FC06/FC03 -> timed valve.
//!
//! Requires private PLC Web PR #654 checkout on PYTHONPATH and Python deps.
//! This is a real cross-process PLC scan, a simulated Modbus device, and a
//! deliberately simple discrete equipment model; no Gefest physics engine.
use serde_json::json;
use sim_kernel::{AdvanceOutcome, TimeGrant};
use sim_kernel::conservative::{
    ActorError, CausalInput, CausalParticipant, Claims, ConservativeCoordinator, DirectedEvent,
};
use sim_kernel::external_boundary::{SignalKey, SignalValue};
use sim_kernel::resources::ParticipantId;
use sim_time::{SimDuration, SimInstant};
use std::{collections::BTreeMap, error::Error, process::Command, sync::{Arc, Mutex}};
use ven_core::PortId;
use ven_medium::rs485::UartFormat;
use ven_modbus_sim::ModbusBus;
use ven_plc_bridge::{IoMessage, PythonPlcActor, PythonScheduledRuntime, ScanPolling};
use ven_plc_bridge::modbus_valve::{ValveModbusConfig, ValveModbusGateway};
use ven_plc_bridge::plant::{ValveConfig, ValveGeometry, ValvePlantActor};
use ven_rs485_phy::{IdleLine, Rs485DigitalPhy, TransceiverProfile};
use ven_topology::BusId;

const PLC: ParticipantId = ParticipantId(1);
const NETWORK: ParticipantId = ParticipantId(2);
const VALVE: ParticipantId = ParticipantId(3);
const OBSERVER: ParticipantId = ParticipantId(4);

fn key(s: &str) -> SignalKey { SignalKey::new(s).expect("nonempty symbol") }
fn causal(error: impl std::fmt::Debug) -> std::io::Error {
    std::io::Error::other(format!("{error:?}"))
}

struct Observer {
    now: SimInstant,
    seen: Arc<Mutex<Vec<(SimInstant, bool)>>>,
}
impl CausalParticipant<IoMessage> for Observer {
    fn current_time(&self) -> SimInstant { self.now }
    fn next_wakeup(&self) -> Option<SimInstant> { None }
    fn advance(&mut self, grant: TimeGrant) -> Result<AdvanceOutcome<DirectedEvent<IoMessage>>, ActorError> {
        self.now = grant.until;
        Ok(AdvanceOutcome { reached: self.now, outputs: Vec::new(), next_wakeup: None })
    }
    fn receive(&mut self, input: CausalInput<IoMessage>) -> Result<(), ActorError> {
        match input.payload {
            IoMessage::PlcOutput { key, value: SignalValue::Bool(open) }
                if key.as_str() == "monitor_open" =>
            {
                self.seen.lock().expect("observer mutex").push((input.at, open));
                Ok(())
            }
            _ => Err("unexpected observation".into()),
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    // Both visible outputs come from REAL scheduled IR execution, not from
    // Rust shadow logic or a hardcoded PLC fixture.
    let graph = json!({
        "environment":"opta", "hardwareId":"opta-wifi",
        "nodes":[
            {"id":"request","type":"input"},
            {"id":"valve_cmd","type":"output"},
            {"id":"opened","type":"input"},
            {"id":"monitor_open","type":"output"}
        ],
        "connections":[
            {"id":"c1","from":{"nodeId":"request","port":"q"},
             "to":{"nodeId":"valve_cmd","port":"in"}},
            {"id":"c2","from":{"nodeId":"opened","port":"q"},
             "to":{"nodeId":"monitor_open","port":"in"}}
        ]
    });
    let python = std::env::var("PLC_PYTHON").unwrap_or_else(|_| "python3".into());
    let mut command = Command::new(python);
    command.args(["-u","-m","backend.services.ven_runtime_worker"]);
    let runtime = PythonScheduledRuntime::spawn_with_command(command, graph, 100)?;
    let mut routes = BTreeMap::new();
    routes.insert(key("valve_cmd"), NETWORK);
    routes.insert(key("monitor_open"), OBSERVER);
    let plc = PythonPlcActor::new(runtime, routes)?
        .with_polling(ScanPolling::EveryScan { target: NETWORK });

    let mut phy = Rs485DigitalPhy::new(
        BusId(7), 9600, UartFormat::MODBUS_RTU_8E1,
        SimDuration::ZERO, IdleLine::High,
    )?;
    for id in 1..=2 {
        phy.add_port(PortId(id), TransceiverProfile::default())?;
    }
    let mut bus = ModbusBus::digital_bits(phy)?;
    bus.attach_master(PortId(1), SimDuration::from_nanos(100_000_000))?;
    bus.attach_slave(PortId(2), 2)?;
    let gateway = ValveModbusGateway::new(bus, ValveModbusConfig {
        plc: PLC, plant: VALVE, master_port: PortId(1), slave_port: PortId(2),
        slave_address: 2,
        command_key: key("valve_cmd"),
        opened_key: key("opened.q"),
        closed_key: key("valve.closed"),
        fault_key: key("valve.fault"),
        communication_fault_key: key("valve.communication_fault"),
    })?;
    let claims = gateway.claimed_buses();
    let valve = ValvePlantActor::new(ValveConfig {
        plc: NETWORK, // physical feedback MUST pass through Modbus registers
        command_key: key("valve_cmd"), opened_key: key("opened.q"),
        closed_key: key("valve.closed"), fault_key: key("valve.fault"),
        open_duration: SimDuration::from_nanos(150_000_000),
        close_duration: SimDuration::from_nanos(190_000_000),
        geometry: ValveGeometry::Unbound,
    })?;
    let observed = Arc::new(Mutex::new(Vec::new()));
    let mut host = ConservativeCoordinator::<IoMessage, BusId>::new();
    host.register(PLC, Claims::NoExclusiveResources, Box::new(plc)).map_err(causal)?;
    host.register(NETWORK, Claims::Exclusive(claims), Box::new(gateway)).map_err(causal)?;
    host.register(VALVE, Claims::NoExclusiveResources, Box::new(valve)).map_err(causal)?;
    host.register(OBSERVER, Claims::NoExclusiveResources,
        Box::new(Observer { now: SimInstant::ZERO, seen: Arc::clone(&observed) })
    ).map_err(causal)?;
    host.inject_now(PLC, ParticipantId(0), IoMessage::PlcInput {
        key: key("request.q"), value: SignalValue::Bool(true),
    }).map_err(causal)?;

    let report = host.advance_to(SimInstant::from_nanos(600_000_000)).map_err(causal)?;
    assert_eq!(report.reached, SimInstant::from_nanos(600_000_000));
    let seen = observed.lock().expect("observer mutex");
    assert!(seen.iter().any(|(_, open)| !open), "initial open limit must be false");
    let observed_open = seen.iter().find(|(_, open)| *open)
        .ok_or("PLC did not observe Modbus-polled open limit")?;
    assert!(observed_open.0 >= SimInstant::from_nanos(300_000_000));
    println!("PLC saw valve open at {} ns, after FC06, motion, FC03, and next scan.",
        observed_open.0.as_nanos());
    println!("{} causal deliveries; owned bus: {:?}",
        report.deliveries.len(), host.owner_of(BusId(7)));
    Ok(())
}
