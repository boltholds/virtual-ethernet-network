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
| Balanced differential bus, multiple tri-state drivers and receivers | RS-485 physical | One shared BusId, driver-overlap detection; electrical characteristics not modeled |
| Start/data/parity/stop bits, baud rate, turnaround | UART / serial configuration | Configurable bit count and baud; no DE/RE turnaround or real per-bit waveform |
| Addressing, master/client initiated transactions, CRC16, RTU t1.5/t3.5 | Modbus serial-line protocol | Codec, CRC and standalone character-clock receiver **implemented**; transaction state machine **not implemented** |
| Trunk, short stubs, line terminations at two ends, common conductor, failsafe bias, unit loads | Physical installation | **Not implemented**; BusId is abstract connectivity |
| Transmit collision | Multiple RS-485 drivers active simultaneously | Conservatively mark all overlapping transmissions **not delivered**, emit simulation-only diagnostic |
| Auto collision detection/retry | Not guaranteed by RS-485 | **Not implemented** |
| Ethernet switch output queues / full duplex | Ethernet | Separate wired model; absolutely no reuse of RS-485 contention semantics |

## Earlier simplifications corrected

1. **An RS-485 multidrop line is not an Ethernet hub or star**. The typical MODBUS 2-wire wiring is a trunk with direct or short branch connections, plus a signal common. The model's BusId expresses *electrical sharing*, NOT actual cable geometry. Real topology/impedance checks require trunk/branch segment geometry and per-device loading.
2. **"Collision => corrupted frame/CRC failure" is not a universal electrical rule**. Multiple enabled drivers cause bus contention, which may yield unpredictable sampled data, receiver behavior and possible damage depending on transceivers. The current LOD-2 rule explicitly drops all overlapping frames and generates a simulation diagnostic; a UART receiver is not assumed to detect an actual collision.
3. **RS-485 has no built-in master/slave polling or t3.5 silence**. Those are specified by MODBUS Serial Line. A compliant single-master transaction pattern aims to avoid overlapping drivers; contention testing is fault-injection/misconfiguration modeling.
4. **Serial byte cost is not just eight data bits**. UART character formatting is accounted for: typical Modbus RTU 8E1 or 8N2 uses 11 line bits per 8-bit data byte.
5. **Do not assume 32 devices is a universal hard limit**. RS-485 node counts depend on transceiver unit load, bias, repeaters and network design. The Modbus guide discusses 32 as a general baseline, not an immutable ceiling.
6. **A fault involving missing termination or broken common is not necessarily a packet drop**. Impedance mismatch, reflections, bias, common-mode range and sampling must be modeled explicitly before claiming a physical outcome.
7. **Do not equate frame-complete scheduling with bit-accurate physical simulation**. Propagation is currently one configurable constant per bus; no per-port delay, simultaneous waveform superposition, interference or cut-through behavior.
8. **No false protocol conformance**: PROFINET and EtherCAT are still unimplemented. Modbus RTU CRC/timer/parser primitives exist, but neither a full master/slave state machine nor integrated per-character RS-485 receive simulation is implemented.

## Modeling semantics and test oracle

- At the current byte-frame LOD, a local call to `send_rs485` asserts a simulated driver for a fixed interval `[start, finish)`.
- A second driver starting before the first finishes contends, even if starts are staggered. An exactly adjacent start (`start == preceding finish`) is not a collision.
- Each contended transmission produces one `NetworkOutput::Rs485Contention` diagnostic at its modeled completion+propagation instant, and zero valid receiver frames.
- Without contention, each receiving participant (excluding sender) receives the same opaque bytes after full serialization and uniform propagation.
- Test results from this model do **not** certify real physical reliability, electrical safety, protocol conformity, or industrial commissioning readiness.

## Planned next slice

Modbus RTU is now a **separate protocol crate** above UART/RS-485, with CRC, 256-byte ADU bound, legal address range, t1.5/t3.5 calculations and a character-timestamped receiver. Still required: character-emission integration, master/slave state machine, addressing/filtering, turnaround, device timeouts, and deliberate fault injection (e.g. stuck-enabled transceivers).
