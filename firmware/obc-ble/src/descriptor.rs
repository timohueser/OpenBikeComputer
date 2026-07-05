//! The control-plane descriptor codecs — small, typed, fixed-shape messages that ride GATT while
//! the CoC stays raw payload bytes:
//!
//! - [`TransferControl`]: the fixed **16-byte** descriptor to open / abort a transfer, or announce
//!   a download's size + CRC.
//! - [`StatusMessage`]: the device → app `status` notification envelope — a `u8` discriminator +
//!   fixed body.
//! - [`ObjectStoreDigest`]: the 10-byte "did anything change" read/notify value.
//! - [`Config`]: the whole-blob Config object that crosses GATT (not the CoC).
//!
//! Every layout mirrors the app's Swift codecs field-for-field. All integers little-endian.

/// Why a control-plane descriptor failed to decode. Mirrors the app's `DescriptorError` so a
/// firmware reject and an app reject classify the same wire byte the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescriptorError {
    /// The slice is shorter than the layout requires.
    Truncated,
    /// The `op` byte is not a known [`Op`].
    UnknownOp(u8),
    /// The `type` byte is not a known [`ObjectType`].
    UnknownType(u8),
    /// A status/discriminator byte is not a known value.
    UnknownStatus(u8),
}

/// The kind of object a bulk transfer carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ObjectType {
    Route = 1,
    Ride = 2,
    /// Reserved on the CoC — Config crosses GATT whole-blob.
    ConfigBlob = 3,
    Diagnostics = 4,
    /// Reserved (future OTA).
    Firmware = 5,
    RouteList = 6,
    RideList = 7,
    /// Dev/test loopback: the device streams back exactly what it received.
    Echo = 8,
}

impl ObjectType {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Decode a `type` byte, rejecting reserved/unknown ids.
    pub const fn from_u8(v: u8) -> Result<Self, DescriptorError> {
        Ok(match v {
            1 => Self::Route,
            2 => Self::Ride,
            3 => Self::ConfigBlob,
            4 => Self::Diagnostics,
            5 => Self::Firmware,
            6 => Self::RouteList,
            7 => Self::RideList,
            8 => Self::Echo,
            other => return Err(DescriptorError::UnknownType(other)),
        })
    }
}

/// The imperative a [`TransferControl`] carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    /// app → device: the app streams `object[offset…]` over the CoC.
    Upload = 1,
    /// device → app: the app requests, the device announces (`total_len`/`crc32`) then streams.
    Download = 2,
    /// Either side stops cleanly; the device drains and discards.
    Abort = 3,
}

impl Op {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    pub const fn from_u8(v: u8) -> Result<Self, DescriptorError> {
        Ok(match v {
            1 => Self::Upload,
            2 => Self::Download,
            3 => Self::Abort,
            other => return Err(DescriptorError::UnknownOp(other)),
        })
    }
}

/// The fixed **16-byte** transfer descriptor — one shape serves upload, download request/announce,
/// and abort, so the CoC needs no per-chunk header.
///
/// ```text
///   op         u8    1 = upload · 2 = download · 3 = abort
///   type       u8    ObjectType
///   object_id  u16   0xFFFF on upload = "new" (device assigns; see TransferResult)
///   total_len  u32   upload: full object size · download request / abort: 0
///   crc32      u32   upload: whole-object CRC-32/IEEE · download request / abort: 0
///   offset     u32   byte offset to start streaming from (0 = fresh) — the resume anchor
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferControl {
    pub op: Op,
    pub ty: ObjectType,
    pub object_id: u16,
    pub total_len: u32,
    pub crc32: u32,
    pub offset: u32,
}

impl TransferControl {
    pub const ENCODED_LEN: usize = 16;

    /// The `object_id` an upload sends to mean "new — the device assigns the id".
    pub const NEW_OBJECT_ID: u16 = 0xFFFF;

    pub fn encode(&self) -> [u8; Self::ENCODED_LEN] {
        let mut b = [0u8; Self::ENCODED_LEN];
        b[0] = self.op.as_u8();
        b[1] = self.ty.as_u8();
        b[2..4].copy_from_slice(&self.object_id.to_le_bytes());
        b[4..8].copy_from_slice(&self.total_len.to_le_bytes());
        b[8..12].copy_from_slice(&self.crc32.to_le_bytes());
        b[12..16].copy_from_slice(&self.offset.to_le_bytes());
        b
    }

    /// Decode a descriptor from a GATT write. Purely structural — semantic checks (e.g. an offset
    /// past `total_len`) belong to the transfer state machine, which answers them with a typed
    /// [`TransferResult`] rather than a bare ATT failure.
    pub fn decode(data: &[u8]) -> Result<Self, DescriptorError> {
        if data.len() < Self::ENCODED_LEN {
            return Err(DescriptorError::Truncated);
        }
        Ok(Self {
            op: Op::from_u8(data[0])?,
            ty: ObjectType::from_u8(data[1])?,
            object_id: u16::from_le_bytes([data[2], data[3]]),
            total_len: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
            crc32: u32::from_le_bytes([data[8], data[9], data[10], data[11]]),
            offset: u32::from_le_bytes([data[12], data[13], data[14], data[15]]),
        })
    }
}

/// The outcome of a transfer, reported in a [`TransferResult`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TransferStatus {
    /// Stored + CRC verified.
    Committed = 0,
    /// Rejected — nothing committed.
    CrcMismatch = 1,
    /// Cancelled by either side.
    Aborted = 2,
    /// Storage / internal failure.
    Error = 3,
    /// Unknown object type/id.
    NotFound = 4,
    /// A transfer is already active.
    Busy = 5,
    /// The route catalog is full — a new-route upload was rejected at descriptor-open time.
    StorageFull = 6,
}

impl TransferStatus {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    pub const fn from_u8(v: u8) -> Result<Self, DescriptorError> {
        Ok(match v {
            0 => Self::Committed,
            1 => Self::CrcMismatch,
            2 => Self::Aborted,
            3 => Self::Error,
            4 => Self::NotFound,
            5 => Self::Busy,
            6 => Self::StorageFull,
            other => return Err(DescriptorError::UnknownStatus(other)),
        })
    }

    /// The descriptor-open reject rule for a route **upload**, before any byte streams (issue #452).
    ///
    /// A *new* upload — id [`TransferControl::NEW_OBJECT_ID`] (`0xFFFF`) or a named id the device
    /// doesn't hold — grows the catalog, so it is refused when the store can't index another object
    /// (`catalog_full`: the route table is at `MAX_ROUTES` or the durable id space is exhausted):
    ///
    /// - new + full → [`StorageFull`](Self::StorageFull) — the phone tells the rider to free space.
    /// - named-but-unknown id with room to spare → [`NotFound`](Self::NotFound) — a real client error.
    /// - a *replace-by-id* of an existing route (`id_known`) reuses its slot → **exempt**; `None`
    ///   (proceed), even at the cap. Updating the actively-navigated route must never hit storage-full.
    ///
    /// `None` means "no reject at this stage" — the caller proceeds to arm the transfer.
    pub const fn upload_open_reject(object_id: u16, id_known: bool, catalog_full: bool) -> Option<Self> {
        let is_new = object_id == TransferControl::NEW_OBJECT_ID || !id_known;
        if !is_new {
            return None; // replace-by-id: exempt from the cap
        }
        if catalog_full {
            return Some(Self::StorageFull);
        }
        if object_id != TransferControl::NEW_OBJECT_ID {
            return Some(Self::NotFound); // named-but-unknown id, room to spare
        }
        None
    }
}

/// The closing result of a transfer (`msg = 1`). `committed_offset` is the durable byte count.
/// For a fresh upload (`object_id == 0xFFFF`) `object_id` carries the assigned id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferResult {
    pub object_id: u16,
    pub status: TransferStatus,
    pub committed_offset: u32,
}

impl TransferResult {
    /// Body length inside the `status` envelope (`msg` byte + 7).
    pub const ENCODED_LEN: usize = 8;

    pub fn new(object_id: u16, status: TransferStatus, committed_offset: u32) -> Self {
        Self { object_id, status, committed_offset }
    }
}

/// Which object store moved + its new revision (`msg = 2`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreChanged {
    pub ty: ObjectType,
    pub revision: u32,
}

/// The result of a `command` write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CommandStatus {
    Ok = 0,
    UnknownCommand = 1,
    NotFound = 2,
    Busy = 3,
    Error = 4,
}

impl CommandStatus {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    pub const fn from_u8(v: u8) -> Result<Self, DescriptorError> {
        Ok(match v {
            0 => Self::Ok,
            1 => Self::UnknownCommand,
            2 => Self::NotFound,
            3 => Self::Busy,
            4 => Self::Error,
            other => return Err(DescriptorError::UnknownStatus(other)),
        })
    }
}

/// The result notified after a `command` write (`msg = 3`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandResult {
    /// Echoes the command byte.
    pub command: u8,
    pub status: CommandStatus,
    /// Command-specific; 0 unless documented.
    pub detail: u8,
}

impl CommandResult {
    pub fn new(command: u8, status: CommandStatus) -> Self {
        Self { command, status, detail: 0 }
    }
}

/// One `status` characteristic notification: a `u8` discriminator + fixed body. The app **ignores
/// unknown discriminators** (forward compatibility), never failing the link over one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusMessage {
    /// `msg = 1`, 8 bytes.
    TransferResult(TransferResult),
    /// `msg = 2`, 6 bytes.
    StoreChanged(StoreChanged),
    /// `msg = 3`, 4 bytes.
    CommandResult(CommandResult),
}

impl StatusMessage {
    /// The longest encoded message (`transferResult`) — a notify buffer of this size fits any.
    pub const MAX_ENCODED_LEN: usize = TransferResult::ENCODED_LEN;

    /// Encode into a fixed buffer; the returned length is the slice to notify (`&buf[..len]`).
    pub fn encode(&self) -> ([u8; Self::MAX_ENCODED_LEN], usize) {
        let mut b = [0u8; Self::MAX_ENCODED_LEN];
        let len = match self {
            Self::TransferResult(r) => {
                b[0] = 1;
                b[1..3].copy_from_slice(&r.object_id.to_le_bytes());
                b[3] = r.status.as_u8();
                b[4..8].copy_from_slice(&r.committed_offset.to_le_bytes());
                8
            }
            Self::StoreChanged(s) => {
                b[0] = 2;
                b[1] = s.ty.as_u8();
                b[2..6].copy_from_slice(&s.revision.to_le_bytes());
                6
            }
            Self::CommandResult(c) => {
                b[0] = 3;
                b[1] = c.command;
                b[2] = c.status.as_u8();
                b[3] = c.detail;
                4
            }
        };
        (b, len)
    }

    /// Decode a `status` notification. Returns `Ok(None)` for an unknown discriminator (the app
    /// ignores those); `Err` only for a known discriminator whose body is malformed/truncated.
    pub fn decode(data: &[u8]) -> Result<Option<Self>, DescriptorError> {
        let Some(&msg) = data.first() else {
            return Err(DescriptorError::Truncated);
        };
        Ok(Some(match msg {
            1 => {
                if data.len() < TransferResult::ENCODED_LEN {
                    return Err(DescriptorError::Truncated);
                }
                Self::TransferResult(TransferResult {
                    object_id: u16::from_le_bytes([data[1], data[2]]),
                    status: TransferStatus::from_u8(data[3])?,
                    committed_offset: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
                })
            }
            2 => {
                if data.len() < 6 {
                    return Err(DescriptorError::Truncated);
                }
                Self::StoreChanged(StoreChanged {
                    ty: ObjectType::from_u8(data[1])?,
                    revision: u32::from_le_bytes([data[2], data[3], data[4], data[5]]),
                })
            }
            3 => {
                if data.len() < 4 {
                    return Err(DescriptorError::Truncated);
                }
                Self::CommandResult(CommandResult {
                    command: data[1],
                    status: CommandStatus::from_u8(data[2])?,
                    detail: data[3],
                })
            }
            _ => return Ok(None),
        }))
    }
}

/// The `objectStore` digest: the cheap "did anything change" signal that replaces polling the
/// CoC-sized lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ObjectStoreDigest {
    pub revision: u32,
    pub route_count: u16,
    pub ride_count: u16,
}

impl ObjectStoreDigest {
    pub const ENCODED_LEN: usize = 10;

    pub fn encode(&self) -> [u8; Self::ENCODED_LEN] {
        let mut b = [0u8; Self::ENCODED_LEN];
        b[0..4].copy_from_slice(&self.revision.to_le_bytes());
        b[4..6].copy_from_slice(&self.route_count.to_le_bytes());
        b[6..8].copy_from_slice(&self.ride_count.to_le_bytes());
        // b[8..10] = reserved, already 0.
        b
    }

    pub fn decode(data: &[u8]) -> Result<Self, DescriptorError> {
        if data.len() < Self::ENCODED_LEN {
            return Err(DescriptorError::Truncated);
        }
        Ok(Self {
            revision: u32::from_le_bytes([data[0], data[1], data[2], data[3]]),
            route_count: u16::from_le_bytes([data[4], data[5]]),
            ride_count: u16::from_le_bytes([data[6], data[7]]),
        })
    }
}

/// The Config object — the one object small enough to cross GATT whole-blob, not the CoC. Rename =
/// write Config with a changed `name`. Append-only: readers ignore unknown trailing bytes, absent
/// trailing fields mean "device default". Borrows `name` from the wire buffer, so decode is
/// alloc-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config<'a> {
    /// The device name, UTF-8, ≤ [`Config::MAX_NAME`] bytes.
    pub name: &'a [u8],
    /// `0 = metric · 1 = imperial`.
    pub units: u8,
}

impl<'a> Config<'a> {
    /// The name-length cap (matches the OBCR route-name cap).
    pub const MAX_NAME: usize = 48;
    pub const MAX_ENCODED: usize = 128;
    /// The smallest well-formed blob: `name_len` (2) + empty name + `units` (1).
    pub const MIN_ENCODED: usize = 3;

    /// Encode into `out` (must be ≥ `2 + name.len() + 1`), returning the written length. `None` if
    /// the name is over-long or the buffer is too small.
    pub fn encode(&self, out: &mut [u8]) -> Option<usize> {
        let len = 2 + self.name.len() + 1;
        if self.name.len() > Self::MAX_NAME || len > Self::MAX_ENCODED || out.len() < len {
            return None;
        }
        out[0..2].copy_from_slice(&(self.name.len() as u16).to_le_bytes());
        out[2..2 + self.name.len()].copy_from_slice(self.name);
        out[2 + self.name.len()] = self.units;
        Some(len)
    }

    /// Decode + validate a written Config blob: a `name_len` ≤ 48 that fits, whole blob in
    /// `[MIN_ENCODED, MAX_ENCODED]`. A trailing byte after `units` is tolerated (append-only rule).
    /// `None` = malformed (the board rejects it with an ATT error rather than silently storing it).
    pub fn decode(data: &'a [u8]) -> Option<Self> {
        if data.len() < Self::MIN_ENCODED || data.len() > Self::MAX_ENCODED {
            return None;
        }
        let name_len = u16::from_le_bytes([data[0], data[1]]) as usize;
        if name_len > Self::MAX_NAME || 2 + name_len + 1 > data.len() {
            return None;
        }
        Some(Self { name: &data[2..2 + name_len], units: data[2 + name_len] })
    }
}
