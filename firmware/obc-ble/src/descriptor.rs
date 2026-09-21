/// Why a control-plane descriptor failed to decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescriptorError {
    Truncated,
    UnknownOp(u8),
    UnknownType(u8),
    UnknownStatus(u8),
    /// A fixed-width field decoded correctly but names a value outside the command's contract.
    Bounds,
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
    /// A complete `UPDATE.BIN` OBCU container, app to device, upload only. The transfer layer sees
    /// opaque bytes; installing it is the separate, confirmed `installFw` command.
    FwImage = 5,
    RouteList = 6,
    RideList = 7,
    /// Dev/test loopback: the device streams back exactly what it received.
    Echo = 8,
    /// Metadata referencing route object ids in ride order. Trip ids come from a separate device
    /// counter, never shared with a route or ride id.
    Trip = 9,
    /// The trip catalog list, device to app: 76-byte entries mirroring `routeList`.
    TripList = 10,
    /// An `.obcm` map, host to device, upload only. A map is too large for BLE, so only the USB
    /// transport carries it. The transfer layer sees opaque bytes.
    Map = 16,
}

impl ObjectType {
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    pub const fn from_u8(v: u8) -> Result<Self, DescriptorError> {
        Ok(match v {
            1 => Self::Route,
            2 => Self::Ride,
            3 => Self::ConfigBlob,
            4 => Self::Diagnostics,
            5 => Self::FwImage,
            6 => Self::RouteList,
            7 => Self::RideList,
            8 => Self::Echo,
            9 => Self::Trip,
            10 => Self::TripList,
            // 11-15 stay reserved and keep rejecting.
            16 => Self::Map,

            other => return Err(DescriptorError::UnknownType(other)),
        })
    }

    /// No firmware caller: the iOS client and the vector oracles read this shared wire vocabulary.
    pub const fn is_map_payload(self) -> bool {
        matches!(self, Self::Map)
    }
}

/// The imperative a [`TransferControl`] carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    /// App to device: the app streams the whole object over the CoC.
    Upload = 1,
    /// Device to app: the app requests, the device announces `total_len`/`crc32`, then streams.
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

/// The fixed 12-byte transfer descriptor. One shape serves upload, download request and announce,
/// and abort, so the CoC needs no per-chunk header.
///
/// ```text
///   op         u8    1 = upload · 2 = download · 3 = abort
///   type       u8    ObjectType
///   object_id  u16   0xFFFF on upload = "new" (device assigns; see TransferResult)
///   total_len  u32   upload / download announce: full object size · download request / abort: 0
///   crc32      u32   upload / download announce: whole-object CRC-32/IEEE · download request / abort: 0
/// ```
///
/// There is no offset field: transfers restart, they never resume. The app writes the descriptor to
/// open a transfer; `transferControl` is write-only, so a download announce — the same 12 bytes with
/// `total_len`/`crc32` filled — travels as a [`StatusMessage::DownloadAnnounce`] instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferControl {
    pub op: Op,
    pub ty: ObjectType,
    pub object_id: u16,
    pub total_len: u32,
    pub crc32: u32,
}

impl TransferControl {
    pub const ENCODED_LEN: usize = 12;

    /// The `object_id` an upload sends to mean "new — the device assigns the id".
    pub const NEW_OBJECT_ID: u16 = 0xFFFF;

    pub fn encode(&self) -> [u8; Self::ENCODED_LEN] {
        let mut b = [0u8; Self::ENCODED_LEN];
        b[0] = self.op.as_u8();
        b[1] = self.ty.as_u8();
        b[2..4].copy_from_slice(&self.object_id.to_le_bytes());
        b[4..8].copy_from_slice(&self.total_len.to_le_bytes());
        b[8..12].copy_from_slice(&self.crc32.to_le_bytes());
        b
    }

    /// Structural decode only. Semantic checks belong to the transfer state machine, which answers
    /// them with a typed [`TransferResult`] rather than a bare ATT failure.
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
    Aborted = 2,
    /// Storage / internal failure.
    Error = 3,
    /// Unknown object type/id.
    NotFound = 4,
    /// A transfer is already active.
    Busy = 5,
    /// A catalog is full: a new-object upload was rejected at descriptor-open time.
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

    /// The descriptor-open reject for a route or trip upload, before any byte streams. A new upload
    /// — id `0xFFFF`, or a named id the device does not hold — grows the catalog, so it is refused
    /// when the catalog is full. A replace of a known id reuses its slot and is always allowed, so
    /// updating the route in use can never hit storage-full. `None` means no reject at this stage.
    pub const fn upload_open_reject(object_id: u16, id_known: bool, catalog_full: bool) -> Option<Self> {
        let is_new = object_id == TransferControl::NEW_OBJECT_ID || !id_known;
        if !is_new {
            return None;
        }
        if catalog_full {
            return Some(Self::StorageFull);
        }
        if object_id != TransferControl::NEW_OBJECT_ID {
            return Some(Self::NotFound); // named-but-unknown id, room to spare
        }
        None
    }

    /// Refuse a `fwImage` upload larger than the update-slot ceiling, before any bytes stream.
    /// `total_len` is the whole OBCU container, so the caller passes a container-sized ceiling; the
    /// constant stays out of this crate so the wire codec never links the DFU crate.
    pub const fn fwimage_announce_reject(total_len: u32, max_len: u32) -> Option<Self> {
        if total_len > max_len {
            Some(Self::Error)
        } else {
            None
        }
    }

    /// Refuse a map upload before any byte streams, because a map that fails at byte 300,000,000
    /// has cost the rider minutes. A map is new-only: the device never replaces a stored map in
    /// place, so a named id gets `notFound`. `min_len` is the OBCM header length, `headroom` the
    /// card reserve that keeps a map from stranding the ride log. An unknown `free_bytes` allows
    /// the transfer. `None` = accept.
    pub const fn map_announce_reject(
        object_id: u16,
        total_len: u32,
        min_len: u32,
        free_bytes: Option<u64>,
        headroom: u64,
    ) -> Option<Self> {
        if object_id != TransferControl::NEW_OBJECT_ID {
            return Some(Self::NotFound);
        }
        if total_len < min_len {
            return Some(Self::Error);
        }
        if let Some(free) = free_bytes {
            if total_len as u64 + headroom > free {
                return Some(Self::StorageFull);
            }
        }
        None
    }
}

/// The closing result of a transfer (`msg = 1`). `committed_offset` is the durable byte count.
/// For a fresh upload, `object_id` carries the id the device assigned.
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

    /// `ackRides` reports its newly-flagged count in `detail`.
    pub fn with_detail(command: u8, status: CommandStatus, detail: u8) -> Self {
        Self { command, status, detail }
    }
}

/// `deleteObject`: `cmd u8 · type u8 · object_id u16 LE`.
pub const CMD_DELETE_OBJECT: u8 = 1;
/// `ackRides`: see [`AckRides`].
pub const CMD_ACK_RIDES: u8 = 2;
/// `installFw`: the `cmd` byte only. Asks the device to install the staged `/UPDATE.BIN`.
pub const CMD_INSTALL_FW: u8 = 3;
/// `forgetBond`: the `cmd` byte only. The device answers `commandResult(ok)`, then clears its side
/// of the bond, drops the link, and advertises for open pairing again. The gated `command`
/// characteristic needs the encrypted link, so only the bonded phone can send it.
pub const CMD_FORGET_BOND: u8 = 4;
pub const CMD_SET_CLOCK: u8 = 5;
pub const SET_CLOCK_MIN_UTC: u32 = 1_577_836_800;
/// The magnitude bound on a `setClock` UTC offset: 14 h, the real-world offset span. A write
/// outside it is rejected.
pub const SET_CLOCK_MAX_OFFSET_MIN: i16 = 14 * 60;

/// Map the device state at the BLE edge to the `installFw` `commandResult.status`; the precedence
/// is busy, then no stage, then invalid, then ok. The command never installs on its own: a physical
/// confirm on the device is always necessary.
pub const fn install_fw_reply(has_staged: bool, busy: bool, staged_invalid: bool) -> CommandStatus {
    if busy {
        CommandStatus::Busy
    } else if !has_staged {
        CommandStatus::NotFound
    } else if staged_invalid {
        CommandStatus::Error
    } else {
        CommandStatus::Ok
    }
}

/// The `ackRides` command: `cmd u8 · count u8 · count × object_id u16 LE`. The app lists the ride
/// ids it holds and the device flags each listed id it still stores as synced. The flag means
/// "downloaded at least once", so an id is never un-flagged. The command is idempotent and
/// order-free, thus a long list can be split across writes. Unknown ids are ignored.
///
/// Borrowed view over the id bytes; bytes past `count × 2` are ignored.
#[derive(Clone, Copy, Debug)]
pub struct AckRides<'a> {
    /// Exactly `count × 2` little-endian id bytes.
    ids: &'a [u8],
}

impl<'a> AckRides<'a> {
    pub const fn encoded_len(count: usize) -> usize {
        2 + count * 2
    }

    /// Decode a full `command` write, starting at the command byte.
    pub fn decode(data: &'a [u8]) -> Result<Self, DescriptorError> {
        let [cmd, count, rest @ ..] = data else {
            return Err(DescriptorError::Truncated);
        };
        if *cmd != CMD_ACK_RIDES {
            return Err(DescriptorError::UnknownOp(*cmd));
        }
        let n = *count as usize * 2;
        match rest.get(..n) {
            Some(ids) => Ok(Self { ids }),
            None => Err(DescriptorError::Truncated),
        }
    }

    pub fn count(&self) -> usize {
        self.ids.len() / 2
    }

    pub fn iter(&self) -> impl Iterator<Item = u16> + 'a {
        self.ids.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]]))
    }

    /// Returns the written length, or `None` for more than 255 ids or a too-small buffer. The
    /// firmware only decodes; this is here for the shared-vector tests and the app-side codec.
    pub fn encode(ids: &[u16], out: &mut [u8]) -> Option<usize> {
        if ids.len() > u8::MAX as usize || out.len() < Self::encoded_len(ids.len()) {
            return None;
        }
        out[0] = CMD_ACK_RIDES;
        out[1] = ids.len() as u8;
        for (i, id) in ids.iter().enumerate() {
            out[2 + i * 2..4 + i * 2].copy_from_slice(&id.to_le_bytes());
        }
        Some(Self::encoded_len(ids.len()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetClock {
    /// The phone's current UTC time, unix seconds.
    pub utc: u32,
    /// The phone's current local UTC offset in minutes (`+02:00` → `120`), DST already folded in.
    pub offset_min: i16,
}

impl SetClock {
    /// The wire length: `cmd u8 · utc u32 · offset_min i16`.
    pub const ENCODED_LEN: usize = 7;

    /// Decode a full `command` write, starting at the command byte. The write must be exactly 7
    /// bytes: `setClock` has no variable tail, so trailing bytes are malformed.
    pub fn decode(data: &[u8]) -> Result<Self, DescriptorError> {
        let bytes: [u8; Self::ENCODED_LEN] = data.try_into().map_err(|_| DescriptorError::Truncated)?;
        if bytes[0] != CMD_SET_CLOCK {
            return Err(DescriptorError::UnknownOp(bytes[0]));
        }
        let utc = u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        let offset_min = i16::from_le_bytes([bytes[5], bytes[6]]);
        // `unsigned_abs`, not `abs`: the field can decode to `i16::MIN`, whose `abs` overflows and
        // panics in a debug build. `unsigned_abs` gives 32768, over the bound, so the decode rejects.
        if utc < SET_CLOCK_MIN_UTC || offset_min.unsigned_abs() > SET_CLOCK_MAX_OFFSET_MIN as u16 {
            return Err(DescriptorError::Truncated);
        }
        Ok(Self { utc, offset_min })
    }

    /// Returns the written length, or `None` for a too-small buffer. The firmware only decodes;
    /// this is here for the shared-vector tests and the app-side codec.
    pub fn encode(utc: u32, offset_min: i16, out: &mut [u8]) -> Option<usize> {
        if out.len() < Self::ENCODED_LEN {
            return None;
        }
        out[0] = CMD_SET_CLOCK;
        out[1..5].copy_from_slice(&utc.to_le_bytes());
        out[5..7].copy_from_slice(&offset_min.to_le_bytes());
        Some(Self::ENCODED_LEN)
    }
}
/// One `status` characteristic notification: a `u8` discriminator and a fixed body. The app ignores
/// unknown discriminators and never fails the link over one. This is the only device-to-app control
/// channel, so every message shares one subscription and one ordering domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusMessage {
    /// `msg = 1`, 8 bytes.
    TransferResult(TransferResult),
    /// `msg = 2`, 6 bytes.
    StoreChanged(StoreChanged),
    /// `msg = 3`, 4 bytes.
    CommandResult(CommandResult),
    /// `msg = 4`, 13 bytes: the `msg` byte and the 12-byte [`TransferControl`] descriptor, with
    /// `op = Download` and `total_len`/`crc32` filled.
    DownloadAnnounce(TransferControl),
}

impl StatusMessage {
    /// A notify buffer of this size fits any message.
    pub const MAX_ENCODED_LEN: usize = 1 + TransferControl::ENCODED_LEN;

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
            Self::DownloadAnnounce(d) => {
                b[0] = 4;
                b[1..1 + TransferControl::ENCODED_LEN].copy_from_slice(&d.encode());
                1 + TransferControl::ENCODED_LEN
            }
        };
        (b, len)
    }

    /// `Ok(None)` for an unknown discriminator; `Err` only for a known one with a malformed body.
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
            4 => {
                if data.len() < 1 + TransferControl::ENCODED_LEN {
                    return Err(DescriptorError::Truncated);
                }
                Self::DownloadAnnounce(TransferControl::decode(&data[1..])?)
            }

            _ => return Ok(None),
        }))
    }
}

/// The `protocolVersion` characteristic read. Readable without encryption.
///
/// ```text
///   version      u16   the protocol version
///   store_epoch  u32   the device's store-epoch nonce    — absent on a store-less device
///   obcm_version u8    the OBCM version the reader reads — absent on older firmware
/// ```
///
/// The read is length-driven: 7 bytes is the full read, 6 leaves `obcm_version` `None`, and 2 is a
/// device with no mounted store. An absent trailing field decodes to `None`, never to `0`: epoch
/// `0` names a legal id era and OBCM `0` would refuse every real map.
///
/// The store epoch names the store's id era. It lives on the card, so a card swap transplants the
/// era and a card from a different device presents its own. The app scopes all id-keyed state to
/// `(device serial, store epoch)`. The caller supplies the epoch and the OBCM version, so this
/// crate links neither the store nor the format crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VersionRead {
    pub version: u16,
    pub store_epoch: u32,
    pub obcm_version: Option<u8>,
}

impl VersionRead {
    pub const ENCODED_LEN: usize = 7;
    pub const ENCODED_LEN_NO_OBCM: usize = 6;

    pub fn encode(&self) -> ([u8; Self::ENCODED_LEN], usize) {
        let mut b = [0u8; Self::ENCODED_LEN];
        b[0..2].copy_from_slice(&self.version.to_le_bytes());
        b[2..6].copy_from_slice(&self.store_epoch.to_le_bytes());
        let Some(obcm) = self.obcm_version else {
            return (b, Self::ENCODED_LEN_NO_OBCM);
        };
        b[6] = obcm;
        (b, Self::ENCODED_LEN)
    }

    /// A read without a store epoch is truncated.
    pub fn decode(data: &[u8]) -> Result<Self, DescriptorError> {
        if data.len() < Self::ENCODED_LEN_NO_OBCM {
            return Err(DescriptorError::Truncated);
        }
        Ok(Self {
            version: u16::from_le_bytes([data[0], data[1]]),
            store_epoch: u32::from_le_bytes([data[2], data[3], data[4], data[5]]),
            obcm_version: data.get(6).copied(),
        })
    }
}

/// The Config object, the one object small enough to cross GATT whole-blob instead of the CoC. A
/// rename is a Config write with a changed `name`. Append-only: readers ignore unknown trailing
/// bytes and an absent trailing field means the device default. `name` borrows the wire buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config<'a> {
    /// UTF-8, at most [`Config::MAX_NAME`] bytes.
    pub name: &'a [u8],
    /// `0 = metric · 1 = imperial`.
    pub units: u8,
}

impl<'a> Config<'a> {
    /// Matches the OBCR route-name cap.
    pub const MAX_NAME: usize = 48;
    pub const MAX_ENCODED: usize = 128;
    /// The smallest well-formed blob: `name_len` (2) + empty name + `units` (1).
    pub const MIN_ENCODED: usize = 3;

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
