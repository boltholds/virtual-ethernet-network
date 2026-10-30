use sim_kernel::{SimulationParticipant, TimeCoordinator};
use sim_time::{SimDuration, SimInstant};
use ven_core::PortId;
use ven_medium::rs485::UartFormat;
use ven_modbus::{RtuAdu, RtuTiming};
use ven_modbus::session::MasterEvent;
use ven_modbus_sim::{BusLod, LineError, LineEvent, ModbusBus, SendTiming};
use ven_rs485_phy::{IdleLine, Rs485DigitalPhy, SampleFault, TransceiverProfile, UartSample};
use ven_sim::VenNetwork;
use ven_topology::{BusId, InterfaceKind, NodeId, Port};

fn configured(mode: BusLod) -> ModbusBus {
    let mut bus = match mode {
        BusLod::CharacterTimed => {
            let mut network = VenNetwork::new();
            for id in 1..=3 {
                network.add_node(NodeId(id)).unwrap();
                network.add_port(PortId(id), Port {
                    node: NodeId(id), kind: InterfaceKind::Rs485,
                }).unwrap();
            }
            network.create_rs485_bus(
                BusId(4), 9600, UartFormat::MODBUS_RTU_8E1, SimDuration::ZERO,
            ).unwrap();
            for id in 1..=3 { network.attach_rs485_port(BusId(4), PortId(id)).unwrap(); }
            ModbusBus::character_timed(
                network, RtuTiming::new(9600, UartFormat::MODBUS_RTU_8E1).unwrap(),
            )
        }
        BusLod::DigitalBits => {
            let mut phy = Rs485DigitalPhy::new(
                BusId(4), 9600, UartFormat::MODBUS_RTU_8E1,
                SimDuration::ZERO, IdleLine::High,
            ).unwrap();
            for id in 1..=3 {
                phy.add_port(PortId(id), TransceiverProfile::default()).unwrap();
            }
            ModbusBus::digital_bits(phy).unwrap()
        }
    };
    assert_eq!(bus.lod(), mode);
    bus.attach_master(PortId(1), SimDuration::from_nanos(100_000_000)).unwrap();
    bus.attach_slave(PortId(2), 2).unwrap();
    bus.attach_slave(PortId(3), 3).unwrap();
    bus.seed_register(PortId(2), 7, 0x1234).unwrap();
    bus.seed_register(PortId(3), 7, 0x9999).unwrap();
    bus
}

fn execute(bus: &mut ModbusBus, horizon: SimInstant) -> Vec<LineEvent> {
    let mut coordinator = TimeCoordinator::new();
    let mut results = Vec::new();
    for _ in 0..50_000 {
        let grant = coordinator.grant(horizon).unwrap();
        let outcome = bus.advance(grant).unwrap();
        coordinator.commit(grant, &outcome).unwrap();
        results.extend(outcome.outputs.into_iter().map(|item| item.value));
        if outcome.reached == horizon { return results; }
    }
    panic!("exceeded maximum event count: scheduling loop suspected");
}

#[test]
fn full_fc03_roundtrip_matches_across_both_lods() {
    for lod in [BusLod::CharacterTimed, BusLod::DigitalBits] {
        let mut bus = configured(lod);
        let request = RtuAdu::new(2, 3, vec![0, 7, 0, 1]).unwrap();
        let receipt = bus.request(PortId(1), request).unwrap();
        assert!(matches!((lod, receipt),
            (BusLod::CharacterTimed, SendTiming::Character(_)) |
            (BusLod::DigitalBits, SendTiming::Digital(_))));
        let events = execute(&mut bus, SimInstant::from_nanos(150_000_000));
        assert!(events.iter().any(|event| matches!(
            event,
            LineEvent::Master { port: PortId(1), event: MasterEvent::Reply(adu) }
                if adu.address() == 2 && adu.data() == [2, 0x12, 0x34]
        )));
        assert!(!events.iter().any(|event| matches!(
            event, LineEvent::Master { event: MasterEvent::TimedOut { .. }, .. }
        )));
        assert!(!events.iter().any(|event| matches!(
            event, LineEvent::PhysicalReceiveError { .. } | LineEvent::BusContention { .. }
        )));
    }
}

#[test]
fn broadcast_write_has_no_slave_replies_in_either_lod() {
    for lod in [BusLod::CharacterTimed, BusLod::DigitalBits] {
        let mut bus = configured(lod);
        bus.request(PortId(1), RtuAdu::new(0, 6, vec![0, 7, 0, 9]).unwrap()).unwrap();
        let events = execute(&mut bus, SimInstant::from_nanos(100_000_000));
        assert_eq!(bus.slave_register(PortId(2), 7), Some(9));
        assert_eq!(bus.slave_register(PortId(3), 7), Some(9));
        assert_eq!(events.iter().filter(|e| matches!(e, LineEvent::BroadcastWrite { .. })).count(), 2);
        assert!(!events.iter().any(|e| matches!(e, LineEvent::Master { .. })));
    }
}

#[test]
fn digital_phy_parity_fault_causes_timeout_not_false_response() {
    let mut bus = configured(BusLod::DigitalBits);
    bus.digital_mut().unwrap().phy_mut()
        .inject_sample_fault(PortId(2), 0, 9, SampleFault::Flip).unwrap();
    bus.request(PortId(1), RtuAdu::new(2, 3, vec![0, 7, 0, 1]).unwrap()).unwrap();
    let events = execute(&mut bus, SimInstant::from_nanos(150_000_000));
    assert!(events.iter().any(|e| matches!(
        e, LineEvent::PhysicalReceiveError {
            port: PortId(2), sample: UartSample::ParityError,
        }
    )));
    assert!(events.iter().any(|e| matches!(
        e, LineEvent::Master { event: MasterEvent::TimedOut { address: 2, .. }, .. }
    )));
    assert!(!events.iter().any(|e| matches!(
        e, LineEvent::Master { event: MasterEvent::Reply(_), .. }
    )));
}

#[test]
fn digital_phy_contention_invalidates_modbus_frame() {
    let mut bus = configured(BusLod::DigitalBits);
    bus.request(PortId(1), RtuAdu::new(2, 3, vec![0, 7, 0, 1]).unwrap()).unwrap();
    let interfering = RtuAdu::new(3, 6, vec![0, 7, 0, 1]).unwrap().encode();
    bus.digital_mut().unwrap().phy_mut().transmit(PortId(3), interfering).unwrap();
    let events = execute(&mut bus, SimInstant::from_nanos(150_000_000));
    assert!(events.iter().any(|e| matches!(e, LineEvent::BusContention { bus: BusId(4), .. })));
    assert!(events.iter().any(|e| matches!(
        e, LineEvent::Master { event: MasterEvent::TimedOut { address: 2, .. }, .. }
    )));
    assert!(!events.iter().any(|e| matches!(
        e, LineEvent::Master { event: MasterEvent::Reply(_), .. }
    )));
}

#[test]
fn invalid_rtu_phy_configuration_cannot_silently_fall_back() {
    let no_parity_one_stop = UartFormat {
        data_bits: 8, parity: ven_medium::rs485::Parity::None, stop_bits: 1,
    };
    let phy = Rs485DigitalPhy::new(
        BusId(5), 9600, no_parity_one_stop,
        SimDuration::ZERO, IdleLine::High,
    ).unwrap();
    assert!(matches!(ModbusBus::digital_bits(phy), Err(LineError::Protocol(_))));
}

#[test]
fn no_duplicate_slave_address_on_digital_bus() {
    let mut bus = configured(BusLod::DigitalBits);
    assert_eq!(
        bus.attach_slave(PortId(3), 2),
        Err(LineError::DuplicatePort),
    );
}

#[test]
fn two_slaves_cannot_claim_one_modbus_address_on_a_digital_bus() {
    let mut phy = Rs485DigitalPhy::new(
        BusId(7), 9600, UartFormat::MODBUS_RTU_8E1,
        SimDuration::ZERO, IdleLine::High,
    ).unwrap();
    phy.add_port(PortId(2), TransceiverProfile::default()).unwrap();
    phy.add_port(PortId(3), TransceiverProfile::default()).unwrap();
    let mut bus = ModbusBus::digital_bits(phy).unwrap();
    bus.attach_slave(PortId(2), 2).unwrap();
    assert_eq!(
        bus.attach_slave(PortId(3), 2),
        Err(LineError::DuplicateSlaveAddress),
    );
}
