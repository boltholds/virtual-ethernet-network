# Virtual Ethernet Network (VEN)

Rust network simulation components for the PLC Web ecosystem.

The first implementation is being prepared on a feature branch. The intended design uses a reusable simulation-time/kernel layer and a network-local event scheduler; controller-specific semantics stay in PLC profiles.

Design goals: standalone usage, embedding in PLC Web, explicit virtual time, protocol-specific communication models, and no unverified claims of physical or protocol conformance.

**Status:** early architecture and implementation. No production-ready PROFINET, EtherCAT, or Modbus emulation is claimed.

License selection is pending. No code is copied from ns-3.
