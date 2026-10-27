# VEN topology contract

`ven-topology` stores typed connectivity: nodes own ports; point-to-point links connect pairs of ports; RS-485 BusId objects attach multiple shared-medium ports.

Invariants:
- Node, port, and link IDs are unique within one topology.
- Ports refer to existing nodes.
- Point-to-point endpoints must be of the same interface kind.
- A point-to-point port may be attached to at most one link.
- A rejected link operation leaves the topology unchanged.
- Ethernet runtime links are full duplex; an RS-485 BusId has two or more useful members and uses an independent shared-medium model.
- A port cannot simultaneously be attached to a point-to-point link and a bus.

`VenNetwork::connect_ethernet` validates topology before registering its physical medium. Sending a frame resolves the destination using the validated adjacency; VEN does not implement switching or automatic multi-hop forwarding yet.

Currently BusId represents logical access to a common RS-485 two-wire medium; it does **not** model spatial position along a trunk, stubs, line termination, grounding or impedance. Those must be modeled explicitly before electrical fidelity claims. Future: trunk/branch cable segments, repeaters, EtherCAT chains and richer Ethernet bridge/switch topologies.
