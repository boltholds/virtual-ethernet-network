use sim_kernel::{SimulationParticipant, TimeCoordinator};
use sim_time::{SimDuration, SimInstant};
use ven_core::{Frame, PortId};
use ven_sim::VenNetwork;
use ven_topology::{InterfaceKind, LinkId, NodeId, Port, TopologyError};

fn connected_network(propagation: SimDuration) -> VenNetwork {
    let mut network = VenNetwork::new();
    network.add_node(NodeId(1)).unwrap();
    network.add_node(NodeId(2)).unwrap();
    network.add_port(PortId(1), Port { node: NodeId(1), kind: InterfaceKind::Ethernet }).unwrap();
    network.add_port(PortId(2), Port { node: NodeId(2), kind: InterfaceKind::Ethernet }).unwrap();
    network.connect_ethernet(LinkId(1), PortId(1), PortId(2), 1_000_000, propagation).unwrap();
    network
}

#[test]
fn delivery_returns_early_to_shared_clock() {
    let mut coordinator = TimeCoordinator::new();
    let mut network = connected_network(SimDuration::from_nanos(20));

    let tx = network.send(PortId(1), PortId(2), Frame::new(vec![0xab; 125])).unwrap();
    assert_eq!(tx.arrives, SimInstant::from_nanos(1_000_020));

    let grant = coordinator.grant(SimInstant::from_nanos(10_000_000)).unwrap();
    let result = network.advance(grant).unwrap();
    assert_eq!(result.reached, tx.arrives);
    assert_eq!(result.outputs.len(), 1);
    assert_eq!(result.outputs[0].at, tx.arrives);
    assert_eq!(result.outputs[0].value.to, PortId(2));
    assert_eq!(result.outputs[0].value.frame.bytes.len(), 125);

    coordinator.commit(grant, &result).unwrap();
    assert_eq!(coordinator.now(), network.current_time());

    let grant = coordinator.grant(SimInstant::from_nanos(10_000_000)).unwrap();
    let next = network.advance(grant).unwrap();
    assert!(next.outputs.is_empty());
    assert_eq!(next.reached, grant.until);
    coordinator.commit(grant, &next).unwrap();
    assert_eq!(coordinator.now(), SimInstant::from_nanos(10_000_000));
}

#[test]
fn equal_time_arrivals_are_batched() {
    let mut network = connected_network(SimDuration::ZERO);
    network.send(PortId(1), PortId(2), Frame::new(vec![1; 1])).unwrap();
    network.send(PortId(2), PortId(1), Frame::new(vec![2; 1])).unwrap();

    let result = network.advance(sim_kernel::TimeGrant {
        until: SimInstant::from_nanos(1_000_000),
    }).unwrap();
    assert_eq!(result.reached.as_nanos(), 8_000);
    assert_eq!(result.outputs.len(), 2);
    assert_eq!(result.outputs[0].value.to, PortId(2));
    assert_eq!(result.outputs[1].value.to, PortId(1));
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
        Err(ven_sim::VenError::Topology(TopologyError::IncompatibleInterfaces))
    );
    assert!(n.topology().links().next().is_none());
}
