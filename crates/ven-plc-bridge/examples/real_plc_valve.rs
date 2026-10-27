//! Real PLC Web ScheduledRuntime + VEN conservative time + a bounded valve.
//! Requires Python PLC Web backend on PYTHONPATH and all Python dependencies.
//!
//!   PYTHONPATH=/path/to/PLC-web/backend/src cargo run -p ven-plc-bridge --example real_plc_valve
//!
//! This is an engineering process model, NOT CAD/FEM/hydraulic physics.
use serde_json::json;
use sim_kernel::conservative::{Claims, ConservativeCoordinator};
use sim_kernel::external_boundary::{SignalKey, SignalValue};
use sim_kernel::resources::ParticipantId;
use sim_time::{SimDuration, SimInstant};
use std::{collections::BTreeMap, process::Command};
use ven_plc_bridge::{IoMessage, PythonPlcActor, PythonScheduledRuntime};
use ven_plc_bridge::plant::{ValveConfig, ValveGeometry, ValvePlantActor};

fn key(value: &str) -> SignalKey { SignalKey::new(value).unwrap() }

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let graph = json!({
        "environment": "opta", "hardwareId": "opta-wifi",
        "nodes": [
            {"id":"open_request","type":"input"},
            {"id":"valve_cmd","type":"output"}
        ],
        "connections": [
            {"id":"c1", "from":{"nodeId":"open_request","port":"q"},
             "to":{"nodeId":"valve_cmd","port":"in"}}
        ]
    });
    let python = std::env::var("PLC_PYTHON").unwrap_or_else(|_| "python3".to_owned());
    let mut command = Command::new(python);
    command.args(["-u", "-m", "backend.services.ven_runtime_worker"]);
    let runtime = PythonScheduledRuntime::spawn_with_command(command, graph, 100)?;
    let mut routing = BTreeMap::new();
    routing.insert(key("valve_cmd"), ParticipantId(2));
    let plc = PythonPlcActor::new(runtime, routing)?;
    let valve = ValvePlantActor::new(ValveConfig {
        plc: ParticipantId(1),
        command_key: key("valve_cmd"),
        opened_key: key("valve.opened"),
        closed_key: key("valve.closed"),
        fault_key: key("valve.fault"),
        open_duration: SimDuration::from_nanos(150_000_000),
        close_duration: SimDuration::from_nanos(200_000_000),
        geometry: ValveGeometry::Unbound,
    })?;
    let mut host = ConservativeCoordinator::<IoMessage, u64>::new();
    host.register(ParticipantId(1), Claims::NoExclusiveResources, Box::new(plc))?;
    host.register(ParticipantId(2), Claims::NoExclusiveResources, Box::new(valve))?;
    host.inject_now(ParticipantId(1), ParticipantId(0),
        IoMessage::PlcInput {
            key: key("open_request.q"), value: SignalValue::Bool(true),
        })?;
    let result = host.advance_to(SimInstant::from_nanos(350_000_000))?;
    println!("reached {} ns; {} routed events",
        result.reached.as_nanos(), result.deliveries.len());
    for receipt in &result.deliveries {
        println!("{} ns: actor {} -> actor {}",
            receipt.at.as_nanos(), receipt.source.0, receipt.target.0);
    }
    assert!(result.deliveries.iter().any(|receipt|
        receipt.source == ParticipantId(2) && receipt.target == ParticipantId(1)
            && receipt.at == SimInstant::from_nanos(250_000_000)
    ), "valve position feedback must arrive at physical completion before 300ms PLC scan");
    Ok(())
}
