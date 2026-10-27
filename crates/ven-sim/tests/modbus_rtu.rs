//! A protocol-frame vertical slice over existing RS-485, not a byte-accurate bus.
use sim_kernel::{SimulationParticipant, TimeGrant};
use sim_time::{SimDuration, SimInstant};
use ven_core::{Frame, PortId};
use ven_medium::rs485::UartFormat;
use ven_modbus::{RtuAdu, RtuTiming};
use ven_sim::{NetworkOutput, VenNetwork};
use ven_topology::{BusId, InterfaceKind, NodeId, Port};

fn network() -> VenNetwork {
    let mut net = VenNetwork::new();
    for id in 1..=3 {
        net.add_node(NodeId(id)).unwrap();
        net.add_port(PortId(id), Port {
            node: NodeId(id), kind: InterfaceKind::Rs485,
        }).unwrap();
    }
    net.create_rs485_bus(
        BusId(1), 9600, UartFormat::MODBUS_RTU_8E1,
        SimDuration::from_nanos(100),
    ).unwrap();
    for id in 1..=3 {
        net.attach_rs485_port(BusId(1), PortId(id)).unwrap();
    }
    net
}

#[test]
fn modbus_rtu_request_broadcast_on_shared_medium_then_slave_reply() {
    let mut net = network();
    let timing = RtuTiming::new(9600, UartFormat::MODBUS_RTU_8E1).unwrap();
    let request = RtuAdu::new(2, 3, vec![0, 0, 0, 10]).unwrap();
    let transmit = net.send_rs485(PortId(1), Frame::new(request.encode())).unwrap();

    let event = net.advance(TimeGrant {
        until: SimInstant::from_nanos(50_000_000)
    }).unwrap();
    assert_eq!(event.reached, transmit.arrives);
    // Physical broadcast reception on the common bus is not Modbus application
    // addressing; a protocol-aware recipient must filter by RTU address.
    assert_eq!(event.outputs.len(), 2);
    for output in event.outputs {
        let NetworkOutput::Frame(frame) = output.value else {
            panic!("unexpected physical bus contention");
        };
        assert_eq!(RtuAdu::decode(&frame.frame.bytes).unwrap(), request);
    }

    let silence_ends = transmit.arrives.checked_add(timing.inter_frame).unwrap();
    net.advance(TimeGrant { until: silence_ends }).unwrap();
    let reply = RtuAdu::new(2, 3, vec![2, 0, 9]).unwrap();
    let reply_tx = net.send_rs485(PortId(2), Frame::new(reply.encode())).unwrap();
    let result = net.advance(TimeGrant {
        until: reply_tx.arrives
    }).unwrap();
    assert_eq!(result.outputs.len(), 2);
    for event in result.outputs {
        let NetworkOutput::Frame(received) = event.value else {
            panic!("unexpected contention");
        };
        assert_eq!(received.from, PortId(2));
        assert_eq!(RtuAdu::decode(&received.frame.bytes).unwrap(), reply);
    }
}

#[test]
fn two_modbus_transmitters_conflict_instead_of_auto_retransmitting() {
    let mut net = network();
    let a = RtuAdu::new(1, 3, vec![1]).unwrap();
    let b = RtuAdu::new(2, 3, vec![2]).unwrap();
    net.send_rs485(PortId(1), Frame::new(a.encode())).unwrap();
    net.send_rs485(PortId(2), Frame::new(b.encode())).unwrap();
    let result = net.advance(TimeGrant { until: SimInstant::from_nanos(100_000_000) }).unwrap();
    assert_eq!(result.outputs.len(), 2);
    assert!(result.outputs.iter().all(|event| {
        matches!(&event.value, NetworkOutput::Rs485Contention { .. })
    }));
}
