# Real Python ScheduledRuntime ↔ Rust VEN vertical slice

Implementation branch (PLC Web): [PR #654](https://github.com/boltholds/PLC-web/pull/654), derived from `feature/scheduled-ir-virtual-plc`. It introduces `backend.services.ven_runtime_worker` and tests against the **actual** `ScheduledRuntime.step()`, with persistent state and monotonic sequence numbers.

VEN component: `ven-plc-bridge` crate. It owns the Python subprocess, uses version 1 UTF-8 JSONL over stdin/stdout, and exposes `PythonPlcActor` implementing `CausalParticipant<IoMessage>`. Unlike the previous mock PLC tests, the example below executes the real Python ScheduledRuntime from that separate repository.

## Run

Requires Python 3.13, the PLC Web backend dependencies from the **named feature branch plus PR #654**, and a Rust toolchain. Install PLC Web dependencies into the Python environment beforehand, then run from the VEN checkout:

```sh
export PYTHONPATH=/path/to/PLC-web/backend/src
cargo run -p ven-plc-bridge --example real_plc_valve
```

`PLC_PYTHON` optionally selects the Python executable. The sample graph contains real PLC Web `input` and `output` nodes and a connection. PLC scan period is 100 ms; the valve's commanded opening time is 150 ms. The network-independent pilot asserts the open request at virtual time 0, commits the PLC coil at 100 ms, and expects valve-open feedback at 250 ms. This feedback is held as an input for the next PLC scan at 300 ms. **This example does not transmit that signal over Modbus**; bridging these process-signal DTOs into the existing Modbus bus is a further step.

`ven-plc-bridge/tests/bridge_valve.rs` uses an explicitly labelled Python **wire-protocol fixture**, not the full PLC runtime, to keep base VEN CI independent of PLC Web's private Python environment. Independently, PLC Web PR #654 tests the real `ScheduledRuntime` and stateful RS latch. The Rust example above is the actual cross-process composition; it requires both repositories checked out and configured, and has not been independently validated against production deployment.

## Protocol

Process starts **once per simulation session**:

```json
{"version":1,"op":"init","tick_ms":100,"graph":{"environment":"opta","hardwareId":"opta-wifi","nodes":[],"connections":[]}}
```

Each scheduled PLC scan is an explicit request, not a wall-clock timer:

```json
{"version":1,"op":"scan","sequence":1,"inputs":{"open_request.q":true,"valve.opened":false}}
```

The Python process returns a committed sequence, `time_ns`, and output map. It retains `RuntimeState` and the compiled plan cache in memory. The exact same scan request may be retried idempotently; conflicting sequence/replay or non-finite inputs fail with typed public codes. Python exceptions and stacktraces are never returned as JSON payloads.

Rust validates reply schema, sequence, time and scalar values, and emits changed PLC outputs at committed scan boundaries. The `ConservativeCoordinator` synchronizes the PLC participant and valve participant with a single nanosecond virtual clock. A Python failure poisons the coordinator because no distributed rollback protocol exists.

## Valve process pilot

`ValvePlantActor` is an independent **discrete equipment dynamics model** with configurable opening/closing delays, open/closed limit switches and controllable `ValveFault::{Stuck,OpenLimitFailed,ClosedLimitFailed}` faults. It resolves a movement event at its physical virtual completion instant; the PLC samples resulting input state on its next scan.

The optional `ValveGeometry::Snapshot` carries a strict `GeometrySnapshotRef` to a Gefest CAD revision. **No dynamics are inferred from B-rep, assembly constraints, static dimensions or RustSolid geometry**. Without a real configured CAD snapshot, use `ValveGeometry::Unbound`. There is no pressure/flow/force/torque model, partial valve position or analog pneumatic profile.

## Limitations and gates

- Python subprocess communication is **synchronous blocking I/O**, with bounded response size but no watchdog/timeouts or sandbox. It is suitable for local controlled proof of concept only. A hanging Python worker can hang the coordinator; a hardened local IPC transport must impose a deadline and process termination policy.
- No mutual authentication, shared-machine tenancy isolation, deployment orchestration, distributed snapshots/recovery, hard real-time PLC timing or hot schema migration.
- Current Python ScheduledRuntime scalar inputs are `bool|int|float`. Unsupported/undefined values reject explicitly, without converting them to false or zero. Rich IEC data types require a versioned extension.
- **New:** The optional `ValveModbusGateway` now routes actual Modbus FC06/FC03 transactions for a separate end-to-end valve scenario. Its `SlaveWrite` event is triggered at slave acceptance, **not** at master reply. The earlier simple `real_plc_valve` example remains a direct-causal baseline; run `real_plc_modbus_valve` for the serial network path. Both character and digital-bit LOD versions pass standalone Rust integration tests using an explicitly synthetic Python protocol fixture. The real Python worker cross-repository runner has not yet executed.
- The physical plant state is discrete, not a Gefest computational mechanics engine. It requires independent acceptance criteria for valve motion delays, interlock timeouts, and fault injection.
- The two GitHub PRs remain independent and unmerged; **nothing changes the deployed PLC Web backend or VEN main branch**.

## Traceable source boundaries

- PLC Web feature branch: `backend/src/backend/services/scheduled_runtime_adapter.py`, `virtual_plc.py` and `interfaces.py`.
- Python worker: `backend/src/backend/services/ven_runtime_worker.py`, tested in `backend/tests/test_ven_runtime_worker.py`.
- VEN: `crates/ven-plc-bridge/src/lib.rs`, `plant.rs`, `modbus_valve.rs`, both examples and tests. Modbus internals: `ven-modbus-sim::LineEvent::SlaveWrite` is published in both character/digital adapters. See [PLC_MODBUS_VALVE.md](PLC_MODBUS_VALVE.md).
