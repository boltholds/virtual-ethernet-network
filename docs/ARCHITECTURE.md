# VEN architecture (initial slice)

## Ownership

The VEN workspace contains two **reusable, VEN-independent** crates, `sim-time` and `sim-kernel`. They define one virtual time domain and participant contracts. They are deliberately separate packages so PLC Web, Gefest integrations, and other participants can use the same primitives later. Their current home in this workspace is a bootstrap location, **not** a claim that integration into other repositories already exists.

- `sim-time`: integer nanosecond instants/durations; no wall-clock dependency.
- `sim-kernel`: time grants, early-return results, coordinator validation, externally observable outputs.
- `ven-core`: network port identifiers and opaque encoded frames.
- `ven-scheduler`: local, cancellation-capable, stable ordered network-event queue.
- `ven-medium`: elementary full-duplex point-to-point serialization; RS-485 two-wire medium with UART character timing and explicitly simplified contention.
- `ven-topology`: physical link adjacency and multiple RS-485 ports attached to one shared BusId.
- `ven-sim`: a standalone/embeddable VEN participant binding network events to shared time contracts; emits frame arrivals and model-level contention diagnostics.

VEN **does not own global simulation time**. Its event queue is local, and the host coordinator grants a horizon. An external frame arrival makes VEN return early so other participants can observe it at the correct time.

## Semantics implemented now

- A frame is an opaque already-encoded byte sequence. No Ethernet headers, FCS, preamble, IFG, PHY signaling, or protocol state is implied.
- A full-duplex link has independent directional transmit availability. Back-to-back frames queue behind each other within each direction.
- A frame becomes visible only **after its entire serialization time plus link propagation delay**. This is a deliberately simple store-and-forward receive model, **not** cut-through switching.
- Scheduling two events for the same instant preserves insertion order for repeatable execution. This is an internal tie-break rule, **not a claim about the real world's physical ordering**.
- The local participant returns at the earliest externally observable arrival at or before the host's time grant, emitting all arrivals at that instant.
- Invalid links, unknown endpoints, zero rates, empty frames, and timestamp arithmetic overflow are errors.

- One RS-485 transmitter starts **immediately** when requested; the medium does not auto-queue competing drivers. Every overlapping transmission on the same bus is conservatively considered undeliverable, and the simulator produces a diagnostic at transmission completion. This is an explicit frame-level policy, not a voltage-level calculation or a guarantee of collision detection by the device.
- Byte duration on RS-485 uses configured UART start, data, parity and stop bits, not Ethernet bit counting.
- Every successful RS-485 transmission becomes available to all *other* attached ports at frame completion plus one bus propagation delay. This is a simplified uniform-delay broadcast; no receiver-side echo, bit sampling, individual drop-cable propagation or per-device UART parsing exists.

## Explicitly not implemented

- Vendor-specific PLC execution, PLC scan cycles, controller tasks.
- Ethernet MAC, switch forwarding, physical-voltage RS-485 collision resolution, Modbus RTU/TCP, PROFINET RT/IRT, EtherCAT.
- Packet corruption/garbling, bit errors, jitter, queue limits, traffic shaping, termination/bias/impedance, cable reflections, electrical overload or transceiver damage.
- Optimistic global rollback, cross-participant causal ordering, multi-participant time grant calculation.
- Independent protocol conformance or fidelity certification.

Do **not** label this library a compliant PROFINET, EtherCAT, Modbus, or physical Ethernet simulator.

## Planned boundaries (not crates yet)

Future protocol implementations should sit above the link/medium model, with shared typed transmission interfaces. A dedicated RS-485 medium should own half-duplex bus contention rather than forcing Ethernet collisions onto all links. Transport and protocol timeouts belong to protocol models and the PLC execution profiles, not a global arbitrary priority queue.

The coordinator's policy for causally related same-time events and inter-participant lookahead must be specified before multiple simultaneous PLC and network executors are hosted together. Conservative execution comes first; snapshot/restore and speculative rollback are optional capabilities.

No source code is copied or translated from GPL-licensed ns-3. ns-3 is an architectural and differential-testing reference only.

## Validation path

1. Prove bit serialization, propagation, independent full-duplex lanes, cancellation, and stable event order in unit tests.
2. Add an explicit topology with interface-to-link compatibility checks.
3. Add RS-485 two-wire shared medium and conservative overlap detection (**implemented**); separately implement Modbus RTU frame timing, t1.5/t3.5, CRC, and master/slave exchange state (**not yet implemented**).
4. Add Ethernet switching with bounded queues and full-duplex ports; then specialized PROFINET and EtherCAT protocol execution.
5. Validate timing and observable behavior against documented protocol requirements and independent reference systems.
