use sim_kernel::{SimulationParticipant, TimeCoordinator};
use sim_time::{SimDuration, SimInstant};
use ven_core::{Frame, PortId};
use ven_medium::FullDuplexLink;
use ven_sim::VenNetwork;

#[test]
fn delivery_returns_early_to_shared_clock() {
    let mut coordinator = TimeCoordinator::new();
    let mut network = VenNetwork::new();
    network
        .add_link(
            FullDuplexLink::new(
                PortId(1),
                PortId(2),
                1_000_000,
                SimDuration::from_nanos(20),
            )
            .unwrap(),
        )
        .unwrap();

    let tx = network
        .send(PortId(1), PortId(2), Frame::new(vec![0xab; 125]))
        .unwrap();
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
    let mut network = VenNetwork::new();
    network
        .add_link(
            FullDuplexLink::new(PortId(1), PortId(2), 1_000_000, SimDuration::ZERO)
                .unwrap(),
        )
        .unwrap();
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
