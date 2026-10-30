#!/usr/bin/env python3
"""Explicit protocol fixture, not PLC Web ScheduledRuntime."""
import json
import sys

tick = 0
sequence = 0
for line in sys.stdin:
    req = json.loads(line)
    if req["op"] == "init":
        tick = req["tick_ms"]
        res = {"version": 1, "ok": True, "op": "init", "time_ns": 0, "next_sequence": 1}
    elif req["op"] == "scan" and req["sequence"] == sequence + 1:
        sequence += 1
        inputs = req["inputs"]
        res = {"version": 1, "ok": True, "op": "scan",
               "sequence": sequence, "time_ns": sequence * tick * 1_000_000,
               "outputs": {"valve_cmd": inputs.get("cmd", False),
                           "monitor.open": inputs.get("valve.opened", False)},
               "values": {}}
    else:
        res = {"version": 1, "ok": False, "code": "SEQUENCE_CONFLICT"}
    print(json.dumps(res), flush=True)
