//! UART character events are produced by VEN itself, not synthesized by a test helper.
use sim_kernel::{SimulationParticipant, TimeGrant};
use sim_time::{SimDuration, SimInstant};
use ven_core::{Frame, PortId};
use ven_medium::rs485::UartFormat;
use ven_modbus::{ReceiveEvent, RtuAdu, RtuReceiver, RtuTiming};
use ven_sim::{NetworkOutput, UartFault, VenNetwork};
use ven_topology::{BusId, InterfaceKind, NodeId, Port};

fn network() -> VenNetwork {
    let mut net = VenNetwork::new();
    for id in 1..=3 {
        net.add_node(NodeId(id)).unwrap();
        net.add_port(PortId(id), Port { node: NodeId(id), kind: InterfaceKind::Rs485 }).unwrap();
    }
    net.create_rs485_bus(BusId(1), 9600, UartFormat::MODBUS_RTU_8E1, SimDuration::ZERO).unwrap();
    for id in 1..=3 { net.attach_rs485_port(BusId(1), PortId(id)).unwrap(); }
    net
}

fn run_receiver(fault: UartFault) -> Vec<ReceiveEvent> {
    let mut net = network();
    let timing = RtuTiming::new(9600, UartFormat::MODBUS_RTU_8E1).unwrap();
    let adu = RtuAdu::new(2, 3, vec![0, 0, 0, 10]).unwrap();
    net.send_rs485_uart(PortId(1), Frame::new(adu.encode()), fault).unwrap();
    let mut rx = RtuReceiver::new(timing);
    let mut observations = Vec::new();
    let horizon = SimInstant::from_nanos(100_000_000);
    for _ in 0..100 {
        let step = net.advance(TimeGrant { until: horizon }).unwrap();
        for output in step.outputs {
            if let NetworkOutput::UartCharacter { to: PortId(2), byte, .. } = output.value {
                observations.push(rx.push_character(byte, output.at).unwrap());
            }
        }
        if step.reached == horizon { break; }
    }
    observations.push(rx.flush(horizon).unwrap());
    observations
}

#[test]
fn actual_uart_character_events_decode_frame() {
    let observations = run_receiver(UartFault::None);
    assert!(observations.iter().all(|x| !matches!(x, ReceiveEvent::Discarded(_))));
    assert!(observations.iter().any(|x| matches!(x, ReceiveEvent::Frame(adu) if adu.address() == 2 && adu.function() == 3)));
}

#[test]
fn dropped_character_fails_crc_or_length() {
    let observations = run_receiver(UartFault::DropByte { index: 3 });
    assert!(observations.iter().any(|x| matches!(x, ReceiveEvent::Discarded(_))));
    assert!(!observations.iter().any(|x| matches!(x, ReceiveEvent::Frame(_))));
}

#[test]
fn corrupted_character_fails_crc() {
    let observations = run_receiver(UartFault::CorruptByte { index: 3, xor_mask: 0x80 });
    assert!(observations.iter().any(|x| matches!(x, ReceiveEvent::Discarded(_))));
    assert!(!observations.iter().any(|x| matches!(x, ReceiveEvent::Frame(_))));
}

#[test]
fn mid_frame_silence_triggers_t15_discard() {
    let timing = RtuTiming::new(9600, UartFormat::MODBUS_RTU_8E1).unwrap();
    let gap = timing.inter_character.checked_add(SimDuration::from_nanos(10)).unwrap();
    let observations = run_receiver(UartFault::GapAfter { index: 1, silence: gap });
    assert!(observations.iter().any(|x| matches!(x, ReceiveEvent::Discarded(ven_modbus::RtuError::InterCharacterTimeout))));
    assert!(!observations.iter().any(|x| matches!(x, ReceiveEvent::Frame(_))));
}
