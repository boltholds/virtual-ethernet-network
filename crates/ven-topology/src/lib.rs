//! Network inventory and physical adjacency, not a packet forwarder.
//! Node/port/link IDs are distinct. No Ethernet/RS-485 protocol simulation here.

use std::collections::{BTreeMap, BTreeSet};
use ven_core::PortId;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct NodeId(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct LinkId(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterfaceKind {
    Ethernet,
    Rs485,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Port {
    pub node: NodeId,
    pub kind: InterfaceKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Link {
    pub endpoints: (PortId, PortId),
    pub kind: InterfaceKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TopologyError {
    DuplicateNode,
    DuplicatePort,
    DuplicateLink,
    MissingNode,
    MissingPort,
    SelfConnection,
    SameNodeConnection,
    IncompatibleInterfaces,
    PortAlreadyConnected,
    InvalidLinkKind,
}

#[derive(Clone, Debug, Default)]
pub struct Topology {
    nodes: BTreeSet<NodeId>,
    ports: BTreeMap<PortId, Port>,
    links: BTreeMap<LinkId, Link>,
    connected: BTreeMap<PortId, LinkId>,
}

impl Topology {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_node(&mut self, id: NodeId) -> Result<(), TopologyError> {
        if !self.nodes.insert(id) {
            return Err(TopologyError::DuplicateNode);
        }
        Ok(())
    }

    pub fn add_port(&mut self, id: PortId, port: Port) -> Result<(), TopologyError> {
        if !self.nodes.contains(&port.node) {
            return Err(TopologyError::MissingNode);
        }
        if self.ports.contains_key(&id) {
            return Err(TopologyError::DuplicatePort);
        }
        self.ports.insert(id, port);
        Ok(())
    }

    /// Connects two distinct nodes via exactly two ports. Multi-drop buses
    /// will use a separate medium model; do not model RS-485 as an Ethernet link.
    pub fn connect(
        &mut self,
        id: LinkId,
        a: PortId,
        b: PortId,
    ) -> Result<(), TopologyError> {
        if self.links.contains_key(&id) {
            return Err(TopologyError::DuplicateLink);
        }
        if a == b {
            return Err(TopologyError::SelfConnection);
        }
        let left = self.ports.get(&a).ok_or(TopologyError::MissingPort)?;
        let right = self.ports.get(&b).ok_or(TopologyError::MissingPort)?;
        if left.node == right.node {
            return Err(TopologyError::SameNodeConnection);
        }
        if left.kind != right.kind {
            return Err(TopologyError::IncompatibleInterfaces);
        }
        if self.connected.contains_key(&a) || self.connected.contains_key(&b) {
            return Err(TopologyError::PortAlreadyConnected);
        }
        self.links.insert(id, Link {
            endpoints: (a, b),
            kind: left.kind,
        });
        self.connected.insert(a, id);
        self.connected.insert(b, id);
        Ok(())
    }

    pub fn port(&self, id: PortId) -> Option<&Port> {
        self.ports.get(&id)
    }

    pub fn link(&self, id: LinkId) -> Option<&Link> {
        self.links.get(&id)
    }

    pub fn link_for_port(&self, id: PortId) -> Option<(LinkId, &Link)> {
        let link_id = *self.connected.get(&id)?;
        Some((link_id, self.links.get(&link_id)?))
    }

    pub fn links(&self) -> impl Iterator<Item = (&LinkId, &Link)> {
        self.links.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topology() -> Topology {
        let mut g = Topology::new();
        g.add_node(NodeId(1)).unwrap();
        g.add_node(NodeId(2)).unwrap();
        g.add_node(NodeId(3)).unwrap();
        g.add_port(PortId(10), Port { node: NodeId(1), kind: InterfaceKind::Ethernet }).unwrap();
        g.add_port(PortId(20), Port { node: NodeId(2), kind: InterfaceKind::Ethernet }).unwrap();
        g.add_port(PortId(30), Port { node: NodeId(3), kind: InterfaceKind::Rs485 }).unwrap();
        g
    }

    #[test]
    fn valid_link_records_adjacency() {
        let mut g = topology();
        g.connect(LinkId(1), PortId(10), PortId(20)).unwrap();
        assert_eq!(g.link_for_port(PortId(20)).unwrap().0, LinkId(1));
        assert_eq!(g.link(LinkId(1)).unwrap().kind, InterfaceKind::Ethernet);
    }

    #[test]
    fn invalid_links_do_not_mutate_graph() {
        let mut g = topology();
        assert_eq!(g.connect(LinkId(1), PortId(10), PortId(30)), Err(TopologyError::IncompatibleInterfaces));
        assert_eq!(g.connect(LinkId(1), PortId(10), PortId(10)), Err(TopologyError::SelfConnection));
        assert!(g.links().next().is_none());
        g.connect(LinkId(1), PortId(10), PortId(20)).unwrap();
        assert_eq!(g.connect(LinkId(2), PortId(10), PortId(30)), Err(TopologyError::IncompatibleInterfaces));
        assert_eq!(g.connect(LinkId(1), PortId(10), PortId(20)), Err(TopologyError::DuplicateLink));
    }

    #[test]
    fn rejects_duplicate_ids_and_missing_owners() {
        let mut g = topology();
        assert_eq!(g.add_node(NodeId(1)), Err(TopologyError::DuplicateNode));
        assert_eq!(g.add_port(PortId(40), Port { node: NodeId(50), kind: InterfaceKind::Ethernet }), Err(TopologyError::MissingNode));
        assert_eq!(g.add_port(PortId(10), Port { node: NodeId(1), kind: InterfaceKind::Ethernet }), Err(TopologyError::DuplicatePort));
    }

    #[test]
    fn port_cannot_connect_twice() {
        let mut g = topology();
        g.add_node(NodeId(4)).unwrap();
        g.add_port(PortId(40), Port { node: NodeId(4), kind: InterfaceKind::Ethernet }).unwrap();
        g.connect(LinkId(1), PortId(10), PortId(20)).unwrap();
        assert_eq!(g.connect(LinkId(2), PortId(10), PortId(40)), Err(TopologyError::PortAlreadyConnected));
    }
}
