# RS-485 digital PHY — scope and validation

Crate: `ven-rs485-phy` (`publish = false`), independently implementable as `sim_kernel::SimulationParticipant`.

## Exactly what is modeled

- A logical 2-wire multidrop bus, typed by `BusId`; each endpoint has a `PortId` and configurable `Rs485Turnaround`.
- UART 8-data-bit serial format (start = logical low, 8 payload bits least significant first, configurable even/odd/no parity, one/two stop bits). 9600-baud Modbus 8E1/8N2 examples have 11 bits per character.
- Integer nanosecond time; Nth bit begins at `ceil(N * 1e9 / baud)` after the data start, and receiver samples at `ceil((2N+1) * 1e9 / (2 * baud))`. One common, user-configured propagation delay is added to the sample timestamp.
- Explicit data-start delay after DE assertion, DE release hold after last stop bit, and later RX re-enable. These are generic timing inputs; **there are no claimed vendor-specific transceiver presets**.
- Active drivers contribute logical High/Low. Two or more simultaneously enabled drivers force `Indeterminate` under the **conservative modeling policy** even if their bits are equal; this is not an electrical resolution algorithm.
- With no active driver, `IdleLine::High` assumes effective failsafe bias or an internally failsafe receiver; `IdleLine::Floating` explicitly leaves the logical level indeterminate. Neither option computes the voltage level or bias resistor values.
- A receiver reports `UartSample::Byte`, `ParityError`, `FramingError`, `Indeterminate`, or `ReceiverDisabled`. `PhysicalEvent::Contention` and `DriverReleased` are simulator diagnostics.
- Opt-in `set_trace_bits(true)` surfaces individual receiver bit samples. `inject_sample_fault` supports a one-shot bit flip or indeterminate sample **at a receiver**, without claiming the disturbance applies to all receivers.
- At most 256 bytes per `transmit` in this first implementation. This is a simulation resource safeguard, **not** a physical RS-485 frame-size limit; the Modbus RTU ADU bound is defined separately.

## Example

```rust
use sim_kernel::{SimulationParticipant, TimeGrant};
use sim_time::{SimDuration, SimInstant};
use ven_core::PortId;
use ven_medium::rs485::UartFormat;
use ven_rs485_phy::{IdleLine, Rs485DigitalPhy, TransceiverProfile};
use ven_topology::BusId;

let mut phy = Rs485DigitalPhy::new(
    BusId(1), 9600, UartFormat::MODBUS_RTU_8E1,
    SimDuration::from_nanos(75), IdleLine::High,
).unwrap();
phy.add_port(PortId(1), TransceiverProfile::default()).unwrap();
phy.add_port(PortId(2), TransceiverProfile::default()).unwrap();
phy.transmit(PortId(1), vec![0x55]).unwrap();

let mut events = Vec::new();
loop {
    let step = phy.advance(TimeGrant { until: SimInstant::from_nanos(5_000_000) }).unwrap();
    events.extend(step.outputs);
    if step.reached == SimInstant::from_nanos(5_000_000) { break; }
}
```

The bit-level `DigitalModbusRtuLine` adapter in `ven-modbus-sim` now consumes PHY character events directly, propagates parity/framing/indeterminate sample failures as typed diagnostics, and uses the same Modbus FC03/FC06 endpoint logic. `ModbusBus` selects one transport LOD per bus instance; the previous raw-PHY tests are still useful independent reference checks.

## Manufacturer profiles and scope comparison

The `profiles` module provides pinned datasheet timing tables for ADM3485E and THVD1450 with distinct typical and maximum values, separate receiver-active vs receiver-from-shutdown timing paths, and an **opt-in guard-planning approximation**. These are not calibrated pin-level waveforms or unconditional temperature/voltage guarantees.

The `trace` module ingests canonical oscilloscope CSV samples (time_ns,channel,millivolts). It compares explicitly predicted UART **bit-center** levels against captured samples using a separately measured DE-rising trigger. It never aligns by the first RO edge, which would erase actual propagation-delay differences. Missing, ambiguous, mismatched and indeterminate predictions remain distinct outcomes. Current tests use only **synthetic traces**; no actual hardware capture has been provided or validated.

See [TRANSCEIVER_PROFILES.md](TRANSCEIVER_PROFILES.md) for pin conditions, capture format, reference sources and how to perform a real comparison.

## Boundaries and future work

- This is a **digital sampled logic model, not an analog line solver**: no true differential voltages, receiver ±200 mV thresholds, hysteresis, termination, common-mode, differential impedance, failsafe-bias network sizing, reflections, slew rates, propagation by trunk/stub length, cable damage or electromagnetic noise power.
- Colliding drivers do not always corrupt physical bits in the same way; our `Indeterminate` policy is conservative, not a certified hardware prediction. There is no actual collision-sense or auto-retransmission mechanism.
- The UART receiver's ideal clock samples near bit centers; no independently drifting receiver clocks, oversampling, framing recovery, metastability, start-bit detection from noise or variable per-receiver propagation.
- In `IdleLine::High`, the HIGH level is a configured **assumption**; a real terminated cable with insufficient failsafe bias can be indeterminate.
- The sample scheduler is O(number of receivers × characters × bits), intended for high-fidelity small test networks, not unbounded large-scale PHY simulation.
- This crate remains **standalone/opt-in**. `ModbusBus` now selects one transport per instance, preserving the single virtual-time participant. A future multi-participant coordinator must still detect duplicate ownership of a physical BusId across independent instances.

## Primary grounding

- Analog Devices, [AN-960: RS-485/RS-422 Circuit Implementation Guide](https://www.analog.com/en/resources/app-notes/an-960.html): receiver threshold, termination/failsafe and wiring assumptions.
- Texas Instruments, [SSZT656: failsafe biasing](https://www.ti.com/document-viewer/lit/html/SSZT656/GUID-27C7EED1-D790-406F-8502-C05C35801E1E): open/short/idle bus uncertainty.
- Texas Instruments, [RS-485 Design Guide](https://www.ti.com/lit/an/slla272/slla272.pdf): driver contention, differential signaling and physical constraints.

The **source-derived** claims above concern RS-485 electrical requirements. The event scheduling, collision policy and digital sampling assumptions are VEN engineering design choices, not provisions of the RS-485 standard.
