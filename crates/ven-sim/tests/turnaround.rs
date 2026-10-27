//! DE/RE switching is a separate physical timing model, not Modbus t3.5.
use sim_kernel::{SimulationParticipant, TimeGrant};
use sim_time::{SimDuration, SimInstant};
use ven_core::{Frame, PortId};
use ven_medium::rs485::{Rs485Turnaround, UartFormat};
use ven_sim::{NetworkOutput, UartFault, VenError, VenNetwork};
use ven_topology::{BusId, InterfaceKind, NodeId, Port};

fn network() -> VenNetwork {
    let mut n = VenNetwork::new();
    for id in 1..=3 {
        n.add_node(NodeId(id)).unwrap();
        n.add_port(PortId(id), Port { node: NodeId(id), kind: InterfaceKind::Rs485 }).unwrap();
    }
    n.create_rs485_bus(BusId(1), 9600, UartFormat::MODBUS_RTU_8E1, SimDuration::ZERO).unwrap();
    for id in 1..=3 { n.attach_rs485_port(BusId(1), PortId(id)).unwrap(); }
    n
}

#[test]
fn de_assert_delay_and_release_hold_are_independent_of_uart_bits() {
    let mut n = network();
    n.set_rs485_turnaround(PortId(1), Rs485Turnaround {
        driver_enable: SimDuration::from_nanos(100_000),
        driver_release: SimDuration::from_nanos(2_000_000),
        receiver_enable: SimDuration::from_nanos(700_000),
    }).unwrap();
    let tx = n.send_rs485_uart(PortId(1), Frame::new(vec![0x55]), UartFault::None).unwrap();
    assert_eq!(tx.starts, SimInstant::from_nanos(100_000));
    assert_eq!(tx.finishes, SimInstant::from_nanos(1_245_834));
    let release = n.driver_busy_until(PortId(1)).unwrap();
    assert_eq!(release, SimInstant::from_nanos(3_245_834));
    assert_eq!(n.receiver_ready_at(PortId(1)).unwrap(), SimInstant::from_nanos(3_945_834));
    let delivered = n.advance(TimeGrant { until: tx.arrives }).unwrap();
    assert_eq!(delivered.outputs.len(), 2);
    assert!(delivered.outputs.iter().all(|x| matches!(x.value, NetworkOutput::UartCharacter { .. })));
    assert_eq!(n.send_rs485_uart(PortId(1), Frame::new(vec![0xaa]), UartFault::None),
        Err(VenError::TransmitterBusy));
    let event = n.advance(TimeGrant { until: release }).unwrap();
    assert_eq!(event.reached, release);
    assert!(event.outputs.iter().any(|x| matches!(x.value,
        NetworkOutput::Rs485DriverReleased { sender: PortId(1), .. })));
    assert!(n.send_rs485_uart(PortId(1), Frame::new(vec![0xaa]), UartFault::None).is_ok());
}

#[test]
fn second_driver_during_release_is_immediate_contention() {
    let mut n = network();
    n.set_rs485_turnaround(PortId(1), Rs485Turnaround {
        driver_enable: SimDuration::ZERO,
        driver_release: SimDuration::from_nanos(2_000_000),
        receiver_enable: SimDuration::ZERO,
    }).unwrap();
    let first = n.send_rs485_uart(PortId(1), Frame::new(vec![1]), UartFault::None).unwrap();
    n.advance(TimeGrant { until: first.finishes }).unwrap();
    assert!(n.driver_busy_until(PortId(1)).unwrap() > n.current_time());
    n.send_rs485_uart(PortId(2), Frame::new(vec![2]), UartFault::None).unwrap();
    let collision = n.advance(TimeGrant { until: n.current_time() }).unwrap();
    assert!(matches!(collision.outputs[0].value, NetworkOutput::Rs485Collision {
        bus: BusId(1), first: PortId(1), second: PortId(2),
    }));
    assert_eq!(collision.reached, first.finishes);
}

#[test]
fn receiver_is_deaf_until_reenabled_after_own_transmission() {
    let mut n = network();
    n.set_rs485_turnaround(PortId(1), Rs485Turnaround {
        driver_enable: SimDuration::ZERO,
        driver_release: SimDuration::ZERO,
        receiver_enable: SimDuration::from_nanos(3_000_000),
    }).unwrap();
    let first = n.send_rs485_uart(PortId(1), Frame::new(vec![1]), UartFault::None).unwrap();
    n.advance(TimeGrant { until: first.finishes }).unwrap();
    n.send_rs485_uart(PortId(2), Frame::new(vec![2]), UartFault::None).unwrap();
    let mut seen_port3 = false;
    let mut seen_port1 = false;
    for _ in 0..10 {
        let out = n.advance(TimeGrant { until: SimInstant::from_nanos(10_000_000) }).unwrap();
        seen_port3 |= out.outputs.iter().any(|x| matches!(x.value,
            NetworkOutput::UartCharacter { to: PortId(3), .. }));
        seen_port1 |= out.outputs.iter().any(|x| matches!(x.value,
            NetworkOutput::UartCharacter { to: PortId(1), .. }));
        if out.reached == SimInstant::from_nanos(10_000_000) { break; }
    }
    assert!(seen_port3);
    assert!(!seen_port1);
}

#[test]
fn opaque_frame_arrival_is_not_delayed_by_driver_release() {
    let mut n = network();
    n.set_rs485_turnaround(PortId(1), Rs485Turnaround {
        driver_enable: SimDuration::ZERO,
        driver_release: SimDuration::from_nanos(2_000_000),
        receiver_enable: SimDuration::ZERO,
    }).unwrap();
    let tx = n.send_rs485(PortId(1), Frame::new(vec![0xa5])).unwrap();
    let held_until = n.driver_busy_until(PortId(1)).unwrap();
    assert!(held_until > tx.arrives);
    let arrival = n.advance(TimeGrant { until: tx.arrives }).unwrap();
    assert_eq!(arrival.reached, tx.arrives);
    assert_eq!(arrival.outputs.iter().filter(|event| matches!(
        event.value, NetworkOutput::Frame(_))).count(), 2);
    assert!(!arrival.outputs.iter().any(|event| matches!(
        event.value, NetworkOutput::Rs485DriverReleased { .. })));
    let release = n.advance(TimeGrant { until: held_until }).unwrap();
    assert!(release.outputs.iter().any(|event| matches!(
        event.value, NetworkOutput::Rs485DriverReleased { sender: PortId(1), .. })));
}
