# Modbus RTU — implemented scope and remaining requirements

Primary reference: [MODBUS over Serial Line Specification and Implementation Guide V1.02](https://www.modbus.org/file/secure/modbusoverserial.pdf) (December 20, 2006), §§2.1, 2.2, 2.5.1, 6.2.2.

## Implemented in `ven-modbus`

- RTU ADU **encoding/decoding**, allowed address range `0..=247`, generic nonzero function byte, maximum 256 bytes, CRC16 (initial `0xffff`, polynomial `0xa001`) transmitted low byte first.
- `RtuTiming` with **11-bit** Modbus RTU character format. For baud rates <=19200, `t1.5` and `t3.5` are character-time based, rounded up in integer nanoseconds. At >19200, the *recommended* fixed 750us and 1750us timers are used.
- `RtuReceiver` processes one **UART character-completion** event per byte and rejects a frame for inter-character gaps greater than `t1.5`; it finalizes only after `t3.5` silence. A partial frame / CRC error produces a typed `Discarded` result.
- Cross-crate test over VEN's existing RS-485 shared bus: encoded request -> physical broadcast -> decode, followed by a separately scheduled reply after the minimum silence. Another test verifies overlapping transmitters cause diagnostics, not fabricated valid packets.

## Not implemented

- Modbus master/slave application state machines, register map, interpretation of function codes, broadcast-write authorization, exception response rules, transaction timeouts, retries or serial turnaround control.
- Automatic enforcement of `t3.5` by the shared RS-485 bus: the bus does not know that Modbus is running above it.
- Byte-level UART emission from `VenNetwork`: `send_rs485` still sends an **opaque whole frame**, so the RtuReceiver is only driven by individually timestamped bytes supplied by an explicit caller. Modbus t1.5 fault-injection needs a UART-character event source before integration into end-to-end network simulation.
- Address filtering in the RS-485 physical layer. **Physical reception of a frame by all ports does not mean every Modbus slave accepts the request**. Real Modbus receivers filter by slave address.
- Modbus TCP, ASCII, electrical waveform, reflections, termination, biasing, or certification.

## Next vertical slice

1. Add explicit UART character emission timing events and error injection in a serial-link adapter, not to `ven-medium`'s Ethernet models.
2. Connect `RtuReceiver` per attached device, including address filtering and silent periods.
3. Build a master-request/slave-response state machine, with one outstanding transaction, broadcast without response, and protocol-specific timeouts.
4. Test collision/partial-frames, t1.5/t3.5, CRC, and response timeouts against independent reference traces.
