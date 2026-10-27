# Virtual Ethernet Network (VEN)

Early-stage Rust network simulation workspace for the PLC Web ecosystem.

VEN is designed to run independently or as a **simulation participant** inside the PLC Web shared virtual-time runtime. It does not emulate a particular PLC or replace the PLC's target-specific execution profile.

## Workspace

| Crate | Responsibility |
|---|---|
| `sim-time` | Shared nanosecond virtual instants and durations |
| `sim-kernel` | Shared time grants, early-return events and validation contract |
| `ven-core` | Opaque network frames and port identifiers |
| `ven-scheduler` | Specialized, locally ordered and cancellable network event queue |
| `ven-medium` | Timed full-duplex links and RS-485 shared bus with UART duration and configurable DE/RE turnaround |
| `ven-topology` | Typed node/port/link/RS-485 bus connectivity |
| `ven-modbus` | Modbus RTU codec, timing, receiver and limited 0x03/0x06 session state machines |
| `ven-modbus-sim` | Protocol adapter integrating actual VEN UART events, Modbus endpoints and deadlines |
| `ven-sim` | Network participant with UART arrivals, immediate contention events, per-port line release and receive readiness |

The two `sim-*` crates are **VEN-independent** and intentionally kept portable so they can later serve PLC Web, Gefest-related simulation adapters, and other simulation participants. The current shared-kernel implementation validates grants for **one** participant; a real multi-participant co-simulation coordinator is future work.

## Run

Requires Rust 1.76+.

```sh
cargo test --workspace --all-targets
```

The vertical slice schedules a frame over a 1 Mbit/s full-duplex link. `ven-sim` returns control to the host coordinator on the exact modeled arrival timestamp, and both link directions can transmit independently.

## Scope and limitations

Currently this is **a working foundation, not a complete Ethernet/industrial protocol simulator**. Frame bytes are opaque, and line serialization uses only bytes supplied by the caller (physical overhead/IFG/FCS are not added). Ethernet switching, electrical-layer collision resolution, PROFINET, EtherCAT, complete Modbus function set and conformance validation, advanced local clocks, global multi-participant coordination and rollback are **not implemented**. Shared RS-485 bus access, conservative immediate contention diagnostics, configurable per-port direction switching and UART serialization timing **are implemented**. VEN schedules UART character-completion events; the Modbus adapter consumes those events, maintains timeouts and supports addressed reads (0x03), single-register writes (0x06), and broadcast writes. This is a bounded vertical slice, not a certified Modbus endpoint.

Next: response/turnaround timing policy, robust collision invalidation, fuller Modbus register/function coverage, then Ethernet switching and industrial protocol executors, each with explicit simulation LOD and conformance evidence.

Architecture, invariants and non-goals are described in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). See [docs/NETWORK_RELEVANCE.md](docs/NETWORK_RELEVANCE.md) for primary-source correctness and model fidelity audit, and [docs/MODBUS_RTU.md](docs/MODBUS_RTU.md) for the implemented protocol scope.

## Licenses and provenance

Licensing policy has not yet been selected for this new repository; the crates are marked `publish = false`. No code from the GPL-licensed ns-3 repository has been copied or translated. ns-3 remains a conceptual reference and potential external conformance/benchmark tool.
