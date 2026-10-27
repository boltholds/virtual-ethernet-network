# VEN topology contract

`ven-topology` stores typed, physical point-to-point adjacency: nodes own ports, and links connect ports belonging to distinct nodes.

Invariants:
- Node, port, and link IDs are unique within one topology.
- Ports refer to existing nodes.
- Point-to-point endpoints must be of the same interface kind.
- A point-to-point port may be attached to at most one link.
- A rejected link operation leaves the topology unchanged.
- Currently supported Ethernet runtime links are full duplex; RS-485 enum presence **does not** mean shared-bus simulation exists.

`VenNetwork::connect_ethernet` validates topology before registering its physical medium. Sending a frame resolves the destination using the validated adjacency; VEN does not implement switching or automatic multi-hop forwarding yet.

Planned: explicit Bus/Segment entities for multidrop RS-485, EtherCAT chains and richer Ethernet bridge/switch topologies. Avoid representing RS-485 collisions as Ethernet MAC collisions.
