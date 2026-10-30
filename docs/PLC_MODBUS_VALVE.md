# PLC Web ↔ VEN Modbus RTU ↔ deterministic valve process

Status: simulated serial-link **integration implemented**; real Python runtime + Rust Modbus end-to-end exercise prepared but its private GitHub Actions runner has not executed. No real hardware or Gefest dynamics validation.

## Actual data path

1. PLC Web `ScheduledRuntime.step`, hosted in a persistent Python JSONL worker, commits `valve_cmd` at a fixed scan boundary (100 ms in the example).
2. `PythonPlcActor` emits the changed command to `ValveModbusGateway` and one periodic poll intent per scan.
3. The gateway executes an addressed **Modbus RTU FC06** request to a configured slave (e.g. address 2, register 0, value 1). Only one master request may be in flight. Additional reads/writes are queued (maximum 16).
4. The slave accepts the validated FC06 after UART transmission and RTU framing silence `t3.5`. At that instant, `LineEvent::SlaveWrite` starts `ValvePlantActor` motion. **The master ACK does not initiate motion**, eliminating the earlier timing ambiguity.
5. The process actor raises/lower its open/closed limit switch flags after configured physical delay. These sensor events are routed **only to the gateway** and update the simulated device holding registers.
6. An explicit PLC-initiated **Modbus FC03** read queries three consecutive sensor registers; the master receives a reply only after modeled serialization and `t3.5` silence. Typed `PlcInput` values enter the Python PLC actor at the real response timestamp and are sampled on the **next** scheduled PLC scan. No direct valve→PLC feedback shortcut is present.
7. A timeout or Modbus exception raises `communication_fault` rather than fabricating a valid sensor reading. A successful transaction clears it. A damaged UART start/parity bit on the slave side prevents the FC06 application and ultimately causes a communication timeout.

## Local pilot register map

| 0-based holding-register address | Meaning | Origin | Access |
|---|---|---|---|
| 0 | Open command (0/1) | PLC FC06 | Read/write |
| 1 | Open-limit feedback (0/1) | Valve process | Read by FC03 |
| 2 | Closed-limit feedback (0/1) | Valve process | Read by FC03 |
| 3 | Process fault (0/1) | Valve process | Read by FC03 |

The communication fault is **not** a fake Modbus holding register. It is a local PLC-side result of protocol failures.

Register numbers and meanings are specific to this **simulated demonstration device**, not industry-wide Modbus definitions or a real manufacturer's register map.

## Files and testing

- `crates/ven-plc-bridge/src/modbus_valve.rs`: typed gateway implementing `CausalParticipant<IoMessage>`.
- `crates/ven-modbus-sim/src/{lib,digital}.rs`: new addressed `SlaveWrite` event on successful FC06 application.
- `crates/ven-plc-bridge/tests/modbus_valve_e2e.rs`: both network LODs, stuck valve, parity fault producing no actuation and a communication-fault indication. The test process `python_modbus_mock.py` is **explicitly synthetic**.
- `crates/ven-plc-bridge/examples/real_plc_modbus_valve.rs`: real Python ScheduledRuntime input/output graph plus VEN digital PHY, Modbus master/slave, valve and observed feedback. Requires the separate private PLC Web PR #654 and its Python dependencies.

For the real example, from the VEN checkout:

```sh
export PYTHONPATH=/path/to/PLC-web/backend/src
cargo run -p ven-plc-bridge --example real_plc_modbus_valve
```

A dedicated private PLC Web workflow `.github/workflows/ven-runtime-bridge.yml` includes the real example. Its latest job was rejected before allocation of a runner (no job steps), so **real** multi-repository execution has **not yet been independently certified**. The Rust crate's CI builds the example and tests the protocol against its controlled fixture.

## Fidelity and remaining limits

- The gateway uses only FC03 and FC06, not arbitrary Modbus functions; at most one master request is in flight, polling is queued/coalesced, and the demo does not model an independently running OEM slave firmware.
- `SlaveWrite` is emitted at RTU receive/acceptance time. There is no slave-side task scheduling delay, isolated I/O sampling clock, retransmit policy or redundant master.
- The dynamic valve model is discrete (full open/closed after fixed delays). It has typed stuck/limit-switch failure scenarios but does not calculate pneumatic/hydraulic flow, acceleration, pressure or torque.
- Rust master/slave semantics are **simulated**, not conformance-certified. The digital PHY is a bit-sampled approximation, not an analog RS-485 solver.
- Python JSONL subprocess I/O remains **blocking** with no timeout/watchdog. An unresponsive Python process can stall the whole coordinator. Before production deployment, implement an abortable, deadline-bound IPC transport.
- Gefest's CAD revision may label an object, but **Gefest and RustSolid do not provide validated dynamics in this slice**.
- No physical RS-485 oscilloscope recordings have been supplied. No real hardware acceptance test has been performed.
