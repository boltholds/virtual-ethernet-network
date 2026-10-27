# Real oscilloscope capture protocol for RS-485 VEN validation

**Current evidence status:** no real captures have been provided or tested. This procedure prepares the input. The comparator uses digital bit-center levels; it does not fit impedance, reflection, threshold hysteresis or line waveform amplitude.

## Required measurements and safety

Capture the **same transmission** using a common acquisition timebase, ideally with DE and DI on the transmitting board, RO at the receiving board, and the A−B differential voltage on the RS-485 bus. Record an actual trigger on **DE asserting**, preserving pre-trigger samples; do not align traces afterwards to a receiver edge.

Use a properly rated differential/isolated probe for A−B or two appropriate inputs with safe subtraction and common reference, within the instrument/probe's common-mode range. **Do not attach the ground clip of an earth-referenced oscilloscope directly to either A or B** unless grounding is verified safe; this can short the bus. If working on mains-connected equipment, follow lab electrical safety practices and use qualified personnel/instrumentation.

Capture timing inputs (DE, /RE, DI), the digital RO output, and the differential bus. Validate A/B polarity against the transceiver datasheet; do not assume that an oscilloscope's channel naming proves UART logical polarity. Only measure systems you own or are authorized to test.

Start with a controlled sequence of test frames such as 0x55, 0x00, 0xFF, then an encoded Modbus RTU request. Capture distinct cases: one transmitter, clean request/reply, deliberate slow turnaround, and a safe lab-only contention case **only if the hardware setup is specifically rated and protected for it**. Do not intentionally short commercial transceiver outputs or inject faults into an operating industrial installation.

## Retain acquisition metadata

For every trace, record:
- exact part and package revision (ADM3485E/THVD1450 or other), board revision, supply voltage, ambient temperature
- baud, UART data/parity/stop, transmitted hexadecimal bytes, frame number, direction
- DE and /RE logic convention, firmware DE hold, RE enable time, and any sleep/shutdown state
- termination and bias network details, common/ground arrangement, line length, stubs and receiver count
- oscilloscope and probe models, bandwidth, attenuation, sample interval, trigger and voltage calibration
- source file name, SHA-256 of original raw capture and who collected it

Keep the original scope files immutable, and store a sidecar manifest with these details. A canonical CSV is a **derived file** and must not replace raw data.

## Normalize to supported CSV

The reusable Rust parser accepts only the following schema:

\`\`\`csv
time_ns,channel,millivolts
0,DE,0
100,DE,3300
100,RO,3300
210,RO,0
400,RO,3300
\`\`\`

This block is **synthetic illustration**, not a measured device. Real datasets should record every relevant sampled timepoint (not just logic transitions) at an appropriate sampling rate for the baud and expected timing. Convert the scope's native time units and voltages with documented rounding; preserve times in integer nanoseconds relative to one acquisition time origin. Each channel's timestamps must be strictly increasing.

The \`DE\`, \`RO\`, \`DI\`, \`RE\`, \`A\`, \`B\` and \`DIFF\` channels are supported by the parser. The first CLI comparator intentionally compares only digital **RO vs DE** with one supplied pair of logic-level voltage thresholds; physical A−B, analog slew, and receiver threshold behavior remain unvalidated. A differential measurement alone is **not** interpreted as a calibrated TTL/CMOS RO signal.

## Run the comparison

The example requires a real acquisition file and known transmitted bytes. A sample command for a measured **9600-baud 8E1** request containing hexadecimal \`01 03 00 00 00 0A C5 CD\`:

\`\`\`sh
cargo run -p ven-rs485-phy --example compare_capture -- \
  captures/adm3485e-9600-rtu.csv 9600 01030000000AC5CD 800 2000 1000 62
\`\`\`

Positional fields: \`csv\`, \`baud\`, \`transmitted-hex-bytes\`, \`low-mV\`, \`high-mV\`, \`max-sample-age-ns\`, \`assumed-RO-propagation-ns\`. The illustrative 800/2000 mV thresholds and 62 ns RO propagation **are not default calibration for all devices**, temperatures or probes; set values matching your receiver logic supply and measured operating conditions.

The example uses **zero driver-enable and cable delay** and ideal 8E1 UART timing. Its result is a reproducible initial **digital sample comparison**, not full waveform calibration. For any other physical configuration, adapt the profile and propagation assumptions using \`ven-rs485-phy::profiles\` and \`Rs485DigitalPhy\` rather than shifting the measured trace.

The tool returns an error status if any bits mismatch, measurements are missing, or predictions/measurements are uncertain. It never silently labels these cases as passes.

## Acceptance gates

1. Deterministic replay: same raw capture/manifest and same VEN revision reproduce identical comparator output.
2. DE-trigger alignment is explicit; receive edges are never independently auto-aligned.
3. Every predicted bit is accounted for as matched, mismatched, missing, measurement-indeterminate or model-indeterminate.
4. A matched digital-bit capture is **not** a guarantee of analog line fidelity, physical safety, full Modbus conformance or device interchangeability.
5. Only after acquiring multiple independent captures under documented conditions should we estimate timing residual distributions and consider calibrating vendor-specific transceiver behavior.

This file does not constitute real laboratory measurements. No proprietary vendor waveforms or external scope files have been fabricated or included.
