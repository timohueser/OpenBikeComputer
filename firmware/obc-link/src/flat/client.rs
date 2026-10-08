//! A host client over complete protocol records. Adapters perform every I/O action and report its
//! completion. Payload sources must support reads from offset zero after a restored link.

use alloc::{collections::VecDeque, vec, vec::Vec};
use obc_crc::Crc32;

use super::{wire::*, ArchiveResult, Ceilings, Channel, EntryFlags, EntryMeta, ObjectId, StoreId};

mod control;
mod query;

use query::Query;
pub use query::{QueryId, QueryOutcome};

fn record_id(record: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(record[at..at + 4].try_into().unwrap())
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub timeout_ms: u64,
    pub cancel_timeout_ms: u64,
    pub list_restarts: u8,
    pub reconnect_attempts: u8,
    pub remove_duplicate_creates: bool,
    /// Whole stream records per payload read, bounded by the pending window.
    pub upload_source_records: usize,
    /// Stream records that may await real transport completion.
    pub upload_pending_records: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            timeout_ms: 15_000,
            cancel_timeout_ms: 2_000,
            list_restarts: 4,
            reconnect_attempts: 1,
            remove_duplicate_creates: false,
            upload_source_records: 1,
            upload_pending_records: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Busy,
    InvalidInput,
    Protocol,
    Checksum,
    Remote(Refusal),
    Timeout,
    Cancelled,
    Io,
    LinkLost,
    StoreChanged { previous: StoreId, current: StoreId },
    CatalogChanged,
    NotCommitted,
    OutcomeUnknown,
    RequestIdsExhausted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Catalog { store: StoreId, sequence: u64, entries: Vec<EntryMeta> },
    Status(StatusResponse),
    Get(TransferResponse),
    Put(TransferResponse),
    Remove { sequence: Option<u64> },
    Cancel(bool),
    Arm { reserve: ObjectId, sequence: u64 },
    Format(StoreId),
    ArchiveRide(ArchiveResult),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// The token identifies this write, not the protocol request. Report `Written` after delivery.
    Send {
        token: u64,
        channel: Channel,
        record: Vec<u8>,
    },
    ReadSource {
        token: u64,
        offset: u64,
        max_len: usize,
    },
    WriteSink {
        token: u64,
        offset: u64,
        bytes: Vec<u8>,
    },
    /// Discard partial download output before a retry or an unsuccessful terminal result.
    ResetSink,
    Progress {
        done: u64,
        total: u64,
    },
    ResetChannels,
    Restore,
    Complete(Result<Outcome, Error>),
}

pub enum Event<'a> {
    Control(&'a [u8]),
    Stream(&'a [u8]),
    Source {
        token: u64,
        offset: u64,
        bytes: &'a [u8],
    },
    Written(u64),
    SinkWritten {
        token: u64,
        offset: u64,
        len: usize,
    },
    Tick,
    Cancel,
    /// Failure of a Send, ReadSource or WriteSink action with this completion token.
    IoFailed(u64),
    LinkLost,
    Restored(Ceilings),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Purpose {
    Introduce,
    Run,
    Identity,
    Status,
    FindCreate,
    RemoveDuplicate,
}

#[derive(Clone, Copy)]
struct Pending {
    id: RequestId,
    request: Request,
    purpose: Purpose,
}

struct Operation {
    request: Request,
    expected_store: Option<StoreId>,
    started: bool,
    received: u64,
    settled: u64,
    crc: Crc32,
    answer: Option<TransferResponse>,
    source_due: Option<(u64, u64, usize)>,
    sinks: VecDeque<(u64, u64, usize)>,
    stream_writes: VecDeque<StreamWrite>,
    entries: Vec<EntryMeta>,
    sequence: Option<u64>,
    restarts: u8,
    reconnects: u8,
    recovered: Option<TransferResponse>,
}

impl Operation {
    fn new(request: Request, expected_store: Option<StoreId>) -> Self {
        Self {
            request,
            expected_store,
            started: false,
            received: 0,
            settled: 0,
            crc: Crc32::new(),
            answer: None,
            source_due: None,
            sinks: VecDeque::new(),
            stream_writes: VecDeque::new(),
            entries: Vec::new(),
            sequence: None,
            restarts: 0,
            reconnects: 0,
            recovered: None,
        }
    }

    fn transfer(&self) -> bool {
        matches!(self.request, Request::Get(_) | Request::Put(_))
    }

    fn reset_transfer(&mut self) {
        self.received = 0;
        self.settled = 0;
        self.crc = Crc32::new();
        self.answer = None;
        self.source_due = None;
        self.sinks.clear();
        self.stream_writes.clear();
    }
}

#[derive(Clone, Copy)]
enum Write {
    Control,
}

struct StreamWrite {
    token: u64,
    offset: u64,
    len: usize,
    written: bool,
}

struct Cancellation {
    id: RequestId,
    transfer: RequestId,
    cause: Error,
    answered: bool,
    confirmed: bool,
    transfer_answered: bool,
}

/// One primary operation and independent metadata queries. Cancellation targets the live transfer.
/// No transport, file, task, timer or whole payload is owned by this client.
pub struct Client {
    options: Options,
    ceilings: Ceilings,
    store: Option<StoreId>,
    connected: bool,
    next_id: u32,
    next_token: u64,
    pending: Option<Pending>,
    operation: Option<Operation>,
    write: Option<(u64, Write)>,
    cancellation: Option<Cancellation>,
    restoring: bool,
    deadline: u64,
    actions: VecDeque<Action>,
    queries: Vec<Query>,
    query_results: VecDeque<(QueryId, Result<QueryOutcome, Error>)>,
}

impl Client {
    pub fn new(ceilings: Ceilings, options: Options) -> Self {
        Self {
            options,
            ceilings,
            store: None,
            connected: true,
            next_id: 1,
            next_token: 1,
            pending: None,
            operation: None,
            write: None,
            cancellation: None,
            restoring: false,
            deadline: 0,
            actions: VecDeque::new(),
            queries: Vec::new(),
            query_results: VecDeque::new(),
        }
    }

    pub fn store_id(&self) -> Option<StoreId> {
        self.store
    }

    pub fn is_busy(&self) -> bool {
        self.operation.is_some()
    }

    /// Configure the deadline policy only while all operations are idle. None leaves timeouts
    /// to the adapter; transport failures and cancellation still produce terminal results.
    pub fn set_timeout_ms(&mut self, timeout_ms: Option<u64>) -> Result<(), Error> {
        if self.is_busy() || !self.queries.is_empty() {
            return Err(Error::Busy);
        }
        self.options.timeout_ms = timeout_ms.unwrap_or(u64::MAX);
        Ok(())
    }

    pub fn next_deadline_ms(&self) -> Option<u64> {
        self.queries
            .iter()
            .map(|query| query.deadline)
            .chain(self.operation.as_ref().map(|_| self.deadline))
            .filter(|deadline| *deadline != u64::MAX)
            .min()
    }

    /// A reader may park only after the corresponding control write settles. Ignored late
    /// replies leave this expectation live; a transfer answer can settle before its stream.
    pub fn awaiting_control(&self) -> bool {
        if !self.connected || self.restoring {
            return false;
        }
        self.queries.iter().any(|query| query.token.is_none())
            || self.write.is_none()
                && self.pending.is_some()
                && self.operation.as_ref().is_some_and(|op| {
                    self.cancellation
                        .as_ref()
                        .map_or(op.answer.is_none(), |cancel| !cancel.answered || !cancel.transfer_answered)
                })
    }

    /// GET can send stream records before its control answer. Once the announced byte count
    /// arrives, the adapter stops reading while the remaining sink writes settle.
    pub fn awaiting_stream(&self) -> bool {
        self.connected
            && !self.restoring
            && self.cancellation.is_none()
            && self.write.is_none()
            && self
                .pending
                .is_some_and(|pending| pending.purpose == Purpose::Run && matches!(pending.request, Request::Get(_)))
            && self.operation.as_ref().is_some_and(|op| op.answer.is_none_or(|answer| op.received < answer.payload_len))
    }

    pub fn active_transfer_id(&self) -> Option<RequestId> {
        if self.restoring {
            return None;
        }
        self.pending
            .filter(|pending| {
                pending.purpose == Purpose::Run && matches!(pending.request, Request::Get(_) | Request::Put(_))
            })
            .map(|pending| pending.id)
    }

    pub fn next_action(&mut self) -> Option<Action> {
        self.actions.pop_front()
    }

    /// Configure finite upload reads and pending records while the primary operation is idle.
    /// Adapters may group records, but report Written only after their actual batch completes.
    pub fn set_upload_window(&mut self, source_records: usize, pending_records: usize) -> Result<(), Error> {
        if self.operation.is_some() {
            return Err(Error::Busy);
        }
        if source_records == 0
            || pending_records < source_records
            || pending_records.checked_mul(self.ceilings.stream()).is_none()
        {
            return Err(Error::InvalidInput);
        }
        self.options.upload_source_records = source_records;
        self.options.upload_pending_records = pending_records;
        Ok(())
    }

    /// A LIST starts at the first page; pagination and snapshot restarts belong to the client.
    /// ARCHIVE_RIDE is an assertion by the adapter that archive persistence barriers completed.
    pub fn start(&mut self, request: Request, now_ms: u64) -> Result<(), Error> {
        self.start_scoped(request, None, now_ms)
    }

    /// Names the card a persisted object reference belongs to. A reconnect never moves an
    /// outstanding operation to another card with coincidentally equal object/revision numbers.
    pub fn start_scoped(
        &mut self,
        request: Request,
        expected_store: Option<StoreId>,
        now_ms: u64,
    ) -> Result<(), Error> {
        if self.is_busy()
            || self.actions.iter().any(|action| {
                !matches!(action,
            Action::Send { token, .. } if self.queries.iter().any(|query| query.token == Some(*token)))
            })
        {
            return Err(Error::Busy);
        }
        if !self.connected {
            return Err(Error::LinkLost);
        }
        if matches!(request, Request::List(ListRequest { cursor: Some(_), .. })) {
            return Err(Error::InvalidInput);
        }
        if matches!(request, Request::Put(_)) {
            self.set_upload_window(self.options.upload_source_records, self.options.upload_pending_records)?;
        }
        let mut record = [0u8; MAX_REQUEST_LEN];
        encode_request(&mut record, RequestId(1), request).ok_or(Error::InvalidInput)?;
        self.operation = Some(Operation::new(request, expected_store.or(self.store)));
        let result = if self.store.is_none() && !matches!(request, Request::List(_) | Request::Format(_)) {
            self.send_request(Request::List(ListRequest { kind: None, cursor: None }), Purpose::Introduce, now_ms)
        } else {
            self.run(now_ms)
        };
        if result.is_err() {
            self.operation = None;
        }
        result
    }

    pub fn event(&mut self, event: Event<'_>, now_ms: u64) {
        match &event {
            Event::Control(record) if self.query_control(record, now_ms) => return,
            Event::Written(token) if self.query_written(*token, now_ms) => return,
            Event::IoFailed(token) if self.query_failed(*token) => return,
            Event::Tick => self.expire_queries(now_ms),
            Event::LinkLost => self.fail_queries(Error::LinkLost),
            _ => {}
        }
        if !self.is_busy() {
            match event {
                Event::LinkLost => {
                    self.connected = false;
                    self.store = None;
                }
                Event::Restored(ceilings) => {
                    self.connected = true;
                    self.ceilings = ceilings;
                    self.store = None;
                }
                _ => {}
            }
            return;
        }
        let result = match event {
            Event::Control(record) => self.control(record, now_ms),
            Event::Stream(record) => self.stream(record, now_ms),
            Event::Source { token, offset, bytes } => self.source(token, offset, bytes, now_ms),
            Event::Written(token) => self.written(token, now_ms),
            Event::SinkWritten { token, offset, len } => self.sink_written(token, offset, len, now_ms),
            Event::Cancel => self.cancel(Error::Cancelled, now_ms),
            Event::IoFailed(token) if self.action_pending(token) => self.cancel(Error::Io, now_ms),
            Event::IoFailed(_) => Ok(()),
            Event::LinkLost => self.link_lost(now_ms),
            Event::Restored(ceilings) => {
                if !self.restoring {
                    Err(Error::Protocol)
                } else {
                    self.restoring = false;
                    self.connected = true;
                    self.ceilings = ceilings;
                    self.send_request(
                        Request::List(ListRequest { kind: None, cursor: None }),
                        Purpose::Identity,
                        now_ms,
                    )
                }
            }
            Event::Tick if now_ms >= self.deadline => {
                if let Some(cancel) = &self.cancellation {
                    let uncertain_put = self
                        .operation
                        .as_ref()
                        .is_some_and(|op| matches!(op.request, Request::Put(put) if op.received == put.payload_len));
                    if uncertain_put && !cancel.confirmed && !cancel.transfer_answered {
                        Err(Error::OutcomeUnknown)
                    } else {
                        Err(cancel.cause)
                    }
                } else if self.restoring {
                    Err(Error::Timeout)
                } else {
                    self.cancel(Error::Timeout, now_ms)
                }
            }
            Event::Tick => Ok(()),
        };
        if let Err(error) = result {
            if matches!(error, Error::Protocol | Error::Checksum | Error::Io)
                && self.cancellation.is_none()
                && self.cancel(error, now_ms).is_ok()
            {
                return;
            }
            self.finish(Err(error));
        }
    }

    fn id(&mut self) -> Result<RequestId, Error> {
        let value = self.next_id;
        self.next_id = value.checked_add(1).ok_or(Error::RequestIdsExhausted)?;
        Ok(RequestId(value))
    }

    fn send_request(&mut self, request: Request, purpose: Purpose, now: u64) -> Result<(), Error> {
        let id = self.id()?;
        let mut bytes = vec![0; MAX_REQUEST_LEN];
        let len = encode_request(&mut bytes, id, request).ok_or(Error::InvalidInput)?;
        if len > self.ceilings.control() {
            return Err(Error::InvalidInput);
        }
        bytes.truncate(len);
        self.pending = Some(Pending { id, request, purpose });
        self.send(Channel::Control, bytes, Write::Control)?;
        self.deadline = now.saturating_add(self.options.timeout_ms);
        Ok(())
    }

    fn token(&mut self) -> Result<u64, Error> {
        let token = self.next_token;
        self.next_token = token.checked_add(1).ok_or(Error::RequestIdsExhausted)?;
        Ok(token)
    }

    fn send(&mut self, channel: Channel, record: Vec<u8>, write: Write) -> Result<(), Error> {
        let token = self.token()?;
        self.write = Some((token, write));
        self.actions.push_back(Action::Send { token, channel, record });
        Ok(())
    }

    fn run(&mut self, now: u64) -> Result<(), Error> {
        let op = self.operation.as_mut().unwrap();
        op.reset_transfer();
        op.started = true;
        let request = op.request;
        if let (Some(previous), Some(current)) = (op.expected_store, self.store) {
            if previous != current {
                return Err(Error::StoreChanged { previous, current });
            }
        }
        if let Request::ArchiveRide(source) = request {
            if let Some(store) = self.store {
                if source.store != store {
                    return Err(Error::StoreChanged { previous: source.store, current: store });
                }
            }
        }
        if matches!(request, Request::Get(_)) {
            self.actions.push_back(Action::ResetSink);
        }
        if let Request::Put(put) = request {
            self.actions.push_back(Action::Progress { done: 0, total: put.payload_len });
        }
        self.send_request(request, Purpose::Run, now)
    }

    fn written(&mut self, token: u64, now: u64) -> Result<(), Error> {
        if self.write.is_some_and(|(expected, _)| expected == token) {
            self.write = None;
            if self.cancellation.is_some() || self.restoring {
                return Ok(());
            }
            self.deadline = now.saturating_add(self.options.timeout_ms);
            if self.pending.as_ref().is_some_and(|p| p.purpose == Purpose::Run && matches!(p.request, Request::Put(_)))
            {
                self.request_source()?;
            }
            return self.finish_transfer();
        }
        if self.cancellation.is_some() || self.restoring {
            return Ok(());
        }
        let Some(op) = self.operation.as_mut() else {
            return Ok(());
        };
        let Some(write) = op.stream_writes.iter_mut().find(|write| write.token == token) else {
            return Ok(());
        };
        if write.written {
            return Ok(());
        }
        write.written = true;
        let before = op.settled;
        while op.stream_writes.front().is_some_and(|write| write.written) {
            let write = op.stream_writes.pop_front().unwrap();
            if write.offset != op.settled {
                return Err(Error::Protocol);
            }
            op.settled += write.len as u64;
        }
        if op.settled != before {
            let Request::Put(put) = op.request else {
                return Err(Error::Protocol);
            };
            self.actions.push_back(Action::Progress { done: op.settled, total: put.payload_len });
        }
        self.deadline = now.saturating_add(self.options.timeout_ms);
        self.request_source()?;
        self.finish_transfer()
    }

    fn request_source(&mut self) -> Result<(), Error> {
        let op = self.operation.as_ref().unwrap();
        let Request::Put(put) = op.request else {
            return Err(Error::Protocol);
        };
        if op.source_due.is_some() || op.received == put.payload_len {
            return Ok(());
        }
        let available = self.options.upload_pending_records - op.stream_writes.len();
        let payload = (self.ceilings.stream() - STREAM_HEADER_LEN).min(u16::MAX as usize);
        let remaining = put.payload_len - op.received;
        let records = self
            .options
            .upload_source_records
            .min(usize::try_from(remaining.div_ceil(payload as u64)).unwrap_or(usize::MAX));
        if available < records {
            return Ok(());
        }
        let max_len = remaining.min((records * payload) as u64) as usize;
        let token = self.token()?;
        let op = self.operation.as_mut().unwrap();
        op.source_due = Some((token, op.received, max_len));
        self.actions.push_back(Action::ReadSource { token, offset: op.received, max_len });
        Ok(())
    }

    fn source(&mut self, token: u64, offset: u64, bytes: &[u8], now: u64) -> Result<(), Error> {
        if self.cancellation.is_some() || self.restoring {
            return Ok(());
        }
        let Some(op) = self.operation.as_mut() else {
            return Ok(());
        };
        let Some((expected, due, max_len)) = op.source_due else {
            return Ok(());
        };
        if token != expected {
            return Ok(());
        }
        let Request::Put(put) = op.request else {
            return Err(Error::Protocol);
        };
        op.source_due = None;
        if self.write.is_some()
            || offset != due
            || bytes.len() > max_len
            || offset != op.received
            || bytes.is_empty()
            || bytes.len() as u64 > put.payload_len - offset
        {
            return Err(Error::Protocol);
        }
        let mut crc = op.crc;
        crc.update(bytes);
        let received = op.received + bytes.len() as u64;
        if received == put.payload_len && crc.finalize() != put.payload_crc {
            return Err(Error::Checksum);
        }
        op.crc = crc;
        op.received = received;
        let id = self.pending.as_ref().ok_or(Error::Protocol)?.id;
        let payload = (self.ceilings.stream() - STREAM_HEADER_LEN).min(u16::MAX as usize);
        let mut at = offset;
        for chunk in bytes.chunks(payload) {
            let token = self.token()?;
            let mut record = vec![0; STREAM_HEADER_LEN + chunk.len()];
            write_stream(&mut record, id, at, chunk.len()).ok_or(Error::Protocol)?;
            record[STREAM_HEADER_LEN..].copy_from_slice(chunk);
            self.operation.as_mut().unwrap().stream_writes.push_back(StreamWrite {
                token,
                offset: at,
                len: chunk.len(),
                written: false,
            });
            self.actions.push_back(Action::Send { token, channel: Channel::Stream, record });
            at += chunk.len() as u64;
        }
        self.deadline = now.saturating_add(self.options.timeout_ms);
        self.request_source()
    }

    fn stream(&mut self, record: &[u8], now: u64) -> Result<(), Error> {
        if self.cancellation.is_some() || self.restoring {
            return Ok(());
        }
        let Some(pending) = self.pending.as_ref() else {
            return Ok(());
        };
        if record.len() >= 4 && record_id(record, 0) != pending.id.0 {
            return Ok(());
        }
        let (frame, bytes) = StreamFrame::split(record).ok_or(Error::Protocol)?;
        if pending.purpose != Purpose::Run || !matches!(pending.request, Request::Get(_)) {
            return Ok(());
        }
        if record.len() > self.ceilings.stream() {
            return Err(Error::Protocol);
        }
        let token = self.token()?;
        let op = self.operation.as_mut().unwrap();
        if frame.offset != op.received {
            return Err(Error::Protocol);
        }
        op.received = op.received.checked_add(bytes.len() as u64).ok_or(Error::Protocol)?;
        if op.answer.is_some_and(|a| op.received > a.payload_len) {
            return Err(Error::Protocol);
        }
        op.crc.update(bytes);
        op.sinks.push_back((token, frame.offset, bytes.len()));
        self.actions.push_back(Action::WriteSink { token, offset: frame.offset, bytes: bytes.to_vec() });
        self.deadline = now.saturating_add(self.options.timeout_ms);
        Ok(())
    }

    fn sink_written(&mut self, token: u64, offset: u64, len: usize, now: u64) -> Result<(), Error> {
        if self.cancellation.is_some() || self.restoring {
            return Ok(());
        }
        let Some(op) = self.operation.as_mut() else {
            return Ok(());
        };
        let Some((expected, due, count)) = op.sinks.front().copied() else {
            return Ok(());
        };
        if token != expected {
            return if op.sinks.iter().any(|item| item.0 == token) { Err(Error::Protocol) } else { Ok(()) };
        }
        if offset != due || len != count {
            return Err(Error::Protocol);
        }
        op.sinks.pop_front();
        if !matches!(op.request, Request::Get(_))
            || offset != op.settled
            || len == 0
            || len as u64 > op.received - op.settled
        {
            return Err(Error::Protocol);
        }
        op.settled += len as u64;
        self.actions
            .push_back(Action::Progress { done: op.settled, total: op.answer.map_or(op.settled, |a| a.payload_len) });
        self.deadline = now.saturating_add(self.options.timeout_ms);
        self.finish_transfer()
    }

    fn finish_transfer(&mut self) -> Result<(), Error> {
        let Some(op) = self.operation.as_ref() else {
            return Ok(());
        };
        let Some(answer) = op.answer else {
            return Ok(());
        };
        if op.received > answer.payload_len || op.settled > answer.payload_len {
            return Err(Error::Protocol);
        }
        if op.settled != answer.payload_len {
            return Ok(());
        }
        if op.crc.finalize() != answer.payload_crc {
            return Err(Error::Checksum);
        }
        let outcome = if matches!(op.request, Request::Put(_)) { Outcome::Put(answer) } else { Outcome::Get(answer) };
        self.finish(Ok(outcome));
        Ok(())
    }

    fn finish(&mut self, result: Result<Outcome, Error>) {
        if let Err(error) = result {
            self.fail_queries(error);
            self.actions.clear();
        }
        if self.operation.as_ref().is_some_and(|op| matches!(op.request, Request::Get(_))) && result.is_err() {
            self.actions.push_back(Action::ResetSink);
        }
        self.pending = None;
        self.operation = None;
        self.write = None;
        self.cancellation = None;
        self.restoring = false;
        if result.is_err() {
            self.actions.push_back(Action::ResetChannels);
        }
        self.actions.push_back(Action::Complete(result));
    }

    fn action_pending(&self, token: u64) -> bool {
        self.write.is_some_and(|(expected, _)| expected == token)
            || self.operation.as_ref().is_some_and(|op| {
                op.source_due.is_some_and(|(expected, _, _)| expected == token)
                    || op.sinks.iter().any(|(expected, _, _)| *expected == token)
                    || op.stream_writes.iter().any(|write| write.token == token)
            })
    }
}
