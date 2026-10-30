//! Compare a real (or explicitly synthetic) canonical scope CSV to VEN digital PHY.
//! No automatic waveform alignment, no implicit claim of physical validation.
//!
//! cargo run -p ven-rs485-phy --example compare_capture -- \
//!   <csv> <baud> <hex-rtu-adu> <low-mv> <high-mv> <max-sample-age-ns> <ro-delay-ns>

use sim_kernel::{SimulationParticipant, TimeGrant, TimedOutput};
use sim_time::{SimDuration, SimInstant};
use std::{env, error::Error, fs};
use ven_core::PortId;
use ven_medium::rs485::UartFormat;
use ven_rs485_phy::{
    trace::{
        expected_from_phy, Channel, ObservationPoint, SampleFinding, ScopeTrace, Thresholds,
    },
    IdleLine, PhysicalEvent, Rs485DigitalPhy, TransceiverProfile,
};
use ven_topology::BusId;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() != 7 {
        return Err(
            "usage: compare_capture <csv> <baud> <hex-rtu-adu> <low-mv> <high-mv> <max-sample-age-ns> <ro-delay-ns>"
                .into()
        );
    }
    let capture_file = &args[0];
    let baud: u64 = args[1].parse()?;
    let hex = &args[2];
    if hex.is_empty() || hex.len() % 2 != 0 {
        return Err("hex-rtu-adu must have an even non-zero number of digits".into());
    }
    let data: Result<Vec<u8>, _> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
        .collect();
    let data = data?;
    let thresholds = Thresholds {
        low_mv: args[3].parse()?,
        high_mv: args[4].parse()?,
    };
    thresholds.validate()?;
    let max_age = SimDuration::from_nanos(args[5].parse()?);
    let ro_delay = SimDuration::from_nanos(args[6].parse()?);

    let csv = fs::read_to_string(capture_file)?;
    let scope = ScopeTrace::from_canonical_csv(&csv)?;
    let measured_trigger = scope.trigger_rising(Channel::De, thresholds)?;

    // Known ideal UART configuration for this example: 8E1. For other UART
    // formats, build the PHY explicitly from your actual device settings.
    let mut phy = Rs485DigitalPhy::new(
        BusId(1), baud, UartFormat::MODBUS_RTU_8E1,
        SimDuration::ZERO, IdleLine::High,
    )?;
    phy.add_port(PortId(1), TransceiverProfile::default())?;
    phy.add_port(PortId(2), TransceiverProfile::default())?;
    phy.set_trace_bits(true);
    let tx = phy.transmit(PortId(1), data)?;
    let horizon = tx.receiver_ready.checked_add(SimDuration::from_nanos(1_000_000))
        .ok_or("horizon overflow")?;
    let mut simulated = Vec::<TimedOutput<PhysicalEvent>>::new();
    for _ in 0..100_000 {
        let step = phy.advance(TimeGrant { until: horizon })?;
        simulated.extend(step.outputs);
        if step.reached == horizon { break; }
    }
    let expected = expected_from_phy(
        &simulated,
        PortId(1), PortId(2), SimInstant::ZERO,
        ObservationPoint::ReceiverOutput { propagation: ro_delay },
    )?;
    if expected.is_empty() {
        return Err("simulation emitted no observable bit samples".into());
    }
    let (summary, findings) = scope.compare_bits(
        measured_trigger, Channel::Ro, thresholds, max_age, &expected,
    )?;
    println!("Capture: {capture_file}");
    println!("DE trigger: {} ns; predicted bits: {}", measured_trigger.as_nanos(), expected.len());
    println!(
        "match={} mismatch={} missing={} uncertain_measurement={} uncertain_prediction={}",
        summary.matched, summary.mismatched, summary.missing,
        summary.uncertain_measured, summary.uncertain_predicted,
    );
    for (index, finding) in findings.iter().enumerate() {
        if *finding != SampleFinding::Match {
            println!(
                "bit {} at +{} ns: {:?}",
                index, expected[index].after_trigger.as_nanos(), finding,
            );
        }
    }
    println!("These results compare DIGITAL SAMPLE LEVELS; they do not validate analog voltage, cable physics, or timing against a calibrated real hardware model.");
    if !summary.all_matched(expected.len()) {
        return Err("waveform comparison contains mismatches or unverified points".into());
    }
    Ok(())
}
