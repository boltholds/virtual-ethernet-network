use sim_kernel::conservative::{
    CausalInput, CausalParticipant, Claims, ConservativeCoordinator,
};
use sim_kernel::resources::ParticipantId;
use sim_kernel::external_boundary::{SignalKey, SignalValue};
use sim_time::{SimDuration, SimInstant};
use serde_json::json;
use std::{collections::BTreeMap, process::Command, sync::{Arc, Mutex}};
use ven_plc_bridge::{IoMessage, PythonPlcActor, PythonScheduledRuntime};
use ven_plc_bridge::plant::{ValveConfig, ValveFault, ValveGeometry, ValvePlantActor, ValveMotion};
use sim_kernel::{TimeGrant};

fn key(s: &str) -> SignalKey { SignalKey::new(s).unwrap() }
fn mock_process() -> PythonScheduledRuntime {
    let mut process = Command::new("python3");
    process.arg("-u").arg(format!("{}/tests/fixtures/python_mock.py", env!("CARGO_MANIFEST_DIR")));
    PythonScheduledRuntime::spawn_with_command(process, json!({}), 100).unwrap()
}

#[test]
fn persistent_process_respects_sequence_and_time() {
    let mut worker = mock_process();
    let mut inputs = BTreeMap::new();
    inputs.insert(key("cmd"), SignalValue::Bool(true));
    let (first_time, first) = worker.scan(&inputs).unwrap();
    assert_eq!(worker.sequence(), 1);
    assert_eq!(first_time, SimInstant::from_nanos(100_000_000));
    assert_eq!(first[&key("valve_cmd")], SignalValue::Bool(true));
    inputs.insert(key("cmd"), SignalValue::Bool(false));
    let (second_time, second) = worker.scan(&inputs).unwrap();
    assert_eq!(worker.sequence(), 2);
    assert_eq!(second_time, SimInstant::from_nanos(200_000_000));
    assert_eq!(second[&key("valve_cmd")], SignalValue::Bool(false));
}

#[test]
fn real_causal_barrier_wraps_python_process_and_timed_valve() {
    let proc = mock_process(); // protocol fixture, real worker tested in PLC-web
    let mut routes = BTreeMap::new();
    routes.insert(key("valve_cmd"), ParticipantId(2));
    let plc = PythonPlcActor::new(proc, routes).unwrap();
    let cfg = ValveConfig {
        plc: ParticipantId(1),
        command_key: key("valve_cmd"),
        opened_key: key("valve.open_limit"),
        closed_key: key("valve.closed_limit"),
        fault_key: key("valve.fault"),
        open_duration: SimDuration::from_nanos(150_000_000),
        close_duration: SimDuration::from_nanos(200_000_000),
        geometry: ValveGeometry::Unbound,
    };
    let valve = ValvePlantActor::new(cfg).unwrap();
    let mut run = ConservativeCoordinator::<IoMessage, u64>::new();
    run.register(ParticipantId(1), Claims::NoExclusiveResources, Box::new(plc)).unwrap();
    run.register(ParticipantId(2), Claims::NoExclusiveResources, Box::new(valve)).unwrap();
    run.inject_now(ParticipantId(1), ParticipantId(0),
        IoMessage::PlcInput { key: key("cmd"), value: SignalValue::Bool(true) }).unwrap();
    let report = run.advance_to(SimInstant::from_nanos(350_000_000)).unwrap();
    assert_eq!(report.reached, SimInstant::from_nanos(350_000_000));
    assert!(!run.is_poisoned());
    assert!(report.deliveries.iter().any(|d|
        d.source == ParticipantId(1) && d.target == ParticipantId(2)
            && d.at == SimInstant::from_nanos(100_000_000)
    ));
    // Physical movement finishes at 250ms, not rounded to the 300ms PLC scan.
    assert!(report.deliveries.iter().any(|d|
        d.source == ParticipantId(2) && d.target == ParticipantId(1)
            && d.at == SimInstant::from_nanos(250_000_000)
    ));
}

#[test]
fn stuck_fault_stops_motor_without_fabricating_open_limit() {
    let cfg = ValveConfig {
        plc: ParticipantId(1),
        command_key: key("command"),
        opened_key: key("open_limit"),
        closed_key: key("close_limit"),
        fault_key: key("fault"),
        open_duration: SimDuration::from_nanos(50_000_000),
        close_duration: SimDuration::from_nanos(70_000_000),
        geometry: ValveGeometry::Unbound,
    };
    let mut valve = ValvePlantActor::new(cfg).unwrap();
    let at = SimInstant::ZERO;
    valve.receive(CausalInput {
        source: ParticipantId(0), at,
        payload: IoMessage::ValveFault(ValveFault::Stuck)
    }).unwrap();
    valve.receive(CausalInput {
        source: ParticipantId(1), at,
        payload: IoMessage::PlcOutput {
            key: key("command"), value: SignalValue::Bool(true)
        }
    }).unwrap();
    assert_eq!(valve.motion(), ValveMotion::StuckOpening);
    let out = valve.advance(TimeGrant { until: SimInstant::ZERO }).unwrap();
    assert!(out.outputs.iter().any(|e| matches!(
        &e.value.payload,
        IoMessage::PlcInput { key, value: SignalValue::Bool(true) }
            if key == &key_fn("fault")
    )));
    assert!(!out.outputs.iter().any(|e| matches!(
        &e.value.payload,
        IoMessage::PlcInput { key, value: SignalValue::Bool(true) }
            if key == &key_fn("open_limit")
    )));
    assert_eq!(valve.next_wakeup(), None);
}

fn key_fn(s: &str) -> SignalKey { SignalKey::new(s).unwrap() }
