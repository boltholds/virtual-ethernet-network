# VEN architecture (initial slice)

## Ownership

The VEN workspace contains two **reusable, VEN-independent** crates, `sim-time` and `sim-kernel`. They define one virtual time domain and participant contracts. They are deliberately separate packages so PLC Web, Gefest integrations, and other participants can use the same primitives later. Their current home in this workspace is a bootstrap location, **not** a claim that integration into other repositories already exists.

- `sim-time`: integer nanosecond instants/durations; no wall-clock dependency.
- `sim-kernel`: time grants, early-return results, coordinator validation, externally observable outputs.
- `ven-core`: network port identifiers and opaque encoded frames.
- `ven-scheduler`: local, cancellation-capable, stable ordered network-event queue.
- `ven-medium`: elementary full-duplex point-to-point serialization; RS-485 two-wire medium with UART character timing and configurable driver-enable, driver-release, and receiver-enable durations.
- `ven-topology`: physical link adjacency and multiple RS-485 ports attached to one shared BusId.
- `ven-rs485-phy`: opt-in bit-sampled logical RS-485 bus with modeled TX/RX turnaround, conservative digital contention, parity/framing/uncertainty reports and deterministic receiver-side faults. Independently hosted as a `SimulationParticipant` (not yet wired into the Modbus adapter).
- `ven-modbus`: independent Modbus RTU codec, CRC, UART character/timing parser, limited master/slave state machines for 0x03/0x06.
- `ven-modbus-sim`: protocol adapter above VEN UART characters, t3.5 finalization and master response deadlines.
- `ven-sim`: a standalone/embeddable VEN participant binding network events to shared time contracts; emits frame arrivals, per-character UART events, immediate collision notifications and observable driver-release events.
- `ven-modbus-sim::DigitalModbusRtuLine`: translates `PhysicalEvent::Character` bit-sampled bytes and parity/framing errors into the same RTU protocol states, drives slave replies, preserves shared virtual-time deadlines and prevents duplicate receive sources within one bus instance.

VEN **does not own global simulation time**. Its event queue is local, and the host coordinator grants a horizon. An external frame arrival makes VEN return early so other participants can observe it at the correct time.

## Semantics implemented now

- A frame is an opaque already-encoded byte sequence. No Ethernet headers, FCS, preamble, IFG, PHY signaling, or protocol state is implied.
- A full-duplex link has independent directional transmit availability. Back-to-back frames queue behind each other within each direction.
- A frame becomes visible only **after its entire serialization time plus link propagation delay**. This is a deliberately simple store-and-forward receive model, **not** cut-through switching.
- Scheduling two events for the same instant preserves insertion order for repeatable execution. This is an internal tie-break rule, **not a claim about the real world's physical ordering**.
- The local participant returns at the earliest externally observable arrival at or before the host's time grant, emitting all arrivals at that instant.
- Invalid links, unknown endpoints, zero rates, empty frames, and timestamp arithmetic overflow are errors.

- At `send_rs485_uart` or `send_rs485`, DE is modeled as asserted **immediately** at the current virtual instant. Optional `driver_enable` holds off the UART data start; `driver_release` retains line ownership after the last stop bit; `receiver_enable` holds the sender's local RX disabled after release. These are **per-device parameters**, not Modbus constants. Currently a conservative model considers the driver active throughout the enable-settle period.
- Any overlap of two modeled driver-held intervals on one bus schedules a `Rs485Collision` at the exact current instant the second driver is asserted, not at eventual transmission completion. Once marked contended, no future valid UART characters from either overlapping transmission are delivered. Separate completion diagnostics remain available. Any characters delivered **before** the collision cannot be retracted, so the protocol adapter invalidates its incomplete RTU frame immediately.
- Same-time ordering is deterministic in the event queue, but VEN does not claim to reconstruct physical simultaneity. Future-scheduled driver activations or waveform-level electrical settling are **not** modeled.
- Byte duration on RS-485 uses configured UART start, data, parity and stop bits, not Ethernet bit counting. `driver_busy_until` and `receiver_ready_at` expose the configured direction-switching boundaries; a port cannot transmit another frame while its own driver is still held.
- Legacy RS-485 frame transmissions become visible after frame completion; the opt-in UART mode delivers one character completion per received byte with deterministic gap/corrupt/drop injection. Both use simplified uniform propagation delay and no analog waveform or per-port drop-cable geometry.

## Explicitly not implemented

- Vendor-specific PLC execution, PLC scan cycles, controller tasks.
- Ethernet MAC, switch forwarding, physical-voltage RS-485 collision resolution, complete Modbus function support, Modbus TCP, PROFINET RT/IRT, EtherCAT.
- **Analog** electrical contention resolution, realistic stochastic bit-error rates, jitter, queue limits, traffic shaping, termination/bias/impedance, cable reflections, DE/RE pin-waveform propagation or transceiver damage. The opt-in digital PHY supports *deterministic injected receiver-bit faults*, not computed electrical errors. Modbus RTU UART parser is connected to an opt-in character-timestamped network mode; legacy opaque-frame mode remains.
- Optimistic global rollback, cross-participant causal ordering, multi-participant time grant calculation.
- Independent protocol conformance or fidelity certification.

Do **not** label this library a compliant PROFINET, EtherCAT, Modbus, or physical Ethernet simulator.

## Simulation levels and physical-model boundary

The high-throughput network participant (`ven-sim`) retains character/frame-level RS-485 and its existing Modbus protocol adapter; `ven-rs485-phy` offers a distinct opt-in bit-level **digital** participant on the same `sim-time`/`sim-kernel` contracts. They are now connected through `ven-modbus-sim::ModbusBus`, an explicit enum with either `CharacterTimed(ModbusRtuLine)` or `DigitalBits(DigitalModbusRtuLine)`. One `ModbusBus` owns exactly one transport scheduler, and the existing Modbus RTU receiver/master/slave semantics drive both. **Global** uniqueness of BusId across separate participants is not yet enforced: a future multi-participant coordinator must own an authoritative bus registry.

The PHY samples the logical shared level at each UART bit center. Multiple active drivers produce `Indeterminate` for digital contention rather than a fabricated deterministic voltage or CRC failure. Local failsafe assumptions only choose what the idle line means logically, not any resistor/cable model. UART decode distinguishes valid bytes, parity errors, framing errors, missing/indeterminate bits and disabled receivers. Bit faults are deterministic *receiver sample* perturbations, not hardware damage.

See [RS485_DIGITAL_PHY.md](RS485_DIGITAL_PHY.md) for details, invariants, bounded per-frame scheduling and fidelity gaps.

## Planned boundaries (not crates yet)

Future protocol implementations should sit above the link/medium model, with shared typed transmission interfaces. A dedicated RS-485 medium should own half-duplex bus contention rather than forcing Ethernet collisions onto all links. Transport and protocol timeouts belong to protocol models and the PLC execution profiles, not a global arbitrary priority queue.

The coordinator's policy for causally related same-time events and inter-participant lookahead must be specified before multiple simultaneous PLC and network executors are hosted together. Conservative execution comes first; snapshot/restore and speculative rollback are optional capabilities.

No source code is copied or translated from GPL-licensed ns-3. ns-3 is an architectural and differential-testing reference only.

## Validation path

1. Prove bit serialization, propagation, independent full-duplex lanes, cancellation, and stable event order in unit tests.
2. Add an explicit topology with interface-to-link compatibility checks.
3. RS-485 two-wire shared medium and overlap detection (**implemented**); Modbus RTU codec, CRC and standalone t1.5/t3.5 character parser (**implemented**); device-bound UART stream and limited 0x03/0x06 master/slave exchange state (**implemented; unverified against hardware**).
4. Add Ethernet switching with bounded queues and full-duplex ports; then specialized PROFINET and EtherCAT protocol execution.
5. Validate timing and observable behavior against documented protocol requirements and independent reference systems.
