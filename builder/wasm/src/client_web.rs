//! The host client owns protocol state. JavaScript owns records, payload I/O and time.

use obc_link::flat::{
    client::{Action, Client, Error, Event, Options, Outcome, QueryOutcome},
    wire::*,
    Ceilings, Channel, DisplayName, EntryMeta, ObjectId, ObjectKind, Revision, StoreId,
};
use serde::Deserialize;
use serde_json::{json, Value};
use wasm_bindgen::prelude::*;

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum BrowserRequest {
    List { kind: Option<u16>, cursor: Option<BrowserCursor> },
    Status { id: String, revision: String },
    Get { id: String, revision: String },
    Put { id: String, revision: String, length: String, crc: u32, kind: u16, name: String },
    Remove { id: String, revision: String },
    Cancel { transfer: u32 },
    Arm { id: String, revision: String },
    Format { expected: String, replacement: String },
}

#[derive(Deserialize)]
struct BrowserCursor {
    id: String,
    revision: String,
    sequence: String,
}

fn number(value: &str) -> Result<u64, JsValue> {
    value.parse().map_err(|_| JsValue::from_str("Invalid unsigned 64-bit value."))
}

fn store(value: &str) -> Result<StoreId, JsValue> {
    if value.len() != 32 || !value.is_ascii() {
        return Err(JsValue::from_str("A store ID has 32 hexadecimal digits."));
    }
    let mut bytes = [0; 16];
    for (at, byte) in bytes.iter_mut().enumerate() {
        *byte =
            u8::from_str_radix(&value[at * 2..at * 2 + 2], 16).map_err(|_| JsValue::from_str("Invalid store ID."))?;
    }
    Ok(StoreId(bytes))
}

fn store_text(store: StoreId) -> String {
    store.0.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn kind(value: u16) -> Result<ObjectKind, JsValue> {
    ObjectKind::decode(value).ok_or_else(|| JsValue::from_str("Invalid object kind."))
}

impl BrowserRequest {
    fn request(self) -> Result<Request, JsValue> {
        Ok(match self {
            Self::List { kind: value, cursor } => Request::List(ListRequest {
                kind: value.map(kind).transpose()?,
                cursor: cursor
                    .map(|c| -> Result<_, JsValue> {
                        Ok(ListCursor {
                            id: ObjectId(number(&c.id)?),
                            revision: Revision(number(&c.revision)?),
                            sequence: number(&c.sequence)?,
                        })
                    })
                    .transpose()?,
            }),
            Self::Status { id, revision } => {
                Request::Status(StatusRequest { id: ObjectId(number(&id)?), revision: Revision(number(&revision)?) })
            }
            Self::Get { id, revision } => {
                Request::Get(GetRequest { id: ObjectId(number(&id)?), revision: Revision(number(&revision)?) })
            }
            Self::Put { id, revision, length, crc, kind: value, name } => Request::Put(PutRequest {
                id: ObjectId(number(&id)?),
                expected: Revision(number(&revision)?),
                payload_len: number(&length)?,
                payload_crc: crc,
                kind: kind(value)?,
                name: DisplayName::new(&name)
                    .ok_or_else(|| JsValue::from_str("A display name has at most 48 bytes."))?,
            }),
            Self::Remove { id, revision } => {
                Request::Remove(RemoveRequest { id: ObjectId(number(&id)?), expected: Revision(number(&revision)?) })
            }
            Self::Cancel { transfer } => Request::Cancel(CancelRequest { transfer: RequestId(transfer) }),
            Self::Arm { id, revision } => {
                Request::Arm(ArmRequest { package: ObjectId(number(&id)?), expected: Revision(number(&revision)?) })
            }
            Self::Format { expected, replacement } => {
                Request::Format(FormatRequest { expected: store(&expected)?, replacement: store(&replacement)? })
            }
        })
    }
}

fn entry(entry: EntryMeta) -> Value {
    json!({
        "objectId": entry.id.0.to_string(), "revision": entry.revision.0.to_string(),
        "payloadLength": entry.payload_len.to_string(), "payloadCrc32": entry.payload_crc,
        "kind": entry.kind.value(), "flags": entry.flags.bits(),
        "displayName": String::from_utf8_lossy(entry.name.as_bytes()),
    })
}

fn error(error: Error) -> Value {
    let name = match error {
        Error::Remote(refusal) => {
            return json!({"kind": "remote", "refusal": {
                "code": refusal.code as u16, "detail": refusal.detail, "context": refusal.context.to_string(),
            }})
        }
        Error::StoreChanged { previous, current } => {
            return json!({
                "kind": "storeChanged", "previous": store_text(previous), "current": store_text(current),
            })
        }
        Error::Busy => "busy",
        Error::InvalidInput => "invalidInput",
        Error::Protocol => "protocol",
        Error::Checksum => "checksum",
        Error::Timeout => "timeout",
        Error::Cancelled => "cancelled",
        Error::Io => "io",
        Error::LinkLost => "linkLost",
        Error::CatalogChanged => "catalogChanged",
        Error::NotCommitted => "notCommitted",
        Error::OutcomeUnknown => "outcomeUnknown",
        Error::RequestIdsExhausted => "requestIdsExhausted",
    };
    json!({"kind": name})
}

fn outcome(outcome: Outcome) -> Value {
    match outcome {
        Outcome::Catalog { store, sequence, entries } => json!({
            "kind": "catalog", "storeId": store_text(store), "commitSequence": sequence.to_string(),
            "entries": entries.into_iter().map(entry).collect::<Vec<_>>(),
        }),
        Outcome::Status(status) => json!({
            "kind": "status", "state": status.state as u8, "headRevision": status.revision.0.to_string(),
            "headPayloadLength": status.payload_len.to_string(), "headPayloadCrc32": status.payload_crc,
        }),
        Outcome::Get(result) => json!({
            "kind": "get", "revisionServed": result.revision.0.to_string(),
            "payloadLength": result.payload_len.to_string(), "payloadCrc32": result.payload_crc,
        }),
        Outcome::Put(result) => json!({
            "kind": "put", "objectId": result.id.0.to_string(), "revision": result.revision.0.to_string(),
            "payloadLength": result.payload_len.to_string(), "payloadCrc32": result.payload_crc,
        }),
        Outcome::Remove { sequence } => json!({"kind": "remove", "commitSequence": sequence.map(|v| v.to_string())}),
        Outcome::Cancel(cancelled) => json!({"kind": "cancel", "cancelled": cancelled}),
        Outcome::Arm { reserve, sequence } => json!({
            "kind": "arm", "rollbackObjectId": reserve.0.to_string(), "commitSequence": sequence.to_string(),
        }),
        Outcome::Format(store) => json!({"kind": "format", "storeId": store_text(store)}),
        Outcome::ArchiveRide(_) => unreachable!("The browser surface has no archive request."),
    }
}

#[wasm_bindgen(js_name = StoreClientAction)]
pub struct JsAction {
    metadata: String,
    bytes: Vec<u8>,
}

#[wasm_bindgen(js_class = StoreClientAction)]
impl JsAction {
    #[wasm_bindgen(getter)]
    pub fn metadata(&self) -> String {
        self.metadata.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }
}

impl From<Action> for JsAction {
    fn from(action: Action) -> Self {
        let mut bytes = Vec::new();
        let metadata = match action {
            Action::Send { token, channel, record } => {
                let header = if channel == Channel::Control {
                    decode_request(&record).ok().map(|(header, _)| header)
                } else {
                    None
                };
                bytes = record;
                json!({"kind": "send", "token": token.to_string(), "channel": match channel {
                    Channel::Control => "control", Channel::Stream => "stream",
                }, "opcode": header.map(|h| h.opcode as u8), "requestId": header.map(|h| h.request.0)})
            }
            Action::ReadSource { token, offset, max_len } => json!({
                "kind": "readSource", "token": token.to_string(), "offset": offset.to_string(), "maxLength": max_len,
            }),
            Action::WriteSink { token, offset, bytes: payload } => {
                bytes = payload;
                json!({"kind": "writeSink", "token": token.to_string(), "offset": offset.to_string()})
            }
            Action::ResetSink => json!({"kind": "resetSink"}),
            Action::Progress { done, total } => {
                json!({"kind": "progress", "done": done.to_string(), "total": total.to_string()})
            }
            Action::ResetChannels => json!({"kind": "resetChannels"}),
            Action::Restore => json!({"kind": "restore"}),
            Action::Complete(result) => match result {
                Ok(value) => json!({"kind": "complete", "ok": true, "outcome": outcome(value)}),
                Err(value) => json!({"kind": "complete", "ok": false, "error": error(value)}),
            },
        };
        Self { metadata: metadata.to_string(), bytes }
    }
}

#[wasm_bindgen(js_name = StoreClient)]
pub struct JsClient {
    inner: Client,
}

#[wasm_bindgen(js_class = StoreClient)]
impl JsClient {
    #[wasm_bindgen(constructor)]
    pub fn new(
        control_ceiling: usize,
        stream_ceiling: usize,
        timeout_ms: u32,
        reconnect_attempts: u8,
    ) -> Result<JsClient, JsValue> {
        let ceilings = Ceilings::new(control_ceiling, stream_ceiling)
            .ok_or_else(|| JsValue::from_str("Invalid record ceilings."))?;
        Ok(Self {
            inner: Client::new(
                ceilings,
                Options { timeout_ms: timeout_ms as u64, reconnect_attempts, ..Options::default() },
            ),
        })
    }

    pub fn start(&mut self, request: &str, expected_store: Option<String>, now: u64) -> Result<(), JsValue> {
        let request: BrowserRequest = serde_json::from_str(request).map_err(|e| JsValue::from_str(&e.to_string()))?;
        self.inner
            .start_scoped(request.request()?, expected_store.as_deref().map(store).transpose()?, now)
            .map_err(|e| JsValue::from_str(&error(e).to_string()))
    }

    #[wasm_bindgen(js_name = nextAction)]
    pub fn next_action(&mut self) -> Option<JsAction> {
        self.inner.next_action().map(Into::into)
    }

    pub fn query(&mut self, request: &str, expected_store: Option<String>, now: u64) -> Result<u64, JsValue> {
        let request: BrowserRequest = serde_json::from_str(request).map_err(|e| JsValue::from_str(&e.to_string()))?;
        self.inner
            .query(request.request()?, expected_store.as_deref().map(store).transpose()?, now)
            .map(|id| id.0)
            .map_err(|e| JsValue::from_str(&error(e).to_string()))
    }

    #[wasm_bindgen(js_name = nextQueryResult)]
    pub fn next_query_result(&mut self) -> Option<String> {
        self.inner.next_query_result().map(|(id, result)| {
            match result {
                Ok(value) => {
                    let value = match value {
                        QueryOutcome::Page { store, sequence, more, entries } => json!({
                            "kind": "page", "storeId": store_text(store), "commitSequence": sequence.to_string(),
                            "more": more, "entries": entries.into_iter().map(entry).collect::<Vec<_>>(),
                        }),
                        QueryOutcome::Status(status) => outcome(Outcome::Status(status)),
                    };
                    json!({"query": id.0.to_string(), "ok": true, "outcome": value})
                }
                Err(value) => json!({"query": id.0.to_string(), "ok": false, "error": error(value)}),
            }
            .to_string()
        })
    }

    #[wasm_bindgen(js_name = nextDeadline)]
    pub fn next_deadline(&self) -> Option<u64> {
        self.inner.next_deadline_ms()
    }

    #[wasm_bindgen(js_name = activeTransfer)]
    pub fn active_transfer(&self) -> Option<u32> {
        self.inner.active_transfer_id().map(|id| id.0)
    }

    #[wasm_bindgen(js_name = setUploadWindow)]
    pub fn set_upload_window(&mut self, source_records: usize, pending_records: usize) -> Result<(), JsValue> {
        self.inner
            .set_upload_window(source_records, pending_records)
            .map_err(|e| JsValue::from_str(&error(e).to_string()))
    }

    pub fn control(&mut self, record: &[u8], now: u64) {
        self.inner.event(Event::Control(record), now);
    }
    pub fn stream(&mut self, record: &[u8], now: u64) {
        self.inner.event(Event::Stream(record), now);
    }
    pub fn source(&mut self, token: u64, offset: u64, bytes: &[u8], now: u64) {
        self.inner.event(Event::Source { token, offset, bytes }, now);
    }
    pub fn written(&mut self, token: u64, now: u64) {
        self.inner.event(Event::Written(token), now);
    }
    #[wasm_bindgen(js_name = sinkWritten)]
    pub fn sink_written(&mut self, token: u64, offset: u64, len: usize, now: u64) {
        self.inner.event(Event::SinkWritten { token, offset, len }, now);
    }
    pub fn tick(&mut self, now: u64) {
        self.inner.event(Event::Tick, now);
    }
    pub fn cancel(&mut self, now: u64) {
        self.inner.event(Event::Cancel, now);
    }
    #[wasm_bindgen(js_name = ioFailed)]
    pub fn io_failed(&mut self, token: u64, now: u64) {
        self.inner.event(Event::IoFailed(token), now);
    }
    #[wasm_bindgen(js_name = linkLost)]
    pub fn link_lost(&mut self, now: u64) {
        self.inner.event(Event::LinkLost, now);
    }
    pub fn restored(&mut self, control_ceiling: usize, stream_ceiling: usize, now: u64) -> Result<(), JsValue> {
        let ceilings = Ceilings::new(control_ceiling, stream_ceiling)
            .ok_or_else(|| JsValue::from_str("Invalid record ceilings."))?;
        self.inner.event(Event::Restored(ceilings), now);
        Ok(())
    }
}
