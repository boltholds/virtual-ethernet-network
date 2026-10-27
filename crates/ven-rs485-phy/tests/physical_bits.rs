use sim_kernel::{SimulationParticipant, TimeCoordinator, TimeGrant};
use sim_time::{SimDuration, SimInstant};
use ven_core::PortId;
use ven_medium::rs485::{Rs485Turnaround, UartFormat};
use ven_modbus::{ReceiveEvent, RtuAdu, RtuReceiver, RtuTiming};
use ven_rs485_phy::{
    IdleLine, LogicLevel, PhysicalEvent, PhyError,
    Rs485DigitalPhy, TransceiverProfile, UartSample,
};
use ven_topology::BusId;

fn bus(format: UartFormat) -> Rs485DigitalPhy {
    let mut bus = Rs485DigitalPhy::new(
        BusId(1), 9600, format, SimDuration::from_nanos(75), IdleLine::High,
    ).unwrap();
    for id in 1..=3 {
        bus.add_port(PortId(id), TransceiverProfile::default()).unwrap();
    }
    bus
}

fn drive(phy: &mut Rs485DigitalPhy, until: SimInstant) -> Vec<(SimInstant, PhysicalEvent)> {
    let mut clock = TimeCoordinator::new();
    let mut outputs = Vec::new();
    for _ in 0..100_000 {
        let grant = clock.grant(until).unwrap();
        let step = phy.advance(grant).unwrap();
        clock.commit(grant, &step).unwrap();
        outputs.extend(step.outputs.into_iter().map(|item| (item.at, item.value)));
        if step.reached == until { return outputs; }
    }
    panic!("too many events before horizon");
}

#[test]
fn uart_8e1_decodes_lsb_first_start_parity_stop_and_modbus_frame() {
    let mut phy = bus(UartFormat::MODBUS_RTU_8E1);
    let frame = RtuAdu::new(2, 3, vec![0, 7, 0, 1]).unwrap().encode();
    let tx = phy.transmit(PortId(1), frame.clone()).unwrap();
    let all = drive(&mut phy, SimInstant::from_nanos(20_000_000));
    let symbols: Vec<_> = all.iter().filter_map(|(_, e)| match e {
        PhysicalEvent::Character { to: PortId(2), sample: UartSample::Byte(b), .. } => Some(*b),
        _ => None,
    }).collect();
    assert_eq!(symbols, frame);
    let mut rx = RtuReceiver::new(RtuTiming::new(9600, UartFormat::MODBUS_RTU_8E1).unwrap());
    let mut completed = Vec::new();
    for (at, event) in all {
        if let PhysicalEvent::Character { to: PortId(2), sample: UartSample::Byte(b), .. } = event {
            completed.push(rx.push_character(b, at).unwrap());
        }
    }
    assert!(completed.iter().all(|v| matches!(v, ReceiveEvent::Pending)));
    assert!(matches!(rx.flush(SimInstant::from_nanos(20_000_000)), Ok(ReceiveEvent::Frame(_))));
    assert!(tx.data_starts < tx.data_finishes);
    assert_eq!(phy.active_transmission_count(), 0);
}

#[test]
fn traces_start_and_stop_bits_and_idle_bias() {
    let mut phy = bus(UartFormat::MODBUS_RTU_8N2);
    assert_eq!(phy.observe_wire(SimInstant::ZERO), LogicLevel::High);
    phy.set_trace_bits(true);
    phy.transmit(PortId(1), vec![0x01]).unwrap();
    let out = drive(&mut phy, SimInstant::from_nanos(2_000_000));
    let bits: Vec<_> = out.iter().filter_map(|(_, e)| match e {
        PhysicalEvent::SampledBit { to: PortId(2), level, .. } => Some(*level),
        _ => None,
    }).collect();
    assert_eq!(bits.len(), 11);
    assert_eq!(bits[0], LogicLevel::Low); // start
    assert_eq!(bits[1], LogicLevel::High); // bit 0 of 0x01
    assert_eq!(bits[2], LogicLevel::Low); // bit 1
    assert_eq!(bits[9], LogicLevel::High);
    assert_eq!(bits[10], LogicLevel::High);
}

#[test]
fn collision_inside_data_byte_produces_unknown_samples_and_diagnostic() {
    let mut phy = bus(UartFormat::MODBUS_RTU_8E1);
    phy.transmit(PortId(1), vec![0x55]).unwrap();
    let overlap = SimInstant::from_nanos(400_000);
    let mut reached = SimInstant::ZERO;
    for _ in 0..128 {
        let step = phy.advance(TimeGrant { until: overlap }).unwrap();
        reached = step.reached;
        if reached == overlap { break; }
    }
    assert_eq!(reached, overlap);
    phy.transmit(PortId(3), vec![0xaa]).unwrap();
    assert_eq!(phy.observe_wire(overlap), LogicLevel::Indeterminate);
    let result = phy.advance(TimeGrant { until: overlap }).unwrap();
    assert!(result.outputs.iter().any(|e| matches!(&e.value,
        PhysicalEvent::Contention { first: PortId(1), second: PortId(3), .. }
    )));
    let out = drive_remaining(&mut phy, SimInstant::from_nanos(4_000_000));
    assert!(out.iter().any(|e| matches!(&e.value,
        PhysicalEvent::Character { to: PortId(2), sample: UartSample::Indeterminate, .. }
    )));
}

fn drive_remaining(phy: &mut Rs485DigitalPhy, until: SimInstant) -> Vec<sim_kernel::TimedOutput<PhysicalEvent>> {
    let mut result = Vec::new();
    for _ in 0..100_000 {
        let out = phy.advance(TimeGrant { until }).unwrap();
        result.extend(out.outputs);
        if out.reached == until { return result; }
    }
    panic!("too many events");
}

#[test]
fn no_failsafe_biased_bus_reports_unknown_when_idle() {
    let mut phy = Rs485DigitalPhy::new(
        BusId(2), 9600, UartFormat::MODBUS_RTU_8E1,
        SimDuration::ZERO, IdleLine::Floating,
    ).unwrap();
    assert_eq!(phy.observe_wire(SimInstant::ZERO), LogicLevel::Indeterminate);
    phy.add_port(PortId(10), TransceiverProfile::default()).unwrap();
    assert_eq!(phy.transmit(PortId(11), vec![1]), Err(PhyError::UnknownPort));
}

#[test]
fn turnaround_release_and_receiver_deafness_are_explicit() {
    let mut phy = Rs485DigitalPhy::new(
        BusId(2), 9600, UartFormat::MODBUS_RTU_8E1,
        SimDuration::ZERO, IdleLine::High,
    ).unwrap();
    phy.add_port(PortId(1), TransceiverProfile {
        turnaround: Rs485Turnaround {
            driver_enable: SimDuration::from_nanos(100_000),
            driver_release: SimDuration::from_nanos(5_000_000),
            receiver_enable: SimDuration::from_nanos(1_000_000),
        },
    }).unwrap();
    phy.add_port(PortId(2), TransceiverProfile::default()).unwrap();
    let timing = phy.transmit(PortId(1), vec![1]).unwrap();
    assert_eq!(timing.data_starts, SimInstant::from_nanos(100_000));
    assert!(timing.driver_released > timing.data_finishes);
    let early = phy.advance(TimeGrant { until: timing.data_finishes }).unwrap();
    assert!(early.reached <= timing.data_finishes);
    assert_eq!(phy.transmit(PortId(1), vec![2]), Err(PhyError::TransmitterBusy));
    // Note: the other device can transmit, but clashes with DE still held.
    phy.transmit(PortId(2), vec![2]).unwrap();
    assert_eq!(phy.observe_wire(phy.current_time()), LogicLevel::Indeterminate);
}

#[test]
fn zero_baud_and_large_frame_are_rejected() {
    assert!(matches!(Rs485DigitalPhy::new(
        BusId(0), 0, UartFormat::MODBUS_RTU_8E1, SimDuration::ZERO,
        IdleLine::High,
    ), Err(PhyError::InvalidBaudRate)));
    let mut phy = bus(UartFormat::MODBUS_RTU_8E1);
    assert_eq!(phy.transmit(PortId(1), vec![]), Err(PhyError::EmptyFrame));
    assert_eq!(phy.transmit(PortId(1), vec![0; 257]), Err(PhyError::TooManyBytes));
    assert_eq!(phy.active_transmission_count(), 0);
}
