//! The store client's C boundary. Swift owns I/O and serializes calls on each handle. Returned
//! bytes and entries belong to the handle until its next `obc_client_next` call or close. Copy
//! them before an await. No device Host or card is needed by a transport client.

use obc_link::flat::{
    client::{Action, Client, Error, Event, Options, Outcome, QueryId, QueryOutcome},
    wire::*,
    ArchiveSource, Ceilings, Channel, DisplayName, EntryMeta, ObjectId, ObjectKind, Revision, StoreId,
};
use std::{ptr, slice};

#[cfg(test)]
mod tests;

#[repr(C)]
pub struct ObcClientRequest {
    pub opcode: u32,
    pub kind: u32,
    pub object_id: u64,
    pub revision: u64,
    pub length: u64,
    pub crc: u32,
    pub scoped: u32,
    pub store: [u8; 16],
    pub replacement: [u8; 16],
    pub name: *const u8,
    pub name_len: usize,
}

#[repr(C)]
pub struct ObcClientEntry {
    pub object_id: u64,
    pub revision: u64,
    pub length: u64,
    pub crc: u32,
    pub kind: u16,
    pub flags: u16,
    pub name_len: usize,
    pub name: [u8; 48],
}

#[repr(C)]
pub struct ObcClientResult {
    pub opcode: u32,
    pub error: u32,
    pub detail: u16,
    pub remote_code: u16,
    pub context: u64,
    pub store: [u8; 16],
    pub previous: [u8; 16],
    pub object_id: u64,
    pub revision: u64,
    pub length: u64,
    pub crc: u32,
    pub sequence: u64,
    pub timestamp: u32,
    pub state: u32,
    pub flag: u32,
    pub entries: *const ObcClientEntry,
    pub entry_count: usize,
}

impl Default for ObcClientResult {
    fn default() -> Self {
        Self {
            opcode: 0,
            error: 0,
            detail: 0,
            remote_code: 0,
            context: 0,
            store: [0; 16],
            previous: [0; 16],
            object_id: 0,
            revision: 0,
            length: 0,
            crc: 0,
            sequence: 0,
            timestamp: 0,
            state: 0,
            flag: 0,
            entries: ptr::null(),
            entry_count: 0,
        }
    }
}

#[repr(C)]
pub struct ObcClientAction {
    pub kind: u32,
    pub channel: u32,
    pub token: u64,
    pub offset: u64,
    pub length: usize,
    pub total: u64,
    pub bytes: *const u8,
    pub result: ObcClientResult,
}

impl Default for ObcClientAction {
    fn default() -> Self {
        Self {
            kind: 0,
            channel: 0,
            token: 0,
            offset: 0,
            length: 0,
            total: 0,
            bytes: ptr::null(),
            result: ObcClientResult::default(),
        }
    }
}

pub struct ObcClient {
    core: Client,
    bytes: Vec<u8>,
    entries: Vec<ObcClientEntry>,
}

fn failure(error: Error) -> ObcClientResult {
    let mut result = ObcClientResult::default();
    result.error = match error {
        Error::Busy => 1,
        Error::InvalidInput => 2,
        Error::Protocol => 3,
        Error::Checksum => 4,
        Error::Remote(refusal) => {
            result.remote_code = refusal.code as u16;
            result.detail = refusal.detail;
            result.context = refusal.context;
            5
        }
        Error::Timeout => 6,
        Error::Cancelled => 7,
        Error::Io => 8,
        Error::LinkLost => 9,
        Error::StoreChanged { previous, current } => {
            result.previous = previous.0;
            result.store = current.0;
            10
        }
        Error::CatalogChanged => 11,
        Error::NotCommitted => 12,
        Error::OutcomeUnknown => 13,
        Error::RequestIdsExhausted => 14,
    };
    result
}

unsafe fn bytes<'a>(pointer: *const u8, len: usize) -> Result<&'a [u8], Error> {
    if len == 0 {
        Ok(&[])
    } else if pointer.is_null() {
        Err(Error::InvalidInput)
    } else {
        Ok(slice::from_raw_parts(pointer, len))
    }
}

unsafe fn request(input: &ObcClientRequest) -> Result<Request, Error> {
    let id = ObjectId(input.object_id);
    let revision = Revision(input.revision);
    Ok(match input.opcode {
        1 => Request::List(ListRequest {
            kind: if input.kind == 0 {
                None
            } else {
                Some(
                    ObjectKind::decode(u16::try_from(input.kind).map_err(|_| Error::InvalidInput)?)
                        .ok_or(Error::InvalidInput)?,
                )
            },
            cursor: None,
        }),
        2 => Request::Status(StatusRequest { id, revision }),
        3 => Request::Get(GetRequest { id, revision }),
        4 => Request::Put(PutRequest {
            id,
            expected: revision,
            payload_len: input.length,
            payload_crc: input.crc,
            kind: ObjectKind::decode(u16::try_from(input.kind).map_err(|_| Error::InvalidInput)?)
                .ok_or(Error::InvalidInput)?,
            name: DisplayName::from_bytes(bytes(input.name, input.name_len)?).ok_or(Error::InvalidInput)?,
        }),
        5 => Request::Remove(RemoveRequest { id, expected: revision }),
        6 => Request::Cancel(CancelRequest {
            transfer: RequestId(u32::try_from(input.object_id).map_err(|_| Error::InvalidInput)?),
        }),
        7 => Request::Arm(ArmRequest { package: id, expected: revision }),
        8 => Request::Format(FormatRequest { expected: StoreId(input.store), replacement: StoreId(input.replacement) }),
        9 => Request::ArchiveRide(ArchiveSource {
            store: StoreId(input.store),
            id,
            revision,
            payload_len: input.length,
            payload_crc: input.crc,
        }),
        _ => return Err(Error::InvalidInput),
    })
}

impl ObcClient {
    fn catalog(&mut self, result: &mut ObcClientResult, store: StoreId, sequence: u64, entries: Vec<EntryMeta>) {
        result.opcode = 1;
        result.store = store.0;
        result.sequence = sequence;
        self.entries = entries
            .into_iter()
            .map(|entry| {
                let text = entry.name.as_bytes();
                let mut name = [0; 48];
                name[..text.len()].copy_from_slice(text);
                ObcClientEntry {
                    object_id: entry.id.0,
                    revision: entry.revision.0,
                    length: entry.payload_len,
                    crc: entry.payload_crc,
                    kind: entry.kind as u16,
                    flags: entry.flags.bits(),
                    name_len: text.len(),
                    name,
                }
            })
            .collect();
        result.entries = self.entries.as_ptr();
        result.entry_count = self.entries.len();
    }

    fn outcome(&mut self, outcome: Result<Outcome, Error>) -> ObcClientResult {
        let mut result = ObcClientResult::default();
        match outcome {
            Err(error) => return failure(error),
            Ok(Outcome::Catalog { store, sequence, entries }) => self.catalog(&mut result, store, sequence, entries),
            Ok(Outcome::Status(status)) => status_result(&mut result, status),
            Ok(Outcome::Get(transfer)) => transfer_result(&mut result, 3, transfer),
            Ok(Outcome::Put(transfer)) => transfer_result(&mut result, 4, transfer),
            Ok(Outcome::Remove { sequence }) => {
                result.opcode = 5;
                result.flag = u32::from(sequence.is_some());
                result.sequence = sequence.unwrap_or(0);
            }
            Ok(Outcome::Cancel(value)) => {
                result.opcode = 6;
                result.flag = u32::from(value);
            }
            Ok(Outcome::Arm { reserve, sequence }) => {
                result.opcode = 7;
                result.object_id = reserve.0;
                result.sequence = sequence;
            }
            Ok(Outcome::Format(store)) => {
                result.opcode = 8;
                result.store = store.0;
            }
            Ok(Outcome::ArchiveRide(archive)) => {
                result.opcode = 9;
                result.sequence = archive.sequence;
                result.timestamp = archive.timestamp;
            }
        }
        result
    }
}

fn status_result(result: &mut ObcClientResult, status: StatusResponse) {
    result.opcode = 2;
    result.state = status.state as u32;
    result.revision = status.revision.0;
    result.length = status.payload_len;
    result.crc = status.payload_crc;
}
fn transfer_result(result: &mut ObcClientResult, opcode: u32, transfer: TransferResponse) {
    result.opcode = opcode;
    result.object_id = transfer.id.0;
    result.revision = transfer.revision.0;
    result.length = transfer.payload_len;
    result.crc = transfer.payload_crc;
}

/// Create a transport client without an ordinary operation deadline.
#[no_mangle]
pub extern "C" fn obc_client_open(control: usize, stream: usize) -> *mut ObcClient {
    let Some(ceilings) = Ceilings::new(control, stream) else {
        return ptr::null_mut();
    };
    let mut core = Client::new(ceilings, Options { remove_duplicate_creates: true, ..Options::default() });
    core.set_timeout_ms(None).unwrap();
    Box::into_raw(Box::new(ObcClient { core, bytes: Vec::new(), entries: Vec::new() }))
}

/// # Safety
/// Handle is NULL or owned, live and not in use by another call.
#[no_mangle]
pub unsafe extern "C" fn obc_client_close(handle: *mut ObcClient) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// # Safety
/// Handle and request are live, exclusive for this call; name bytes are readable for name_len.
#[no_mangle]
pub unsafe extern "C" fn obc_client_start(
    handle: *mut ObcClient,
    input: *const ObcClientRequest,
    query: bool,
    timeout: u64,
    bounded: bool,
    now: u64,
) -> ObcClientResult {
    let (Some(host), Some(input)) = (handle.as_mut(), input.as_ref()) else {
        return failure(Error::InvalidInput);
    };
    let result = (|| {
        host.core.set_timeout_ms(bounded.then_some(timeout))?;
        let request = request(input)?;
        let scope = (input.scoped != 0).then_some(StoreId(input.store));
        if query {
            host.core.query(request, scope, now).map(|id| id.0)
        } else {
            host.core.start_scoped(request, scope, now).map(|_| 0)
        }
    })();
    match result {
        Ok(id) => ObcClientResult { context: id, ..ObcClientResult::default() },
        Err(error) => failure(error),
    }
}

/// # Safety
/// Handle is live/exclusive and bytes are readable for length. Zero length permits NULL.
#[no_mangle]
pub unsafe extern "C" fn obc_client_event(
    handle: *mut ObcClient,
    kind: u32,
    token: u64,
    offset: u64,
    data: *const u8,
    length: usize,
    now: u64,
) -> ObcClientResult {
    let Some(host) = handle.as_mut() else {
        return failure(Error::InvalidInput);
    };
    let data = if matches!(kind, 1..=3) {
        match bytes(data, length) {
            Ok(data) => data,
            Err(error) => return failure(error),
        }
    } else {
        &[]
    };
    let event = match kind {
        1 => Event::Control(data),
        2 => Event::Stream(data),
        3 => Event::Source { token, offset, bytes: data },
        4 => Event::Written(token),
        5 => Event::SinkWritten { token, offset, len: length },
        6 => Event::Tick,
        7 => Event::Cancel,
        8 => Event::IoFailed(token),
        9 => Event::LinkLost,
        10 => {
            let Ok(control) = usize::try_from(offset) else { return failure(Error::InvalidInput) };
            let Some(ceilings) = Ceilings::new(control, length) else { return failure(Error::InvalidInput) };
            Event::Restored(ceilings)
        }
        _ => return failure(Error::InvalidInput),
    };
    host.core.event(event, now);
    ObcClientResult::default()
}

/// Cancel only this local metadata query; transfer cancellation uses Event::Cancel.
/// # Safety
/// Handle is live and exclusive.
#[no_mangle]
pub unsafe extern "C" fn obc_client_cancel_query(handle: *mut ObcClient, id: u64) -> bool {
    handle.as_mut().is_some_and(|host| host.core.cancel_query(QueryId(id)))
}

/// # Safety
/// Handle is live/exclusive. Returned pointers outlive the call until next/close.
#[no_mangle]
pub unsafe extern "C" fn obc_client_next(handle: *mut ObcClient) -> ObcClientAction {
    let Some(host) = handle.as_mut() else {
        return ObcClientAction::default();
    };
    host.bytes.clear();
    host.entries.clear();
    let mut action = ObcClientAction::default();
    if let Some(value) = host.core.next_action() {
        match value {
            Action::Send { token, channel, record } => {
                action.kind = 1;
                action.token = token;
                action.channel = match channel {
                    Channel::Control => 0,
                    Channel::Stream => 1,
                };
                host.bytes = record;
            }
            Action::ReadSource { token, offset, max_len } => {
                action.kind = 2;
                action.token = token;
                action.offset = offset;
                action.length = max_len;
            }
            Action::WriteSink { token, offset, bytes } => {
                action.kind = 3;
                action.token = token;
                action.offset = offset;
                host.bytes = bytes;
            }
            Action::ResetSink => action.kind = 4,
            Action::Progress { done, total } => {
                action.kind = 5;
                action.offset = done;
                action.total = total;
            }
            Action::ResetChannels => action.kind = 6,
            Action::Restore => action.kind = 7,
            Action::Complete(result) => {
                action.kind = 8;
                action.result = host.outcome(result);
            }
        }
    } else if let Some((id, result)) = host.core.next_query_result() {
        action.kind = 9;
        action.token = id.0;
        match result {
            Err(error) => action.result = failure(error),
            Ok(QueryOutcome::Status(status)) => status_result(&mut action.result, status),
            Ok(QueryOutcome::Cancel(cancelled)) => {
                action.result.opcode = 6;
                action.result.flag = u32::from(cancelled);
            }
            Ok(QueryOutcome::Catalog { store, sequence, entries }) => {
                host.catalog(&mut action.result, store, sequence, entries);
            }
            Ok(QueryOutcome::Page { store, sequence, more, entries }) => {
                host.catalog(&mut action.result, store, sequence, entries);
                action.result.flag = u32::from(more);
            }
        }
    }
    if !host.bytes.is_empty() {
        action.bytes = host.bytes.as_ptr();
        action.length = host.bytes.len();
    }
    action
}

/// Input expectations: bit 0 control, bit 1 stream. No borrowed output is returned.
/// # Safety
/// Handle is live and exclusive.
#[no_mangle]
pub unsafe extern "C" fn obc_client_reads(handle: *const ObcClient) -> u32 {
    handle
        .as_ref()
        .map_or(0, |host| u32::from(host.core.awaiting_control()) | (u32::from(host.core.awaiting_stream()) << 1))
}

/// # Safety
/// Handle is live and output is writable for one u64.
#[no_mangle]
pub unsafe extern "C" fn obc_client_deadline(handle: *const ObcClient, output: *mut u64) -> bool {
    let (Some(host), Some(output)) = (handle.as_ref(), output.as_mut()) else {
        return false;
    };
    if let Some(deadline) = host.core.next_deadline_ms() {
        *output = deadline;
        true
    } else {
        false
    }
}

/// # Safety
/// Handle is live.
#[no_mangle]
pub unsafe extern "C" fn obc_client_transfer(handle: *const ObcClient) -> u32 {
    handle.as_ref().and_then(|host| host.core.active_transfer_id()).map_or(0, |id| id.0)
}

/// # Safety
/// Bytes are readable for length; zero length permits NULL.
#[no_mangle]
pub unsafe extern "C" fn obc_client_crc32(data: *const u8, length: usize) -> u32 {
    match bytes(data, length) {
        Ok(data) => {
            let mut crc = obc_crc::Crc32::new();
            crc.update(data);
            crc.finalize()
        }
        Err(_) => 0,
    }
}
