# Network-infrastructure relevance audit

Reviewed against the MODBUS over Serial Line Specification and Implementation Guide V1.02 (Dec 20, 2006), notably sections 2.1, 2.5.1.1, 3.1–3.4; Analog Devices AN-960 and Texas Instruments RS-485 bus-contention/failsafe notes.

Primary sources:
- https://www.modbus.org/file/secure/modbusoverserial.pdf
- https://www.analog.com/en/resources/app-notes/an-960.html
- https://www.ti.com/document-viewer/lit/html/SSZTCC0/GUID-B221A0C5-7A40-439C-A635-B4CE4A25B10A
- https://www.ti.com/document-viewer/lit/html/SSZT656/GUID-27C7EED1-D790-406F-8502-C05C35801E1E

## Correct separation of responsibilities

| Concern | Standard/layer | VEN current implementation |
|---|---|---|
| Balanced differential bus, multiple tri-state drivers and receivers | RS-485 physical | Shared BusId and driver-overlap diagnostics; optional digital PHY supports logic High/Low/Indeterminate, **not** electrical voltage or impedance |
| Start/data/parity/stop bits, baud rate, turnaround | UART / serial configuration | DE/RE timing, character-mode events, and independent digital PHY with per-bit mid-cell sampling and UART parity/framing checks |
| Addressing, master/client initiated transactions, CRC16, RTU t1.5/t3.5 | Modbus serial-line protocol | Separate Modbus codec, 0x03/0x06 Master/Slave and character-driven timeouts implemented; opt-in digital PHY has **not yet been integrated into Modbus adapter** |
| Trunk, short stubs, line terminations at two ends, common conductor, failsafe bias, unit loads | Physical installation | **Not implemented**; BusId is abstract connectivity |
| Transmit collision | Multiple RS-485 drivers active simultaneously | Emit an immediate model-level collision when the second modeled driver is enabled; suppress remaining contended data, and issue later completion summaries |
| Auto collision detection/retry | Not guaranteed by RS-485 | No hardware collision detector or automatic retry; collision output is only a simulation diagnostic |
| Ethernet switch output queues / full duplex | Ethernet | Separate wired model; absolutely no reuse of RS-485 contention semantics |

## Earlier simplifications corrected

1. **An RS-485 multidrop line is not an Ethernet hub or star**. The typical MODBUS 2-wire wiring is a trunk with direct or short branch connections, plus a signal common. The model's BusId expresses *electrical sharing*, NOT actual cable geometry. Real topology/impedance checks require trunk/branch segment geometry and per-device loading.
2. **"Collision => corrupted frame/CRC failure" is not a universal electrical rule**. Multiple enabled drivers cause bus contention, which may yield unpredictable sampled data, receiver behavior and possible damage depending on transceivers. The current LOD-2 rule explicitly drops all overlapping frames and generates a simulation diagnostic; a UART receiver is not assumed to detect an actual collision.
3. **RS-485 has no built-in master/slave polling or t3.5 silence**. Those are specified by MODBUS Serial Line. A compliant single-master transaction pattern aims to avoid overlapping drivers; contention testing is fault-injection/misconfiguration modeling.
4. **Serial byte cost is not just eight data bits**. UART character formatting is accounted for: typical Modbus RTU 8E1 or 8N2 uses 11 line bits per 8-bit data byte.
5. **Do not assume 32 devices is a universal hard limit**. RS-485 node counts depend on transceiver unit load, bias, repeaters and network design. The Modbus guide discusses 32 as a general baseline, not an immutable ceiling.
6. **A fault involving missing termination or broken common is not necessarily a packet drop**. Impedance mismatch, reflections, bias, common-mode range and sampling must be modeled explicitly before claiming a physical outcome.
7. **Do not equate sampled UART bits with electrical waveform accuracy**. The digital PHY resolves multiple drivers to Indeterminate and models one uniform propagation delay per bus. It does not calculate voltage, line reflections, receiver threshold hysteresis, analog rise/fall times or cable-specific faults.
8. **No false protocol conformance**: PROFINET and EtherCAT are still unimplemented. Modbus RTU 0x03/0x06 Master/Slave is an engineering subset; there is no independently confirmed full protocol conformance, nor a production integration between Modbus and the new digital PHY.

## Modeling semantics and test oracle

- At the current byte-frame LOD, a local call to `send_rs485` asserts a simulated driver for a fixed interval `[start, finish)`.
- A second driver starting while the first is still enabled contends, including during the configured post-data `driver_release` hold. Without release delay, exactly adjacent starts at the previous UART finish do not collide. VEN emits the collision at overlap onset.
- At overlap onset, VEN publishes `NetworkOutput::Rs485Collision`, and the Modbus adapter invalidates any partial RTU frame. Each affected transmission later produces a `Rs485Contention` completion summary. Previously delivered characters cannot be retracted; later characters from contended transmissions are suppressed.
- Without contention, whole-frame mode broadcasts the opaque payload. UART mode streams characters with uniform propagation; sender's RE readiness can suppress reception after its own transmission. These modeled signals remain simpler than electrical waveforms.
- Test results from this model do **not** certify real physical reliability, electrical safety, protocol conformity, or industrial commissioning readiness.

## Planned next slice

Wire the opt-in `ven-rs485-phy` into a shared bus-LOD selection API and the already-existing Modbus RTU adapter while preserving one global virtual-time authority and preventing duplicate frame delivery. Validate timing, receiver error classification and per-model assumptions against transceiver datasheets or hardware traces. A deterministic `SampleFault::Flip` or `ForceIndeterminate` is an explicit scenario fault, not a computed probability of bit corruption.

## DE/RE timing basis

Per-port turnaround is a configurable engineering parameter, not a universal RS-485 or Modbus timing. Driver enable/disable and receiver enable/disable are distinct transceiver timing characteristics. References: [Analog Devices AN-960](https://www.analog.com/en/resources/app-notes/an-960.html) and [TI RS-485 Half-Duplex EVM Guide](https://www.ti.com/lit/ug/sllu173c/sllu173c.pdf). Current VEN models one logical enable-settle delay, one post-data driver-release hold and one receiver-reenable delay; it does **not** reproduce voltage, bias, pin propagation edges or guaranteed electrical behavior of any named part.

## New manufacturer timing metadata and oscilloscope comparison

The digital PHY now includes separate manufacturer spec metadata (ADM3485E Rev D and THVD1450 SLLSEY3E Rev E) and a synthetic-tested canonical CSV waveform comparison API. The selected datasheet times represent different pin transition phenomena and test configurations, **not directly equivalent physical line propagation or UART receiver delays**. The guard planner provides *effective delays for a simulation scenario*, not automatically calibrated physical profiles. No real lab oscilloscope captures have been imported; hardware validation remains pending. See [TRANSCEIVER_PROFILES.md](TRANSCEIVER_PROFILES.md).
