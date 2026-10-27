# Virtual Ethernet Network (VEN)

Early-stage Rust network simulation workspace for the PLC Web ecosystem.

VEN is designed to run independently or as a **simulation participant** inside the PLC Web shared virtual-time runtime. It does not emulate a particular PLC or replace the PLC's target-specific execution profile.

## Workspace

| Crate | Responsibility |
|---|---|
| `sim-time` | Shared nanosecond virtual instants and durations |
| `sim-kernel` | Shared time grants, exclusive resources and a conservative multi-actor causal coordinator |
| `ven-core` | Opaque network frames and port identifiers |
| `ven-scheduler` | Specialized, locally ordered and cancellable network event queue |
| `ven-medium` | Timed full-duplex links and RS-485 shared bus with UART duration and configurable DE/RE turnaround |
| `ven-rs485-phy` | Opt-in RS-485 digital PHY; manufacturer timing metadata and oscilloscope sample-comparison tools |
| `ven-topology` | Typed node/port/link/RS-485 bus connectivity |
| `ven-modbus` | Modbus RTU codec, timing, receiver and limited 0x03/0x06 session state machines |
| `ven-modbus-sim` | Per-bus LOD router and session-scoped ModbusBusHost enforcing exclusive BusId ownership |
| `ven-sim` | Network participant with UART arrivals, immediate contention events, per-port line release and receive readiness |
| `ven-plc-bridge` | Persistent JSONL subprocess client for real Python ScheduledRuntime, causal PLC scan actor and deterministic valve dynamics pilot |

The two `sim-*` crates are **VEN-independent** and intentionally kept portable so they can later serve PLC Web, Gefest-related simulation adapters, and other simulation participants. The reusable `ConservativeCoordinator` now synchronizes multiple in-process causal participants; a scalable distributed or rollback-capable coordinator remains future work.

## Run

Requires Rust 1.76+.

```sh
cargo test --workspace --all-targets
```

The vertical slice schedules a frame over a 1 Mbit/s full-duplex link. `ven-sim` returns control to the host coordinator on the exact modeled arrival timestamp, and both link directions can transmit independently.

## Scope and limitations

Currently this is **a working foundation, not a complete Ethernet/industrial protocol simulator**. Frame bytes are opaque, and line serialization uses only bytes supplied by the caller (physical overhead/IFG/FCS are not added). Ethernet switching, electrical-layer collision resolution, PROFINET, EtherCAT, complete Modbus function set and conformance validation, advanced local clocks, distributed coordination and rollback are **not implemented**. Shared RS-485 bus access, conservative immediate contention diagnostics, configurable per-port direction switching and UART serialization timing **are implemented**. VEN schedules UART character-completion events; the Modbus adapter consumes those events, maintains timeouts and supports addressed reads (0x03), single-register writes (0x06), and broadcast writes. This is a bounded vertical slice, not a certified Modbus endpoint.

A separate, opt-in `ven-rs485-phy` implements a slower, **bit-sampled digital** RS-485 model. It is its own `SimulationParticipant` and is now connected to `ven-modbus-sim` through a separate `DigitalModbusRtuLine` adapter. The `ModbusBus` enum selects **exactly one** transport implementation per instantiated bus. This prevents duplicate delivery within one `ModbusBus`. The new `ModbusBusHost` uses `sim-kernel::resources::ResourceRegistry<BusId>` to reject duplicate resource claims among all participants registered with that host. Global registration is not implicitly enforced on arbitrarily instantiated raw participants. Bit-center sampling detects start/data/parity/stop behavior, indeterminate shared-line contention and injected receiver-bit faults; it is **not** an analog/electrical circuit model.

The reusable `ConservativeCoordinator` now runs multiple owned participants at event barriers and routes zero-delay effects in deterministic microsteps. An end-to-end test connects **mock PLC logic**, Modbus over VEN and **mock equipment** in both character-timed and digital-bit LODs. An opt-in `ven-plc-bridge` now runs an actual Python PLC Web `ScheduledRuntime` **as a persistent subprocess** through a versioned JSONL contract (the Python side is proposed in [PLC Web PR #654](https://github.com/boltholds/PLC-web/pull/654)). A simple independent valve dynamics actor has timed open/close limit feedback and injected faults. Rust CI tests its subprocess protocol against a Python **fixture**, not the complete PLC Web backend. The separately runnable real-worker/valve example and its environment requirements are in [PLC_PYTHON_RUNTIME_BRIDGE.md](docs/PLC_PYTHON_RUNTIME_BRIDGE.md). This is an engineering pilot: **real Python↔Rust cross-repository execution is not yet independently certified**, and Gefest CAD is still only a revisioned geometry reference, not a dynamics runtime. Next: end-to-end CI with both repositories, harden blocking IPC/timeouts, connect PLC I/O via Modbus, and introduce measurable equipment acceptance tests. See [RS-485 digital PHY](docs/RS485_DIGITAL_PHY.md), [transceiver profiles](docs/TRANSCEIVER_PROFILES.md), [real capture protocol](docs/CAPTURE_PROTOCOL.md), [bus ownership](docs/BUS_OWNERSHIP.md), and [conservative coordinator](docs/CONSERVATIVE_COORDINATOR.md) for fidelity and integration limits.

Architecture, invariants and non-goals are described in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). See [docs/NETWORK_RELEVANCE.md](docs/NETWORK_RELEVANCE.md) for primary-source correctness and model fidelity audit, and [docs/MODBUS_RTU.md](docs/MODBUS_RTU.md) for the implemented protocol scope.

## Licenses and provenance

Licensing policy has not yet been selected for this new repository; the crates are marked `publish = false`. No code from the GPL-licensed ns-3 repository has been copied or translated. ns-3 remains a conceptual reference and potential external conformance/benchmark tool.
