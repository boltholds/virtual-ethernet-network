# Transceiver datasheet profiles and oscilloscope comparison

Status: **manufacturer specification reference + reusable measurement comparator. Not hardware validated.**

The two named profiles are selected to exercise materially different timing regimes. Their data are captured from primary manufacturer data sheets, **not** community estimates, third-party PCB measurements or made-up waveform data.

## Grounded source tables

### Analog Devices ADM3485E — Rev D, Table 2, page 4

Source: https://www.analog.com/media/en/technical-documentation/data-sheets/ADM3485E.pdf

Timing conditions: VCC = 3.3 V, TA = 25°C. Enable/disable: RL = 110 Ω; receiver-enable/propagation: CL = 15 pF. **Do not interpret typical values as guaranteed maximums.**

| Parameter | Typical | Maximum | Datasheet symbol |
|---|---:|---:|---|
| Driver enable (active) | 42 ns | 90 ns | tPZL / tPZH |
| Driver enable from shutdown | 650 ns | 900 ns | tPSL / tPSH |
| Driver disable | 35 ns | 80 ns | tPLZ / tPHZ |
| Receiver enable (active) | 25 ns | 50 ns | tRPZL / tRPZH |
| Receiver enable from shutdown | 720 ns | 1400 ns | tRPSL / tRPSH |
| Driver propagation | 23 ns | 35 ns | tPLH / tPHL |
| Receiver propagation | 62 ns | 90 ns | tRPLH / tRPHL |

Data rate advertised: 12 Mbps. Shutdown is a separate state: asserting DE low and RE high for a sufficient interval changes the subsequent enable response. Details and thresholds are in the datasheet. **The optional guard planner does not simulate shutdown entry.**

### Texas Instruments THVD1450 — SLLSEY3E Rev E, §7.8, page 11

Source: https://www.ti.com/lit/ds/symlink/thvd1451.pdf

The table's *typical* switching characteristics are at VCC = 5 V, 25°C. Driver and receiver test loads differ: driver RL = 54 Ω / CL = 50 pF; receiver CL = 15 pF.

| Parameter | Typical | Maximum | Critical test condition |
|---|---:|---:|---|
| Driver enable (active) | 20 ns | 50 ns | RE = 0 V |
| Driver enable with receiver disabled | 2500 ns | 10000 ns | RE = VCC, slow path |
| Driver disable | 15 ns | 25 ns | tPHZ / tPLZ |
| Receiver enable (active) | 50 ns | 110 ns | DE = VCC |
| Receiver enable with DE low | 4000 ns | 14000 ns | DE = 0 V, slow path |
| Driver propagation | 10 ns | 20 ns | RL = 54 Ω, CL = 50 pF |
| Receiver propagation | 25 ns | 40 ns | CL = 15 pF |

Data rate advertised: 50 Mbps.

**Critical restriction:** It is NOT correct to use the fast receiver-enable figure when the controller's DE/RE sequencing corresponds to the DE-low path. Likewise, neither device's driver-disable time is the same as the controller software's DE-hold time.

## How the current profile adapter works

`Part::datasheet()` stores **metadata**, keeping typical and max apart and documenting test conditions. `plan_digital_guard(baud, case, driver_start, receiver_start, controller)` constructs a `TransceiverProfile` for VEN's existing *effective* three-part turnaround contract.

- `driver_enable`: selected driver-enable switching bound using explicit `DriverStart::ReceiverEnabled` or `DriverStart::ReceiverDisabled`. These choices correspond to materially different datasheet test conditions and must match the real controller pin sequencing.
- `driver_release`: controller's software DE hold **plus** selected driver-disable time.
- `receiver_enable`: controller's specified re-enable hold **plus** selected receiver-enable timing for the selected active/shutdown path.
- Driver and receiver propagation values are **metadata only**, not added into the PHY clock; cable delay, GPIO propagation, waveform rise times and fixture loading must be calibrated separately.
- `Case::DatasheetMax` is a bound *under the table's specified electrical conditions*, not proof of worst-case behavior of an arbitrary installed board at arbitrary temperature, load and common-mode voltage.
- The adapter does **not** drive an actual DE or RE GPIO waveform and does not model the shutdown-mode state machine. `DriverStart` and `ReceiverStart` declare the intended paths; they are not inferred from the input device state.

For engineering use, select the operating path deliberately, then validate effective delays with the actual board and wiring.

## Canonical oscilloscope input

Currently accepted CSV format (**our interchange schema**, not a universal native Tektronix/Rigol/Siglent export):

```csv
time_ns,channel,millivolts
0,DE,0
100,DE,3300
100,RO,3300
210,RO,0
400,RO,3300
```

Supported names: `DE`, `RE`, `DI`, `RO`, `A`, `B`, `DIFF`. Convert manufacturer-specific CSVs to this exact three-column format without discarding original raw files. The `DIFF` channel should be **measured differentially or explicitly derived from A-B samples** with documented polarity, probe type and common ground. The physical `A/B` bus voltage and digital `RO` output are different observation points; transceiver propagation delay must not be ignored.

For each real acquisition, preserve metadata: device part/revision; VCC; temperature; baud and UART format; DE/RE sequence; load and terminations; cable/stub lengths; oscilloscope/probe model; sample rate; timebase; channel thresholds; which direction is being measured. Record whether a receiver is starting from shutdown. Without this, a timing difference cannot be meaningfully attributed.

### Two explicit comparisons

1. **Digital sample comparison (implemented).** Use an actual **DE rising edge** in the measured trace as the shared time origin. `trace::expected_from_phy(...)` requires explicit `ObservationPoint::ReceiverInput` for logic sampled on the bus or `ObservationPoint::ReceiverOutput { propagation }` for the transceiver RO output. Select a propagation assumption from the datasheet or a measured calibration; it is **not** an exact electrical RO model. Compare those bit-center levels with the `RO` or `DIFF` channel, as appropriate. Require an explicit threshold range and maximum sample age. Missing capture samples, indeterminate voltage bands, prediction uncertainty and mismatches are not counted as passes. Never auto-shift the RO trace to maximize agreement.
2. **Analog waveform fit (NOT implemented).** Would require driver/receiver analog transfer characteristics, cable geometry and impedance, rise/fall slew, receiver hysteresis and temperature dependence. Merely sampling `A` and `B` voltage in a CSV does not magically supply an electrical solver.

### Library example

```rust
use sim_time::{SimDuration, SimInstant};
use ven_rs485_phy::trace::{
    BitExpectation, Channel, Level, ScopeTrace, Thresholds,
};

let csv = std::fs::read_to_string("hardware-capture-normalized.csv")?;
let capture = ScopeTrace::from_canonical_csv(&csv)?;
let logic_thresholds = Thresholds { low_mv: 800, high_mv: 2000 };
let trigger = capture.trigger_rising(Channel::De, logic_thresholds)?;
let expected = vec![
    BitExpectation { after_trigger: SimDuration::from_nanos(52100), level: Level::Low },
    BitExpectation { after_trigger: SimDuration::from_nanos(156300), level: Level::High },
];
let (summary, findings) = capture.compare_bits(
    trigger, Channel::Ro, logic_thresholds,
    SimDuration::from_nanos(1000), &expected,
)?;
assert!(!summary.all_matched(expected.len()) || findings.len() == expected.len());
# Ok::<(), Box<dyn std::error::Error>>(())
```

The example's expected timestamps are **illustrative**. Production comparisons should derive predictions from a matching PHY run with `set_trace_bits(true)` using `trace::expected_from_phy(...)` and a deliberate `ObservationPoint`. These predictions are *not measured values*. A RO delay chosen from the datasheet is only an approximation and differs by operating condition and edge direction. The code above is documentation of the API, not an automatically executable calibration/verification report.

## Evidence levels

- **Datasheet transcriptions:** grounded to the two linked manufacturer PDFs, conditions stated.
- **Synthetic comparator tests:** validate parsing/thresholding/trigger alignment, not simulation fidelity.
- **Real waveform comparisons:** **none performed**; no oscilloscope captures were supplied.
- **Hardware-calibrated physics:** **not implemented**.

The next upgrade should ingest the first actual RO/DE/DI and differential A/B captures, derive model predictions on a shared clock, and report timing error distributions under a reproducible measurement campaign.
