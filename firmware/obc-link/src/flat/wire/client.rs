//! The client half of the shared control-record codec.

use super::*;
use crate::flat::EntryFlags;

pub const MAX_REQUEST_LEN: usize = HEADER_LEN + PUT_BODY_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseError {
    InvalidFrame,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferResponse {
    pub id: ObjectId,
    pub revision: Revision,
    pub payload_len: u64,
    pub payload_crc: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListPage<'a> {
    pub store: StoreId,
    pub sequence: u64,
    pub more: bool,
    entries: &'a [u8],
}

impl ListPage<'_> {
    pub fn entries(&self) -> impl Iterator<Item = EntryMeta> + '_ {
        // The complete page is validated before it can be constructed.
        self.entries.as_chunks::<LIST_ENTRY_LEN>().0.iter().map(|entry| decode_entry(entry).unwrap())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Response<'a> {
    List(ListPage<'a>),
    Status(StatusResponse),
    Get(TransferResponse),
    Put(TransferResponse),
    Remove(u64),
    Cancel(bool),
    Arm { reserve: ObjectId, sequence: u64 },
    Format(StoreId),
    ArchiveRide(ArchiveResult),
    Error(Refusal),
}

pub fn encode_request(out: &mut [u8], id: RequestId, request: Request) -> Option<usize> {
    if id.0 == 0 {
        return None;
    }
    let opcode = match request {
        Request::List(_) => Opcode::List,
        Request::Status(_) => Opcode::Status,
        Request::Get(_) => Opcode::Get,
        Request::Put(_) => Opcode::Put,
        Request::Remove(_) => Opcode::Remove,
        Request::Cancel(_) => Opcode::Cancel,
        Request::Arm(_) => Opcode::Arm,
        Request::Format(_) => Opcode::Format,
        Request::ArchiveRide(_) => Opcode::ArchiveRide,
    };
    let total = write_header(out, opcode, 0, opcode.request_body_len(), id)?;
    let body = &mut out[HEADER_LEN..total];
    body.fill(0);
    match request {
        Request::List(list) => {
            body[..2].copy_from_slice(&list.kind.map_or(0, ObjectKind::value).to_le_bytes());
            if let Some(cursor) = list.cursor {
                body[2..4].copy_from_slice(&1u16.to_le_bytes());
                body[8..16].copy_from_slice(&cursor.id.0.to_le_bytes());
                body[16..24].copy_from_slice(&cursor.revision.0.to_le_bytes());
                body[24..32].copy_from_slice(&cursor.sequence.to_le_bytes());
            }
        }
        Request::Status(r) => pair(body, r.id, r.revision),
        Request::Get(r) => pair(body, r.id, r.revision),
        Request::Put(r) => {
            pair(body, r.id, r.expected);
            body[16..24].copy_from_slice(&r.payload_len.to_le_bytes());
            body[24..28].copy_from_slice(&r.payload_crc.to_le_bytes());
            body[28..30].copy_from_slice(&r.kind.value().to_le_bytes());
            body[32] = r.name.len() as u8;
            body[36..84].copy_from_slice(r.name.padded());
        }
        Request::Remove(r) => pair(body, r.id, r.expected),
        Request::Cancel(r) => body.copy_from_slice(&r.transfer.0.to_le_bytes()),
        Request::Arm(r) => pair(body, r.package, r.expected),
        Request::Format(r) => {
            body[..16].copy_from_slice(&r.expected.0);
            body[16..].copy_from_slice(&r.replacement.0);
        }
        Request::ArchiveRide(r) => {
            body[..16].copy_from_slice(&r.store.0);
            pair(&mut body[16..], r.id, r.revision);
            body[32..40].copy_from_slice(&r.payload_len.to_le_bytes());
            body[40..44].copy_from_slice(&r.payload_crc.to_le_bytes());
        }
    }
    decode_request(&out[..total]).ok()?;
    Some(total)
}

fn pair(out: &mut [u8], id: ObjectId, revision: Revision) {
    out[..8].copy_from_slice(&id.0.to_le_bytes());
    out[8..16].copy_from_slice(&revision.0.to_le_bytes());
}

pub fn decode_response(record: &[u8]) -> Result<(Header, Response<'_>), ResponseError> {
    use ResponseError::{InvalidFrame, Unsupported};
    if record.len() < HEADER_LEN || record[..4] != MAGIC || u32_at(record, 12) == 0 {
        return Err(InvalidFrame);
    }
    if record[4] != WIRE_MAJOR {
        return Err(Unsupported);
    }
    let opcode = Opcode::decode(record[5]).ok_or(Unsupported)?;
    let bits = u16_at(record, 6);
    let body = &record[HEADER_LEN..];
    if u16_at(record, 10) != 0
        || body.len() != u16_at(record, 8) as usize
        || bits & flags::RESPONSE == 0
        || bits & !(flags::RESPONSE | flags::ERROR | flags::MORE) != 0
        || bits & flags::MORE != 0 && (opcode != Opcode::List || bits & flags::ERROR != 0)
    {
        return Err(InvalidFrame);
    }
    let header = Header { opcode, request: RequestId(u32_at(record, 12)) };
    if bits & flags::ERROR != 0 {
        return Ok((header, Response::Error(Refusal::decode(body).ok_or(InvalidFrame)?)));
    }
    let response = match opcode {
        Opcode::List => {
            if body.len() < LIST_PREFIX_LEN || !(body.len() - LIST_PREFIX_LEN).is_multiple_of(LIST_ENTRY_LEN) {
                return Err(InvalidFrame);
            }
            let entries = &body[LIST_PREFIX_LEN..];
            if entries.as_chunks::<LIST_ENTRY_LEN>().0.iter().any(|e| decode_entry(e).is_none())
                || bits & flags::MORE != 0 && entries.is_empty()
            {
                return Err(InvalidFrame);
            }
            Response::List(ListPage {
                store: StoreId(body[..16].try_into().unwrap()),
                sequence: u64_at(body, 16),
                more: bits & flags::MORE != 0,
                entries,
            })
        }
        Opcode::Status if body.len() == 24 && is_zero(&body[1..4]) => {
            let state = match body[0] {
                0 => ObjectState::Absent,
                1 => ObjectState::Committed,
                2 => ObjectState::Superseded,
                _ => return Err(InvalidFrame),
            };
            let value = StatusResponse {
                state,
                revision: Revision(u64_at(body, 4)),
                payload_len: u64_at(body, 12),
                payload_crc: u32_at(body, 20),
            };
            if state == ObjectState::Absent && value != StatusResponse::absent()
                || state != ObjectState::Absent && value.revision.0 == 0
            {
                return Err(InvalidFrame);
            }
            Response::Status(value)
        }
        Opcode::Get if body.len() == 24 && is_zero(&body[20..]) => Response::Get(TransferResponse {
            id: ObjectId::NONE,
            revision: Revision(u64_at(body, 0)),
            payload_len: u64_at(body, 8),
            payload_crc: u32_at(body, 16),
        }),
        Opcode::Put if body.len() == 32 && is_zero(&body[28..]) => Response::Put(TransferResponse {
            id: ObjectId(u64_at(body, 0)),
            revision: Revision(u64_at(body, 8)),
            payload_len: u64_at(body, 16),
            payload_crc: u32_at(body, 24),
        }),
        Opcode::Remove if body.len() == 8 => Response::Remove(u64_at(body, 0)),
        Opcode::Cancel if body.len() == 1 && body[0] <= 1 => Response::Cancel(body[0] == 0),
        Opcode::Arm if body.len() == 16 => {
            Response::Arm { reserve: ObjectId(u64_at(body, 0)), sequence: u64_at(body, 8) }
        }
        Opcode::Format if body.len() == 16 => Response::Format(StoreId(body.try_into().unwrap())),
        Opcode::ArchiveRide if body.len() == 16 && is_zero(&body[12..]) => {
            Response::ArchiveRide(ArchiveResult { sequence: u64_at(body, 0), timestamp: u32_at(body, 8) })
        }
        _ => return Err(InvalidFrame),
    };
    Ok((header, response))
}

fn decode_entry(bytes: &[u8]) -> Option<EntryMeta> {
    if !is_zero(&bytes[33..36]) || !is_zero(&bytes[84..88]) {
        return None;
    }
    let entry = EntryMeta {
        id: ObjectId(u64_at(bytes, 0)),
        revision: Revision(u64_at(bytes, 8)),
        payload_len: u64_at(bytes, 16),
        payload_crc: u32_at(bytes, 24),
        kind: ObjectKind::decode(u16_at(bytes, 28))?,
        flags: EntryFlags::decode(u16_at(bytes, 30))?,
        name: decode_name(bytes[32], &bytes[36..84]).ok()?,
    };
    (entry.id.0 != 0 && entry.revision.0 != 0).then_some(entry)
}
