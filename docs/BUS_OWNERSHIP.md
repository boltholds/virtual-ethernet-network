# Exclusive RS-485 BusId ownership

Status: implemented session-scoped resource uniqueness. A separate conservative multi-participant coordinator is now available in `sim-kernel::conservative`.

## Registry

sim-kernel::resources::ResourceRegistry<R> is independent of VEN and uses typed ParticipantId(u64).
Each claim reserves ALL listed resources atomically or changes nothing. The registry rejects duplicate
participant registrations, empty claims, repeated keys and ownership conflicts with the specific owner.
Release removes all owned resources; registry instances are session-local and intentionally not Clone.

## Modbus session host

ven-modbus-sim::ModbusBusHost stores private ModbusBus executors and one ResourceRegistry<BusId>.
At registration, the host takes ownership of the participant. CharacterTimed instances claim every
BusId in their VenNetwork topology, including when a single network holds several buses.
DigitalBits instances claim the single BusId exposed by Rs485DigitalPhy.
An overlap between either LOD rejects the entire new participant before it becomes runnable.

The host deliberately does not expose mutable ModbusBus/PHY/network handles. Its public operations
provide registration, unregistration, read-only ownership lookup, Modbus endpoint configuration,
request submission and advance_one(id, TimeGrant). The latter advances ONLY the requested executor
under the caller's time grant; global cross-participant causality and time barriers are not solved.

To use: construct a configured ModbusBus, create ModbusBusHost::new(), call
host.register(ParticipantId(1), bus), then operate through host.attach_master, attach_slave,
seed_register, request and advance_one. A second registered participant claiming the same BusId
returns HostError::Registry(RegistryError::ResourceOccupied { resource, owner }).

## Guarantees and exclusions

- Duplicate BusId registration is prevented among participants owned by the same ModbusBusHost.
- Multi-bus claim failures do not partially mutate ownership; repeated registration is rejected.
- Unregister drops the executor and releases claims; a later new participant may claim the keys.
- Separate simulation sessions can legitimately use the same BusId; there is no process-global lock.
- The generic registry is also used by `ConservativeCoordinator` to guard exclusive claims across heterogeneous registered actors. The specialized `ModbusBusHost` still only owns ModbusBus executors.
- Live topology mutation or LOD switching after registration is not offered via the host.
- The conservative coordinator owns all actors registered through its API and enforces deterministic event barriers. Unregistered actors can still run independently; a production application must make the shared coordinator its sole simulation entrypoint.

## Tests

Unit and integration tests cover mixed-LOD collisions, original owner identification, atomic
multi-bus failure, duplicate participant IDs, empty claims, release/reacquisition, independent
sessions, and a full registered digital Modbus FC03 request/response under TimeCoordinator grants.

Ownership is a model-integrity contract, NOT a representation of Modbus address arbitration,
electrical RS-485 contention, real-world device identity or cross-process infrastructure.