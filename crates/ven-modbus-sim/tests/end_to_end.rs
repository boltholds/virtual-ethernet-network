use sim_kernel::{SimulationParticipant, TimeCoordinator};
use sim_time::{SimDuration, SimInstant};
use ven_core::PortId;
use ven_medium::rs485::UartFormat;
use ven_modbus::{RtuAdu, RtuTiming, RtuError};
use ven_modbus::session::{MasterEvent, SessionError};
use ven_modbus_sim::{LineError, LineEvent, ModbusRtuLine};
use ven_sim::{UartFault, VenNetwork};
use ven_topology::{BusId, InterfaceKind, NodeId, Port};

fn line() -> ModbusRtuLine {
    let mut network = VenNetwork::new();
    for id in 1..=3 {
        network.add_node(NodeId(id)).unwrap();
        network.add_port(PortId(id), Port { node: NodeId(id), kind: InterfaceKind::Rs485 }).unwrap();
    }
    network.create_rs485_bus(BusId(1), 9600, UartFormat::MODBUS_RTU_8E1, SimDuration::ZERO).unwrap();
    for id in 1..=3 { network.attach_rs485_port(BusId(1), PortId(id)).unwrap(); }
    let mut line = ModbusRtuLine::new(
        network, RtuTiming::new(9600, UartFormat::MODBUS_RTU_8E1).unwrap()
    );
    line.attach_master(PortId(1), SimDuration::from_nanos(100_000_000)).unwrap();
    line.attach_slave(PortId(2), 2).unwrap();
    line.attach_slave(PortId(3), 3).unwrap();
    line.seed_register(PortId(2), 7, 0x1234).unwrap();
    line.seed_register(PortId(3), 7, 0x9999).unwrap();
    line
}

fn drive(line: &mut ModbusRtuLine, horizon: SimInstant) -> Vec<LineEvent> {
    let mut coord = TimeCoordinator::new();
    let mut events = Vec::new();
    for _ in 0..1000 {
        let grant = coord.grant(horizon).unwrap();
        let step = line.advance(grant).unwrap();
        coord.commit(grant, &step).unwrap();
        for event in step.outputs { events.push(event.value); }
        if step.reached == horizon { return events; }
    }
    panic!("simulation did not reach horizon");
}

#[test]
fn reads_only_addressed_slave_then_matches_reply() {
    let mut line = line();
    let req = RtuAdu::new(2, 3, vec![0, 7, 0, 1]).unwrap();
    line.request(PortId(1), req, UartFault::None).unwrap();
    let events = drive(&mut line, SimInstant::from_nanos(150_000_000));
    assert!(events.iter().any(|event| matches!(
        event, LineEvent::Master { port: PortId(1), event: MasterEvent::Reply(reply) }
        if reply.address() == 2 && reply.data() == &[2, 0x12, 0x34]
    )));
    assert!(!events.iter().any(|event| matches!(
        event, LineEvent::Master { event: MasterEvent::TimedOut { .. }, .. }
    )));
}

#[test]
fn broadcast_write_updates_both_slaves_without_responses() {
    let mut line = line();
    let broadcast = RtuAdu::new(0, 6, vec![0, 7, 0, 5]).unwrap();
    line.request(PortId(1), broadcast, UartFault::None).unwrap();
    let events = drive(&mut line, SimInstant::from_nanos(100_000_000));
    assert_eq!(line.slave_register(PortId(2), 7), Some(5));
    assert_eq!(line.slave_register(PortId(3), 7), Some(5));
    assert_eq!(events.iter().filter(|event| matches!(event, LineEvent::BroadcastWrite { .. })).count(), 2);
    assert!(!events.iter().any(|event| matches!(event, LineEvent::Master { .. })));
}

#[test]
fn corrupt_request_causes_crc_discard_then_master_timeout() {
    let mut line = line();
    line.request(PortId(1), RtuAdu::new(2, 3, vec![0, 7, 0, 1]).unwrap(),
        UartFault::CorruptByte { index: 3, xor_mask: 0x80 }).unwrap();
    let events = drive(&mut line, SimInstant::from_nanos(150_000_000));
    assert!(events.iter().any(|event| matches!(
        event, LineEvent::Discarded { reason: RtuError::CrcMismatch, .. }
    )));
    assert!(events.iter().any(|event| matches!(
        event, LineEvent::Master { event: MasterEvent::TimedOut { address: 2, function: 3 }, .. }
    )));
    assert!(!events.iter().any(|event| matches!(event,
        LineEvent::Master { event: MasterEvent::Reply(_), .. }
    )));
}

#[test]
fn rejected_overlapping_request_does_not_retransmit() {
    let mut line = line();
    let request = RtuAdu::new(2, 3, vec![0, 7, 0, 1]).unwrap();
    line.request(PortId(1), request.clone(), UartFault::None).unwrap();
    assert_eq!(
        line.request(PortId(1), request, UartFault::None),
        Err(LineError::Session(SessionError::Busy))
    );
}

#[test]
fn long_inter_character_gap_is_discarded_and_times_out() {
    let mut line = line();
    let t = RtuTiming::new(9600, UartFormat::MODBUS_RTU_8E1).unwrap();
    line.request(PortId(1), RtuAdu::new(2, 3, vec![0, 7, 0, 1]).unwrap(),
        UartFault::GapAfter {
            index: 2,
            silence: t.inter_character.checked_add(SimDuration::from_nanos(100)).unwrap(),
        }).unwrap();
    let events = drive(&mut line, SimInstant::from_nanos(150_000_000));
    assert!(events.iter().any(|event| matches!(
        event, LineEvent::Discarded { reason: RtuError::InterCharacterTimeout, .. }
    )));
    assert!(events.iter().any(|event| matches!(
        event, LineEvent::Master { event: MasterEvent::TimedOut { address: 2, function: 3 }, .. }
    )));
}

#[test]
fn unsupported_register_returns_modbus_exception_not_fake_data() {
    let mut line = line();
    let request = RtuAdu::new(2, 3, vec![0, 40, 0, 1]).unwrap();
    line.request(PortId(1), request, UartFault::None).unwrap();
    let events = drive(&mut line, SimInstant::from_nanos(150_000_000));
    assert!(events.iter().any(|event| matches!(
        event, LineEvent::Master {
            event: MasterEvent::Exception { address: 2, function: 3, code: 2 }, ..
        }
    )));
    assert!(!events.iter().any(|event| matches!(
        event, LineEvent::Master { event: MasterEvent::Reply(_), .. }
    )));
}

#[test]
fn simultaneous_drivers_do_not_produce_valid_modbus_responses() {
    let mut line = line();
    line.request(PortId(1), RtuAdu::new(2, 3, vec![0, 7, 0, 1]).unwrap(), UartFault::None).unwrap();
    let interfering = RtuAdu::new(3, 6, vec![0, 7, 0, 8]).unwrap();
    line.network_mut().send_rs485_uart(PortId(3), ven_core::Frame::new(interfering.encode()), UartFault::None).unwrap();
    let events = drive(&mut line, SimInstant::from_nanos(150_000_000));
    assert!(events.iter().any(|event| matches!(event, LineEvent::BusContention { .. })));
    assert!(events.iter().any(|event| matches!(
        event, LineEvent::Master { event: MasterEvent::TimedOut { address: 2, function: 3 }, .. }
    )));
    assert!(!events.iter().any(|event| matches!(
        event, LineEvent::Master { event: MasterEvent::Reply(_), .. }
    )));
}
