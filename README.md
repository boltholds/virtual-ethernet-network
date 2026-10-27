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
| `ven-medium` | Timed full-duplex point-to-point links and simplified RS-485 two-wire shared bus with UART duration |
| `ven-topology` | Typed node/port/link/RS-485 bus connectivity |
| `ven-modbus` | Modbus RTU ADU codec, CRC16, t1.5/t3.5 timing and UART-character receiver |
| `ven-sim` | Embeddable network participant with frame arrivals and contention diagnostics |

The two `sim-*` crates are **VEN-independent** and intentionally kept portable so they can later serve PLC Web, Gefest-related simulation adapters, and other simulation participants. The current shared-kernel implementation validates grants for **one** participant; a real multi-participant co-simulation coordinator is future work.

## Run

Requires Rust 1.76+.

```sh
cargo test --workspace --all-targets
```

The vertical slice schedules a frame over a 1 Mbit/s full-duplex link. `ven-sim` returns control to the host coordinator on the exact modeled arrival timestamp, and both link directions can transmit independently.

## Scope and limitations

Currently this is **a working foundation, not a complete Ethernet/industrial protocol simulator**. Frame bytes are opaque, and line serialization uses only bytes supplied by the caller (physical overhead/IFG/FCS are not added). Ethernet switching, electrical-layer collision resolution, PROFINET, EtherCAT, Modbus transaction execution, conformance validation, advanced local clocks, global multi-participant coordination and rollback are **not implemented**. Shared RS-485 bus access and conservative overlapping-driver detection **are implemented**, with UART serialization timing. Modbus RTU frame encoding/decoding and byte-wise receive timing are implemented in a separate crate; master/slave state machines and byte-level RS-485 delivery are not.

Next: byte-level UART events + Modbus master/slave state machines, then Ethernet switching and industrial protocol executors, each with explicit simulation LOD and conformance evidence.

Architecture, invariants and non-goals are described in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). See [docs/NETWORK_RELEVANCE.md](docs/NETWORK_RELEVANCE.md) for primary-source correctness and model fidelity audit, and [docs/MODBUS_RTU.md](docs/MODBUS_RTU.md) for the implemented protocol scope.

## Licenses and provenance

Licensing policy has not yet been selected for this new repository; the crates are marked `publish = false`. No code from the GPL-licensed ns-3 repository has been copied or translated. ns-3 remains a conceptual reference and potential external conformance/benchmark tool.
