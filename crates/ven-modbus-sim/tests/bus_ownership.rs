use sim_kernel::{TimeCoordinator, TimeGrant};
use sim_kernel::resources::{ParticipantId, RegistryError};
use sim_time::{SimDuration, SimInstant};
use ven_core::PortId;
use ven_medium::rs485::UartFormat;
use ven_modbus::RtuAdu;
use ven_modbus::session::MasterEvent;
use ven_modbus_sim::{HostError, LineEvent, ModbusBus, ModbusBusHost};
use ven_rs485_phy::{IdleLine, Rs485DigitalPhy, TransceiverProfile};
use ven_sim::VenNetwork;
use ven_topology::{BusId, InterfaceKind, NodeId, Port};

fn digital(bus: u32) -> ModbusBus {
    let mut phy = Rs485DigitalPhy::new(
        BusId(bus), 9600, UartFormat::MODBUS_RTU_8E1,
        SimDuration::ZERO, IdleLine::High,
    ).unwrap();
    for id in 1..=3 {
        phy.add_port(PortId(id), TransceiverProfile::default()).unwrap();
    }
    ModbusBus::digital_bits(phy).unwrap()
}

fn character(buses: &[u32]) -> ModbusBus {
    let mut network = VenNetwork::new();
    for id in 1..=3 {
        network.add_node(NodeId(id)).unwrap();
        network.add_port(PortId(id), Port {
            node: NodeId(id), kind: InterfaceKind::Rs485,
        }).unwrap();
    }
    for &bus in buses {
        network.create_rs485_bus(
            BusId(bus), 9600, UartFormat::MODBUS_RTU_8E1, SimDuration::ZERO,
        ).unwrap();
    }
    if let Some(&first) = buses.first() {
        for id in 1..=3 { network.attach_rs485_port(BusId(first), PortId(id)).unwrap(); }
    }
    ModbusBus::character_timed(
        network,
        ven_modbus::RtuTiming::new(9600, UartFormat::MODBUS_RTU_8E1).unwrap(),
    )
}

#[test]
fn cross_lod_duplicate_bus_is_rejected_with_exact_existing_owner() {
    let mut host = ModbusBusHost::new();
    host.register(ParticipantId(1), character(&[17])).unwrap();
    assert_eq!(host.owner_of(BusId(17)), Some(ParticipantId(1)));
    assert_eq!(
        host.register(ParticipantId(2), digital(17)),
        Err(HostError::Registry(RegistryError::ResourceOccupied {
            resource: BusId(17), owner: ParticipantId(1),
        }))
    );
    assert_eq!(host.participant_count(), 1);
    assert_eq!(host.owner_of(BusId(17)), Some(ParticipantId(1)));
}

#[test]
fn multi_bus_registration_is_all_or_nothing() {
    let mut host = ModbusBusHost::new();
    host.register(ParticipantId(1), digital(7)).unwrap();
    assert_eq!(
        host.register(ParticipantId(2), character(&[6, 7, 8])),
        Err(HostError::Registry(RegistryError::ResourceOccupied {
            resource: BusId(7), owner: ParticipantId(1),
        }))
    );
    assert_eq!(host.owner_of(BusId(6)), None);
    assert_eq!(host.owner_of(BusId(8)), None);
    host.register(ParticipantId(3), digital(8)).unwrap();
    assert_eq!(host.owner_of(BusId(8)), Some(ParticipantId(3)));
    assert_eq!(host.participant_count(), 2);
}

#[test]
fn release_frees_all_buses_and_drops_executor() {
    let mut host = ModbusBusHost::new();
    host.register(ParticipantId(10), character(&[7, 8])).unwrap();
    assert_eq!(
        host.register(ParticipantId(10), digital(11)),
        Err(HostError::Registry(RegistryError::AlreadyRegistered {
            participant: ParticipantId(10),
        }))
    );
    host.unregister(ParticipantId(10)).unwrap();
    assert_eq!(host.owner_of(BusId(7)), None);
    assert_eq!(host.owner_of(BusId(8)), None);
    assert_eq!(host.participant_count(), 0);
    assert_eq!(
        host.unregister(ParticipantId(10)),
        Err(HostError::UnknownParticipant { participant: ParticipantId(10) })
    );
    host.register(ParticipantId(11), digital(7)).unwrap();
    assert_eq!(host.owner_of(BusId(7)), Some(ParticipantId(11)));
}

#[test]
fn a_participant_with_no_physical_buses_cannot_register() {
    let mut host = ModbusBusHost::new();
    assert_eq!(
        host.register(ParticipantId(1), character(&[])),
        Err(HostError::Registry(RegistryError::EmptyClaim))
    );
    assert_eq!(host.participant_count(), 0);
}

#[test]
fn registered_digital_bus_can_exchange_modbus_under_external_time_grants() {
    let mut host = ModbusBusHost::new();
    let id = ParticipantId(42);
    host.register(id, digital(77)).unwrap();
    host.attach_master(id, PortId(1), SimDuration::from_nanos(100_000_000)).unwrap();
    host.attach_slave(id, PortId(2), 2).unwrap();
    host.attach_slave(id, PortId(3), 3).unwrap();
    host.seed_register(id, PortId(2), 7, 0x1234).unwrap();
    host.seed_register(id, PortId(3), 7, 0x5678).unwrap();
    let request = RtuAdu::new(2, 3, vec![0, 7, 0, 1]).unwrap();
    host.request(id, PortId(1), request).unwrap();
    let mut clock = TimeCoordinator::new();
    let horizon = SimInstant::from_nanos(150_000_000);
    let mut results = Vec::new();
    for _ in 0..100_000 {
        let grant = clock.grant(horizon).unwrap();
        let result = host.advance_one(id, grant).unwrap();
        clock.commit(grant, &result).unwrap();
        results.extend(result.outputs.into_iter().map(|x| x.value));
        if clock.now() == horizon { break; }
    }
    assert_eq!(clock.now(), horizon);
    assert!(results.iter().any(|event| matches!(
        event,
        LineEvent::Master {
            port: PortId(1), event: MasterEvent::Reply(adu),
        } if adu.address() == 2 && adu.data() == [2, 0x12, 0x34]
    )));
    assert!(!results.iter().any(|event| matches!(
        event, LineEvent::Master { event: MasterEvent::TimedOut { .. }, .. }
    )));
    assert_eq!(host.owner_of(BusId(77)), Some(id));
    assert_eq!(host.slave_register(id, PortId(3), 7), Ok(Some(0x5678)));
}

#[test]
fn registry_scope_is_one_simulation_session_not_a_process_singleton() {
    let mut first = ModbusBusHost::new();
    let mut second = ModbusBusHost::new();
    first.register(ParticipantId(1), digital(17)).unwrap();
    second.register(ParticipantId(2), character(&[17])).unwrap();
    assert_eq!(first.owner_of(BusId(17)), Some(ParticipantId(1)));
    assert_eq!(second.owner_of(BusId(17)), Some(ParticipantId(2)));
}
