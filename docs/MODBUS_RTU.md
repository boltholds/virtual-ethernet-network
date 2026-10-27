# Modbus RTU — implemented scope and remaining requirements

Primary reference: [MODBUS over Serial Line Specification and Implementation Guide V1.02](https://www.modbus.org/file/secure/modbusoverserial.pdf) (December 20, 2006), §§2.1, 2.2, 2.5.1, 6.2.2.

## Implemented in `ven-modbus`

- RTU ADU **encoding/decoding**, allowed address range `0..=247`, generic nonzero function byte, maximum 256 bytes, CRC16 (initial `0xffff`, polynomial `0xa001`) transmitted low byte first.
- `RtuTiming` with **11-bit** Modbus RTU character format. For baud rates <=19200, `t1.5` and `t3.5` are character-time based, rounded up in integer nanoseconds. At >19200, the *recommended* fixed 750us and 1750us timers are used.
- `RtuReceiver` processes one **UART character-completion** event per byte and rejects a frame for inter-character gaps greater than `t1.5`; it finalizes only after `t3.5` silence. A partial frame / CRC error produces a typed `Discarded` result.
- `ven-sim::send_rs485_uart` emits true virtual-timestamped UART character-completion events, with deterministic `DropByte`, `CorruptByte`, and `GapAfter` fault modes. This is a character-timed model, not voltage/bit-accurate physical sampling.
- `ven-modbus::session` provides one-outstanding-request master state, response validation, timeout and exception handling, Modbus 0x03 and 0x06 slave handlers, register values and broadcast write-without-reply semantics.
- `ven-modbus-sim` binds per-port `RtuReceiver`s to VEN characters, runs receive-flush and timeout deadlines under the shared `TimeGrant`, and automatically transmits slave replies after RTU frame silence. On a new immediate `Rs485Collision` event, the adapter invalidates **partially received frames at the overlap time**; transmission-completion contention reports are only summaries.
- Per-port `Rs485Turnaround` exposes `driver_enable`, `driver_release` and `receiver_enable`. Tests check collisions during post-data DE hold, delayed RE readiness leading to master timeout, and partial-frame invalidation when a second driver starts mid-frame.

## Opt-in bit-sampled Modbus RTU connection

`ModbusBus::character_timed(network, timing)` uses the original `VenNetwork` character-completion transport; `ModbusBus::digital_bits(phy)` constructs a `DigitalModbusRtuLine` and derives Modbus timing from the PHY's UART/baud configuration (rejecting invalid 8N1 RTU configuration instead of silently falling back). Only one backend is stored per bus instance.

In digital mode, `PhysicalEvent::Character` with `UartSample::Byte` is delivered to the **same** RTU receiver/master/slave state machines. `ParityError`, `FramingError`, `Indeterminate`, or disabled receive results invalidate that port's incomplete RTU frame and surface `LineEvent::PhysicalReceiveError`; no valid bytes or synthetic Modbus exceptions are fabricated. `PhysicalEvent::Contention` invalidates all local incomplete receivers and surfaces a bus diagnostic. t3.5 and master timeout are controlled only by shared virtual time.

Cross-LOD integration tests assert one matching FC03 reply, broadcast writes without replies, bit-level parity fault -> timeout, collision -> timeout, and rejection of unsupported UART format.

**Boundary:** One `ModbusBus` owns one transport. The new `ModbusBusHost` additionally rejects different registered participants claiming the same `BusId` (including mixed LOD modes) using atomic session-scoped claims. The minimal time coordinator remains single-participant; unregistered runners and separate registries are not automatically detected. The digital PHY remains a digital/logic model, not analog electrical conformance.

## Not implemented

- Full Modbus application model: only functions 0x03 and 0x06 are implemented; remaining function codes, full register/device configuration and extended exception policies are not. Automatic retries are not implemented. DE/RE turnaround is now modeled separately from RTU `t3.5`, with configurable delays, but without electrical pin/waveform simulation.
- The raw RS-485 bus does not enforce `t3.5`; only the `ven-modbus-sim` protocol adapter schedules responder behavior after `t3.5`. Direct raw network sends can violate Modbus timing.
- `send_rs485_uart` supports virtual character events. The legacy `send_rs485` retains opaque **whole-frame** behavior for compatibility, and is not interpreted as Modbus UART input by the adapter.
- Modbus address filtering occurs in the dedicated protocol endpoint, **not** inside the RS-485 physical medium, which still broadcasts sampled characters.
- Modbus TCP, ASCII, electrical waveform, reflections, termination, biasing, or certification.

## Next vertical slice

1. Make the future multi-participant coordinator require a shared authoritative registry for all bus-owning protocols and PHYs; resolve cross-participant causality, not just BusId uniqueness.
2. Atomic master request submission/rollback under timestamp overflow, complete protocol function coverage, and explicit retry policies.
3. Real oscil­loscope captures (not yet supplied), calibrated receiver delay and differential voltage/cable measurements.
4. Property-based randomized tests around UART sampling and t1.5/t3.5 boundaries.
