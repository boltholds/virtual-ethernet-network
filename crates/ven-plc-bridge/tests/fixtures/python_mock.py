#!/usr/bin/env python3
"""Wire-protocol fixture ONLY — not PLC Web ScheduledRuntime."""
import json
import sys

tick = 0
next_seq = 1
for line in sys.stdin:
    message = json.loads(line)
    if message["op"] == "init":
        tick = message["tick_ms"]
        answer = {"version": 1, "ok": True, "op": "init",
                  "next_sequence": 1, "time_ns": 0}
    elif message["op"] == "scan":
        seq = message["sequence"]
        if seq != next_seq:
            answer = {"version": 1, "ok": False, "code": "SEQUENCE_CONFLICT"}
        else:
            next_seq += 1
            answer = {"version": 1, "ok": True, "op": "scan",
                      "sequence": seq, "time_ns": seq * tick * 1000000,
                      "outputs": {"valve_cmd": message["inputs"].get("cmd", False)},
                      "values": {}}
    else:
        answer = {"version": 1, "ok": False, "code": "INVALID_REQUEST"}
    sys.stdout.write(json.dumps(answer) + "\n")
    sys.stdout.flush()
