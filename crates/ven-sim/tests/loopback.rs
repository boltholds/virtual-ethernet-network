use sim_kernel::{SimulationParticipant, TimeCoordinator, TimeGrant};
use sim_time::{SimDuration, SimInstant};
use ven_core::{Frame, PortId};
use ven_medium::rs485::{Rs485Error, UartFormat};
use ven_sim::{NetworkOutput, VenError, VenNetwork};
use ven_topology::{BusId, InterfaceKind, LinkId, NodeId, Port, TopologyError};

fn connected_network(propagation: SimDuration) -> VenNetwork {
    let mut n = VenNetwork::new();
    n.add_node(NodeId(1)).unwrap();
    n.add_node(NodeId(2)).unwrap();
    n.add_port(PortId(1), Port { node: NodeId(1), kind: InterfaceKind::Ethernet }).unwrap();
    n.add_port(PortId(2), Port { node: NodeId(2), kind: InterfaceKind::Ethernet }).unwrap();
    n.connect_ethernet(LinkId(1), PortId(1), PortId(2), 1_000_000, propagation).unwrap();
    n
}

fn rs485_network() -> VenNetwork {
    let mut n = VenNetwork::new();
    for id in 1..=3 {
        n.add_node(NodeId(id)).unwrap();
        n.add_port(PortId(id), Port {
            node: NodeId(id), kind: InterfaceKind::Rs485,
        }).unwrap();
    }
    n.create_rs485_bus(
        BusId(1), 9600, UartFormat::MODBUS_RTU_8E1,
        SimDuration::from_nanos(100),
    ).unwrap();
    for id in 1..=3 {
        n.attach_rs485_port(BusId(1), PortId(id)).unwrap();
    }
    n
}

#[test]
fn ethernet_arrival_returns_early_to_shared_clock() {
    let mut coordinator = TimeCoordinator::new();
    let mut network = connected_network(SimDuration::from_nanos(20));
    let tx = network.send(PortId(1), PortId(2), Frame::new(vec![0xab; 125])).unwrap();
    assert_eq!(tx.arrives, SimInstant::from_nanos(1_000_020));
    let grant = coordinator.grant(SimInstant::from_nanos(10_000_000)).unwrap();
    let result = network.advance(grant).unwrap();
    assert_eq!(result.reached, tx.arrives);
    assert_eq!(result.outputs.len(), 1);
    assert_eq!(result.outputs[0].at, tx.arrives);
    let NetworkOutput::Frame(arrival) = &result.outputs[0].value else {
        panic!("expected Ethernet frame arrival");
    };
    assert_eq!(arrival.to, PortId(2));
    assert_eq!(arrival.frame.bytes.len(), 125);
    coordinator.commit(grant, &result).unwrap();
    assert_eq!(coordinator.now(), network.current_time());
    let grant = coordinator.grant(SimInstant::from_nanos(10_000_000)).unwrap();
    let next = network.advance(grant).unwrap();
    assert!(next.outputs.is_empty());
    coordinator.commit(grant, &next).unwrap();
}

#[test]
fn full_duplex_equal_time_arrivals_are_batched() {
    let mut n = connected_network(SimDuration::ZERO);
    n.send(PortId(1), PortId(2), Frame::new(vec![1; 1])).unwrap();
    n.send(PortId(2), PortId(1), Frame::new(vec![2; 1])).unwrap();
    let out = n.advance(TimeGrant { until: SimInstant::from_nanos(1_000_000) }).unwrap();
    assert_eq!(out.reached.as_nanos(), 8_000);
    assert_eq!(out.outputs.len(), 2);
}

#[test]
fn incompatible_connection_is_atomic() {
    let mut n = VenNetwork::new();
    n.add_node(NodeId(1)).unwrap();
    n.add_node(NodeId(2)).unwrap();
    n.add_port(PortId(1), Port { node: NodeId(1), kind: InterfaceKind::Ethernet }).unwrap();
    n.add_port(PortId(2), Port { node: NodeId(2), kind: InterfaceKind::Rs485 }).unwrap();
    assert_eq!(
        n.connect_ethernet(LinkId(1), PortId(1), PortId(2), 1_000_000, SimDuration::ZERO),
        Err(VenError::Topology(TopologyError::IncompatibleInterfaces))
    );
    assert!(n.topology().links().next().is_none());
}

#[test]
fn rs485_one_sender_fans_out_to_other_nodes() {
    let mut n = rs485_network();
    let transmission = n.send_rs485(PortId(1), Frame::new(vec![0xaa; 1])).unwrap();
    assert_eq!(transmission.finishes.as_nanos(), 1_145_834);
    assert_eq!(transmission.arrives.as_nanos(), 1_145_934);
    let out = n.advance(TimeGrant { until: SimInstant::from_nanos(10_000_000) }).unwrap();
    assert_eq!(out.reached, transmission.arrives);
    assert_eq!(out.outputs.len(), 2);
    let targets: Vec<_> = out.outputs.iter().map(|x| {
        match &x.value {
            NetworkOutput::Frame(arrival) => arrival.to,
            NetworkOutput::Rs485Contention { .. } => panic!("not contended"),
        }
    }).collect();
    assert_eq!(targets, vec![PortId(2), PortId(3)]);
}

#[test]
fn simultaneous_rs485_drivers_report_contention_without_valid_frames() {
    let mut n = rs485_network();
    n.send_rs485(PortId(1), Frame::new(vec![1])).unwrap();
    n.send_rs485(PortId(2), Frame::new(vec![2])).unwrap();
    let at = SimInstant::from_nanos(1_145_934);
    let out = n.advance(TimeGrant { until: at }).unwrap();
    assert_eq!(out.outputs.len(), 2);
    assert!(out.outputs.iter().all(|event| {
        matches!(&event.value, NetworkOutput::Rs485Contention { bus: BusId(1), .. })
    }));
}

#[test]
fn staggered_overlap_invalidates_both_transmissions() {
    let mut n = rs485_network();
    n.send_rs485(PortId(1), Frame::new(vec![1])).unwrap();
    n.advance(TimeGrant { until: SimInstant::from_nanos(500_000) }).unwrap();
    let second = n.send_rs485(PortId(2), Frame::new(vec![2])).unwrap();
    assert_eq!(second.starts.as_nanos(), 500_000);
    let first_out = n.advance(TimeGrant { until: SimInstant::from_nanos(2_000_000) }).unwrap();
    assert!(matches!(&first_out.outputs[0].value, NetworkOutput::Rs485Contention { sender: PortId(1), .. }));
    let second_out = n.advance(TimeGrant { until: SimInstant::from_nanos(2_000_000) }).unwrap();
    assert!(matches!(&second_out.outputs[0].value, NetworkOutput::Rs485Contention { sender: PortId(2), .. }));
}

#[test]
fn adjacent_transmissions_are_not_collisions() {
    let mut n = rs485_network();
    let first = n.send_rs485(PortId(1), Frame::new(vec![1])).unwrap();
    n.advance(TimeGrant { until: first.finishes }).unwrap();
    let second = n.send_rs485(PortId(2), Frame::new(vec![2])).unwrap();
    assert_eq!(second.starts, first.finishes);
    let first_out = n.advance(TimeGrant { until: first.arrives }).unwrap();
    assert!(first_out.outputs.iter().all(|x| matches!(&x.value, NetworkOutput::Frame(_))));
    let second_out = n.advance(TimeGrant { until: second.arrives }).unwrap();
    assert!(second_out.outputs.iter().all(|x| matches!(&x.value, NetworkOutput::Frame(_))));
}

#[test]
fn busy_uart_cannot_start_second_overlapping_frame() {
    let mut n = rs485_network();
    n.send_rs485(PortId(1), Frame::new(vec![1])).unwrap();
    assert_eq!(n.send_rs485(PortId(1), Frame::new(vec![2])), Err(VenError::TransmitterBusy));
    let out = n.advance(TimeGrant { until: SimInstant::from_nanos(10_000_000) }).unwrap();
    assert_eq!(out.outputs.len(), 2);
}

#[test]
fn rejected_bus_creation_does_not_mutate_topology() {
    let mut n = VenNetwork::new();
    assert_eq!(
        n.create_rs485_bus(BusId(1), 0, UartFormat::MODBUS_RTU_8E1, SimDuration::ZERO),
        Err(VenError::Rs485(Rs485Error::InvalidBaudRate))
    );
    assert!(n.topology().bus(BusId(1)).is_none());
}
