# Conservative multi-participant simulation coordinator

Implemented in `sim-kernel::conservative`, with a generic `CausalParticipant<E>` trait, `DirectedEvent<E>`, `CausalInput<E>`, `Claims<R>` and `ConservativeCoordinator<E,R>`.

## Core invariant

A participant must announce its **earliest possible externally relevant event** using `next_wakeup()`.
The coordinator chooses the minimum next wakeup of all participants and the requested horizon.
It advances **every actor to the same barrier** before delivering any events occurring at the barrier.
The actor `advance()` must reach exactly the granted time and may emit only events stamped at that barrier.
Unexpected earlier returns, retroactive timestamps, stale wakeups, unknown recipients or failed actors
are treated as causal integrity failures. The coordinator becomes poisoned because there is no rollback.

Events at the same timestamp are delivered by ascending `ParticipantId` and each sender's local emission order.
If delivery schedules another event at the SAME instant, the coordinator handles it in a subsequent microstep.
A configurable maximum barrier count prevents infinite zero-time loops and infinitely dense event sequences.
Nothing advances to a later timestamp until all pending same-time work is settled.

## Ownership

On registration, each actor is moved into the coordinator. `Claims::Exclusive` atomically claims resources
using the same generic `sim-kernel::resources::ResourceRegistry<R>` used by the Modbus host.
`Claims::NoExclusiveResources` is for resource-free PLC logic and observers. A network actor must claim
all physical resource identifiers it drives: the coordinator cannot magically discover hidden resources
inside arbitrary external actor implementations.

`CausalModbusBridge` in `ven-modbus-sim` wraps a real `ModbusBus` and maps protocol input/output to
application-defined events. Call `bridge.claimed_buses()` BEFORE moving it into the coordinator and register
with `Claims::Exclusive(buses)`. One coordinator is the only owner of all registered participant state;
the bridge does not expose a mutable network reference after registration.

## Confirmed end-to-end scenario

Integration tests `crates/ven-modbus-sim/tests/causal_simulation.rs` simulate three registered actors:

1. **Toy PLC actor** submits an FC03 request to Modbus at virtual time zero.
2. **Real Modbus RTU/VEN actor**, configured for either CharacterTimed or DigitalBits, runs UART and bus events.
3. **Toy equipment actor** receives an Actuate command after the PLC sees the Modbus response.

The equipment acknowledgement arrives at the PLC in a later microstep with the **same virtual timestamp**
as the PLC's command. Tests also verify the final horizon and BusId resource claim release.
These PLC/equipment actors are deliberately synthetic fixtures, **not** integrations with PLC Web Runtime or Gefest.

## Important limits

- In-process, single-threaded conservative coordination only; no cross-host transport or speculative rollback.
- `next_wakeup` is a **contract**. A lying actor can partially mutate before being rejected; session poisoning
  prevents subsequent execution but is not an undo operation.
- At present the coordinator trusts a caller to supply truthful resource claims. Registration does not inspect
  arbitrary third-party mutable handles or automatically protect actors outside the session.
- Every registered participant advances at every global barrier. This provides strict correctness boundaries
  for the slice, but can become expensive for many actors and bit-level PHY events.
- No message backpressure, actor priority policy beyond deterministic ID ordering, future-scheduled external
  injection, snapshot/restore, persistence or real PLC scan cycle semantics.
- Individual raw ModbusBusHost remains available as a **separate** specialized host. Do not register its
  underlying executors in both hosts or run two independent schedulers for the same physical BusId.

## Next engineering milestone

Build real adapters for the PLC execution graph and Gefest equipment processes, with explicit typed cross-domain
I/O messages and stable actor identities. Then add conservative lookahead, bounded resource use and checkpoint
support while preserving the existing event-order tests. Hardware RS-485 validation still requires real traces.