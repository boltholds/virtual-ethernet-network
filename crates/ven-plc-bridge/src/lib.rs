//! Persistent Python ScheduledRuntime process, protocol v1 JSONL.
//! See docs/PLC_PYTHON_RUNTIME_BRIDGE.md for deployment and causal limits.
pub mod plant;

use sim_kernel::{AdvanceOutcome, TimeGrant, TimedOutput};
use sim_kernel::conservative::{
    ActorError, CausalInput, CausalParticipant, DirectedEvent,
};
use sim_kernel::external_boundary::{BoundaryError, FiniteReal, SignalKey, SignalValue, scan_period_ms};
use sim_kernel::resources::ParticipantId;
use sim_time::{SimDuration, SimInstant};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const VERSION: u64 = 1;
const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug)]
pub enum BridgeError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Boundary(BoundaryError),
    PeerError(String),
    InvalidProtocol,
    TimeOverflow,
    SequenceMismatch,
    TimeMismatch,
    UnsupportedValue,
    UnboundOutput(String),
    InvalidMessage,
}
impl fmt::Display for BridgeError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "{self:?}")
    }
}
impl std::error::Error for BridgeError {}
impl From<std::io::Error> for BridgeError {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}
impl From<serde_json::Error> for BridgeError {
    fn from(e: serde_json::Error) -> Self { Self::Json(e) }
}
impl From<BoundaryError> for BridgeError {
    fn from(e: BoundaryError) -> Self { Self::Boundary(e) }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IoMessage {
    /// Snapshot value becomes visible on the NEXT scheduled PLC scan.
    PlcInput { key: SignalKey, value: SignalValue },
    /// A committed output at the scan boundary.
    PlcOutput { key: SignalKey, value: SignalValue },
    /// Controlled plant-only scenario fault; never inferred from CAD B-rep.
    ValveFault(plant::ValveFault),
}

fn from_json(value: &Value) -> Result<SignalValue, BridgeError> {
    match value {
        Value::Bool(v) => Ok(SignalValue::Bool(*v)),
        Value::Number(n) => {
            if let Some(n) = n.as_i64() {
                Ok(SignalValue::Integer(n))
            } else if n.is_u64() {
                Err(BridgeError::UnsupportedValue)
            } else if let Some(f) = n.as_f64() {
                Ok(SignalValue::Real(FiniteReal::new(f)?))
            } else { Err(BridgeError::UnsupportedValue) }
        }
        Value::String(v) => Ok(SignalValue::Text(v.clone())),
        Value::Null => Ok(SignalValue::Unavailable),
        _ => Err(BridgeError::UnsupportedValue),
    }
}
fn to_json(value: &SignalValue) -> Result<Value, BridgeError> {
    match value {
        SignalValue::Bool(v) => Ok(json!(v)),
        SignalValue::Integer(v) => Ok(json!(v)),
        SignalValue::Real(v) => Ok(json!(v.get())),
        SignalValue::Text(_) | SignalValue::Unavailable => Err(BridgeError::UnsupportedValue),
    }
}

/// One live subprocess and exactly one stateful ScheduledRuntime scenario.
/// Drop kills and reaps child to avoid leaking orphaned Python worker jobs.
pub struct PythonScheduledRuntime {
    child: Child,
    input: BufWriter<ChildStdin>,
    output: BufReader<ChildStdout>,
    period: SimDuration,
    sequence: u64,
}

impl Drop for PythonScheduledRuntime {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl PythonScheduledRuntime {
    /// Call with Command::new("python3"), args -u -m backend.services.ven_runtime_worker.
    /// Caller controls the Python environment and cwd; VEN does not install dependencies.
    pub fn spawn_with_command(
        mut command: Command, graph: Value, tick_ms: u32,
    ) -> Result<Self, BridgeError> {
        let period = scan_period_ms(tick_ms)?;
        if tick_ms > 60_000 { return Err(BridgeError::InvalidProtocol); }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()) // no leaked stacktrace through VEN
            .spawn()?;
        let input = BufWriter::new(child.stdin.take().ok_or(BridgeError::InvalidProtocol)?);
        let output = BufReader::new(child.stdout.take().ok_or(BridgeError::InvalidProtocol)?);
        let mut client = Self { child, input, output, period, sequence: 0 };
        let response = client.exchange(json!({
            "version": VERSION, "op": "init", "tick_ms": tick_ms, "graph": graph
        }))?;
        if response.get("op").and_then(Value::as_str) != Some("init")
            || response.get("time_ns").and_then(Value::as_u64) != Some(0)
            || response.get("next_sequence").and_then(Value::as_u64) != Some(1)
        { return Err(BridgeError::InvalidProtocol); }
        Ok(client)
    }

    pub fn period(&self) -> SimDuration { self.period }
    pub fn sequence(&self) -> u64 { self.sequence }

    fn exchange(&mut self, message: Value) -> Result<Value, BridgeError> {
        let encoded = serde_json::to_vec(&message)?;
        if encoded.len() > MAX_REPLY_BYTES { return Err(BridgeError::InvalidProtocol); }
        self.input.write_all(&encoded)?;
        self.input.write_all(b"\n")?;
        self.input.flush()?;
        let mut raw = Vec::new();
        // Read one reply with bounded memory; Take<Read> is not a BufRead.
        loop {
            let chunk = self.output.fill_buf()?;
            if chunk.is_empty() { return Err(BridgeError::InvalidProtocol); }
            let end = chunk.iter().position(|&c| c == b'\n');
            let count = end.map(|pos| pos + 1).unwrap_or(chunk.len());
            if raw.len() + count > MAX_REPLY_BYTES {
                return Err(BridgeError::InvalidProtocol);
            }
            raw.extend_from_slice(&chunk[..count]);
            self.output.consume(count);
            if end.is_some() { break; }
        }
        let response: Value = serde_json::from_slice(&raw)?;
        if response.get("version").and_then(Value::as_u64) != Some(VERSION) {
            return Err(BridgeError::InvalidProtocol);
        }
        match response.get("ok").and_then(Value::as_bool) {
            Some(true) => Ok(response),
            Some(false) => {
                let code = response.get("code").and_then(Value::as_str)
                    .unwrap_or("INVALID_PROTOCOL");
                Err(BridgeError::PeerError(code.chars().take(64).collect()))
            }
            None => Err(BridgeError::InvalidProtocol),
        }
    }

    pub fn scan(
        &mut self, values: &BTreeMap<SignalKey, SignalValue>,
    ) -> Result<(SimInstant, BTreeMap<SignalKey, SignalValue>), BridgeError> {
        let next = self.sequence.checked_add(1).ok_or(BridgeError::TimeOverflow)?;
        let mut inputs = serde_json::Map::new();
        for (key, value) in values {
            inputs.insert(key.as_str().to_owned(), to_json(value)?);
        }
        let response = self.exchange(json!({
            "version": VERSION, "op": "scan",
            "sequence": next, "inputs": inputs
        }))?;
        if response.get("op").and_then(Value::as_str) != Some("scan")
            || response.get("sequence").and_then(Value::as_u64) != Some(next)
        { return Err(BridgeError::SequenceMismatch); }
        let time_ns = response.get("time_ns").and_then(Value::as_u64)
            .ok_or(BridgeError::TimeMismatch)?;
        let expected = (next as u128) * u128::from(self.period.as_nanos());
        if u128::from(time_ns) != expected { return Err(BridgeError::TimeMismatch); }
        let output = response.get("outputs").and_then(Value::as_object)
            .ok_or(BridgeError::InvalidProtocol)?;
        let mut decoded = BTreeMap::new();
        for (name, value) in output {
            decoded.insert(SignalKey::new(name.to_owned())?, from_json(value)?);
        }
        // Commit local sequence only after fully validating the response.
        self.sequence = next;
        Ok((SimInstant::from_nanos(time_ns), decoded))
    }
}

/// A *real* external Python engine wrapped in the shared causal time contract.
/// Delivers committed PLC outputs only at scheduled scan boundaries.
pub struct PythonPlcActor {
    runtime: PythonScheduledRuntime,
    now: SimInstant,
    next_scan: SimInstant,
    inputs: BTreeMap<SignalKey, SignalValue>,
    previous: BTreeMap<SignalKey, SignalValue>,
    route: BTreeMap<SignalKey, ParticipantId>,
}

impl PythonPlcActor {
    pub fn new(
        runtime: PythonScheduledRuntime,
        route: BTreeMap<SignalKey, ParticipantId>,
    ) -> Result<Self, BridgeError> {
        let next_scan = SimInstant::ZERO.checked_add(runtime.period())
            .ok_or(BridgeError::TimeOverflow)?;
        Ok(Self {
            runtime, now: SimInstant::ZERO, next_scan, inputs: BTreeMap::new(),
            previous: BTreeMap::new(), route,
        })
    }

    pub fn next_scan(&self) -> SimInstant { self.next_scan }
}

impl CausalParticipant<IoMessage> for PythonPlcActor {
    fn current_time(&self) -> SimInstant { self.now }
    fn next_wakeup(&self) -> Option<SimInstant> { Some(self.next_scan) }
    fn advance(
        &mut self, grant: TimeGrant,
    ) -> Result<AdvanceOutcome<DirectedEvent<IoMessage>>, ActorError> {
        if grant.until < self.now || grant.until > self.next_scan {
            return Err(Box::new(BridgeError::TimeMismatch));
        }
        let mut outputs = Vec::new();
        if grant.until == self.next_scan {
            let (actual, current) = self.runtime.scan(&self.inputs)?;
            if actual != grant.until {
                return Err(Box::new(BridgeError::TimeMismatch));
            }
            // Validate routing BEFORE emitting/committing any output.
            for key in current.keys() {
                if !self.route.contains_key(key) {
                    return Err(Box::new(BridgeError::UnboundOutput(key.as_str().to_owned())));
                }
            }
            for (key, value) in &current {
                if self.previous.get(key) != Some(value) {
                    outputs.push(TimedOutput {
                        at: grant.until,
                        value: DirectedEvent {
                            target: self.route[key],
                            payload: IoMessage::PlcOutput {
                                key: key.clone(), value: value.clone(),
                            },
                        },
                    });
                }
            }
            self.previous = current;
            self.next_scan = self.next_scan.checked_add(self.runtime.period())
                .ok_or(BridgeError::TimeOverflow)?;
        }
        self.now = grant.until;
        Ok(AdvanceOutcome {
            reached: self.now, outputs, next_wakeup: Some(self.next_scan),
        })
    }
    fn receive(&mut self, event: CausalInput<IoMessage>) -> Result<(), ActorError> {
        if event.at != self.now { return Err(Box::new(BridgeError::TimeMismatch)); }
        match event.payload {
            IoMessage::PlcInput { key, value } => {
                // Validate the actual Python runtime input subset on receipt.
                to_json(&value)?;
                self.inputs.insert(key, value);
                Ok(())
            }
            _ => Err(Box::new(BridgeError::InvalidMessage)),
        }
    }
}
