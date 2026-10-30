# PLC Web / Gefest integration audit and external boundary

Date: 2026-10-27. This is a **source-grounded interface audit**, not a claim that a real PLC or equipment process is executing in VEN.

## Repositories and exact sources inspected

- `boltholds/PLC-web`, branch `feature/scheduled-ir-virtual-plc`:
  - `backend/src/backend/services/scheduled_runtime_adapter.py`: `ScheduledRuntime.step(graph, inputs, state, tick_ms)` returns `ScheduledRuntimeResult` with scan number, time_ms, execution order, outputs, values and state.
  - `backend/src/backend/services/virtual_plc.py`: `VirtualPLC.scan(plan, inputs, state, tick_ms)` advances retained `_runtime.scan_number` and `_runtime.time_ms` by a whole scan. Task operations include input snapshot, state read, execute, state commit and output commit. A scan is not a nanosecond-edge callback.
  - `backend/src/backend/services/control_scheduler.py`: builds deterministic typed-IR task scheduling with phase separation and cycle checks.
  - `backend/src/backend/services/interfaces.py`: `SimulationInputs` currently accepts `bool | int | float`, while `StateValue` extends that set with `str | None`. These are current **branch-specific** types, not a comprehensive IEC 61131-3 type system.
  - `backend/src/backend/services/simulation_session.py`: session state, sequence and tick_ms ownership reside inside Python, using a separate legacy `SimulationEngine`.
- `boltholds/Gefest-CAD`, branch `feat/g6e-schema-v2-gefest-20261018`:
  - `backend/app/routes/workspace.py`: project-scoped scene queries and atomic sketch operations.
  - `backend/app/services/workspace_transactions.py`: revisioned atomic document changes and scene patches.
  - `backend/app/geometry/g6_project_bridge.py`: bounded JSON/subprocess interface to RustSolid's geometry document operations.
- `boltholds/Gefest-CAD`, `main`: `docs/industrial-horizontal-slice.md` documents Rust solver handling CAD constraints and revisioned scene changes. This **does not establish** dynamic physical simulation of valves/motors/sensors.

The `PLC-web/main` checked on this date contained an older minimal skeleton; active runtime interfaces above were inspected on the named feature branch. Before integrating production code, resolve the actual deployment branch/version and run direct compatibility tests against it.

## New contract in VEN

`sim-kernel::external_boundary` adds source-aligned **transport-neutral** types:

- `SignalValue::{Bool, Integer, Real(FiniteReal), Text, Unavailable}`; non-finite REAL values are rejected, absent values do not silently become FALSE or zero.
- `PlcScanInputs` and `PlcScanCommit` with strict scan-number and completion-time validation. Integer `tick_ms` converts exactly to virtual nanoseconds, and a scan is committed **atomically at its boundary**.
- `PlcScanInputs::validate_legacy_python_inputs` validates the narrow input subset documented for the inspected Python runtime; other signal types fail explicitly.
- `GeometrySnapshotRef` binds Gefest's project, scene revision and body identity. A stale scene revision is explicitly rejected; **a geometry revision is not a process simulation tick**.

Tests cover precise ms->ns mapping, scan ordering, time mismatch, NaN/infinite rejection, unsupported PLC input values and stale geometry references. This is a validated *boundary contract*, not an RPC transport or a deployed runtime bridge.

## Honest integration status

The Rust conservative coordinator currently owns in-process `CausalParticipant` actors. Python `ScheduledRuntime` cannot implement a Rust trait directly; a concrete Python process bridge (e.g. local IPC/RPC) and a transaction protocol are needed. Requirements before production integration:

1. Agree on authoritative runtime source: live `VirtualPLC`/scheduled IR path vs legacy `SimulationEngine`, preserving its task/scan semantics and retained state.
2. Provide a versioned request/response DTO around real `ScheduledRuntime.step`, with idempotent scan IDs, strict time ownership and matching state revision. An engine error must not partially commit an output snapshot.
3. Bridge the Python process to the Rust coordinator without wall-clock coupling; deny early/late responses or contradictory next-wakeup values. On loss, fail closed or explicitly pause the whole simulation (no invisible extrapolation).
4. Connect actual PLC inputs/outputs by canonical symbol identity, type validation and an unambiguous mapping to VEN device signals. A PLC scan sees one frozen input snapshot and its output commit takes effect at the scan boundary, **not halfway through a scan**.
5. Separate CAD geometry from physical process models. Gefest/RustSolid provides revisioned shapes and transforms; a dynamic plant actor must provide motor/valve/robot state equations, delays, faults, sensors and actuator/physics I/O. These dynamic semantics were **not** found in the inspected Gefest modules.
6. Freeze a Gefest geometry revision for each simulation scenario; if the document changes, pause/rebind or explicitly version a scene update before the next physics integration step. Do not infer collision/constraints/kinematics from a static B-rep alone.
7. Prove with a real PLC program + configured valve that actuator commands leave a committed PLC scan, traverse VEN, change the plant actor state, and sensor feedback is visible at the next appropriate PLC input snapshot. Run both character-timed and digital-bit LODs, compare times and failure behavior.

## Boundary notes

The current `FiniteReal`/scalar subset is intentionally narrower than the eventual PLC Web array/STRUCT/ENUM model. The new layer is not a replacement for the canonical type/IR system; additional typed variants must be negotiated and validated rather than flattened or coerced.

No sources from either external repository were copied into VEN. No production PLC Runtime execution or Gefest device dynamics are claimed as implemented in this commit.
