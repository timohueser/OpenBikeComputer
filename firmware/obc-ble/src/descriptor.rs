//! The control-plane descriptor codecs — small, typed, fixed-shape messages that ride GATT while
//! the CoC stays raw payload bytes:
//!
//! - [`TransferControl`]: the fixed **12-byte** descriptor the app writes to open / abort a transfer
//!   (protocol v2: `transferControl` is **write-only** — a download's announce rides the `status`
//!   envelope as [`StatusMessage::DownloadAnnounce`], not a notify on this characteristic).
//! - [`StatusMessage`]: the device → app `status` notification envelope — a `u8` discriminator +
//!   fixed body. In v2 it is the **sole** device → app control channel, so the download announce
//!   (`msg = 4`) shares its one subscription / one ordering domain.
//! - [`VersionRead`]: the widened `protocolVersion` read — `version u16 · store_epoch u32` (§1).
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
    /// A firmware update image — a complete `UPDATE.BIN` OBCU container, app → device (upload only).
    /// The transfer layer stays format-blind: the payload is opaque bytes staged to `/UPDATE.BIN`
    /// (spec §7.6). Installing it is the separate, on-glass-confirmed `installFw` command.
    FwImage = 5,
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
            5 => Self::FwImage,
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
    /// app → device: the app streams the whole object over the CoC.
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

/// The fixed **12-byte** transfer descriptor — one shape serves upload, download request/announce,
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
/// **v2 drops the `offset` field** — transfers restart, never resume (§1 principle 4), so the byte
/// was permanently `0`. Its `NonZeroOffset` reject went with it. The descriptor is written by the
/// app to *open* a transfer; the device never notifies it (`transferControl` is write-only). A
/// download's announce — the same 12 bytes with `total_len`/`crc32` filled — travels as a
/// [`StatusMessage::DownloadAnnounce`] (`msg = 4`) instead, folding all device → app control traffic
/// onto the one `status` characteristic.
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

    /// Decode a descriptor from a GATT write. Purely structural — semantic checks belong to the
    /// transfer state machine, which answers them with a typed [`TransferResult`] rather than a bare
    /// ATT failure.
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

    /// The announce-time reject for a `fwImage` upload (spec §4.2 / §7.6): an announced object
    /// larger than the device's update-slot ceiling `max_len` is refused at the `transferControl`
    /// write with [`Error`](Self::Error), **before any bytes stream** — a ~900 KB update would
    /// otherwise transfer only to fail at commit. `None` = accept (the caller arms the
    /// [`Receiver`](crate::Receiver)). `total_len` is the whole OBCU container (64-byte header +
    /// raw image), so the board passes the **container-sized** ceiling
    /// `obc_dfu::MAX_IMAGE_LEN + HEADER_LEN` — the raw-image cap plus the header (DR5, #733); the
    /// constants stay out of this crate so the wire codec never links the DFU crate.
    pub const fn fwimage_announce_reject(total_len: u32, max_len: u32) -> Option<Self> {
        if total_len > max_len {
            Some(Self::Error)
        } else {
            None
        }
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

    /// A result whose `detail` byte carries a documented, command-specific value (`ackRides`
    /// reports its newly-flagged count here).
    pub fn with_detail(command: u8, status: CommandStatus, detail: u8) -> Self {
        Self { command, status, detail }
    }
}

/// `command` byte: `deleteObject` (§4.4, cmd 1) — `type u8 · object_id u16 LE`.
pub const CMD_DELETE_OBJECT: u8 = 1;
/// `command` byte: `ackRides` (§4.4, cmd 2) — see [`AckRides`].
pub const CMD_ACK_RIDES: u8 = 2;
/// `command` byte: `installFw` (§4.4, cmd 3) — no args (the `cmd` byte only). Asks the device to
/// install the staged `/UPDATE.BIN`; see [`install_fw_reply`].
pub const CMD_INSTALL_FW: u8 = 3;
/// `command` byte: `forgetBond` (§4.4, cmd 4) — no args (the `cmd` byte only). Asks the device to
/// dissolve **its** side of the bond, so an app-side "Forget device" doesn't leave the pair wedged
/// (the device would otherwise keep rejecting new pairings until the rider ran Forget phone on the
/// device — §8). Honoured **only over the authenticated, bonded link**: the gated `command`
/// characteristic already requires the LESC-encrypted link (§8), so a stranger can never issue it —
/// the bonded phone asking to clear its own bond is fully consistent with the reject-when-bonded
/// posture. The device answers `commandResult(ok)` first, then clears the bond + drops the link and
/// returns to open-pairing advertising.
pub const CMD_FORGET_BOND: u8 = 4;

/// Map the cheaply-knowable device state at the BLE edge to the `installFw` `commandResult.status`
/// (§4.4 cmd 3). The four documented outcomes reuse the existing status vocabulary — **no new status
/// byte** — with precedence **`busy` > `noStaged` > `invalid` > `ok`**:
///
/// - `busy` → [`Busy`](CommandStatus::Busy): a ride is recording, or an install request is already
///   pending.
/// - `noStaged` → [`NotFound`](CommandStatus::NotFound): no `UPDATE.BIN` on the card (a cheap
///   card-root existence check).
/// - `invalid` → [`Error`](CommandStatus::Error): the device can *cheaply* tell the stage is
///   unusable. The reference firmware never runs the multi-second CRC scan inside the command
///   handler, so it always passes `staged_invalid = false` and lets the on-device confirm flow
///   surface a bad image; this arm exists for a device that can reject a stage cheaply.
/// - else → [`Ok`](CommandStatus::Ok): the request is accepted and the on-glass confirm card will
///   show. The command **never installs on its own** — a physical confirm is always required.
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

/// The `ackRides` command (§4.4, cmd `2`): `cmd u8 · count u8 · count × object_id u16 LE` — the
/// phone's **possession ack** for stored rides.
///
/// The device's per-ride "synced" flag is otherwise inferred from one event (a ride download
/// completing), so any divergence between the phone's library and the device's sidecar — rides
/// downloaded before the sidecar existed, a sidecar lost with a reflashed card, an app reinstall —
/// was permanent. This command makes the phone's library the ground truth: on connect (and whenever
/// it likes) the app lists the ride ids it holds, and the device flags every listed id it still
/// stores as synced. **Monotonic** — ids the phone lost are never un-flagged (the flag means
/// "downloaded at least once", not "still held") — and **idempotent and order-free**, so a list
/// longer than one GATT write is simply split across writes. Unknown ids are ignored (`ok` either
/// way): the phone may legitimately hold rides the device has since deleted.
///
/// Borrowed view over the id bytes (alloc-free, like [`Config`]); trailing bytes past
/// `count × 2` are ignored.
#[derive(Clone, Copy, Debug)]
pub struct AckRides<'a> {
    /// Exactly `count × 2` little-endian id bytes.
    ids: &'a [u8],
}

impl<'a> AckRides<'a> {
    /// The encoded length of an ack carrying `count` ids.
    pub const fn encoded_len(count: usize) -> usize {
        2 + count * 2
    }

    /// Decode a full `command` write (starting at the command byte). Errors: not `ackRides`
    /// ([`DescriptorError::UnknownOp`]) or fewer id bytes than `count` promises
    /// ([`DescriptorError::Truncated`]).
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

    /// How many ride ids this ack carries.
    pub fn count(&self) -> usize {
        self.ids.len() / 2
    }

    /// The acked ride ids, in write order.
    pub fn iter(&self) -> impl Iterator<Item = u16> + 'a {
        self.ids.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]))
    }

    /// Encode `ids` into `out` (must be ≥ [`encoded_len`](Self::encoded_len)); returns the written
    /// length, or `None` for more than 255 ids or a too-small buffer. The app side encodes (its
    /// Swift codec mirrors this); the firmware only decodes — this exists for the shared-vector
    /// and round-trip tests.
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

/// One `status` characteristic notification: a `u8` discriminator + fixed body. The app **ignores
/// unknown discriminators** (forward compatibility), never failing the link over one. In protocol
/// v2 this is the **sole** device → app control channel, so every message — including a download's
/// announce (`msg = 4`) — shares its one subscription and one ordering domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusMessage {
    /// `msg = 1`, 8 bytes.
    TransferResult(TransferResult),
    /// `msg = 2`, 6 bytes.
    StoreChanged(StoreChanged),
    /// `msg = 3`, 4 bytes.
    CommandResult(CommandResult),
    /// `msg = 4`, 13 bytes: the download announce — the `msg` byte followed by the 12-byte
    /// [`TransferControl`] descriptor (`op = Download`, `total_len`/`crc32` filled). v2 folds the
    /// announce off `transferControl` and onto this envelope so all device → app control traffic is
    /// one notify characteristic.
    DownloadAnnounce(TransferControl),
}

impl StatusMessage {
    /// The longest encoded message (`downloadAnnounce`: `msg` byte + the 12-byte descriptor) — a
    /// notify buffer of this size fits any.
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

/// The `protocolVersion` characteristic read (widened for v2, epic #632 item 5): the wire version
/// **and** the device's current **store epoch** — a `u32` TRNG nonce that changes on an id-era reset
/// (full-chip reflash, factory reset, a torn id-marks line, a fresh card). The app reads it first on
/// every connect, before any reconcile, so it knows the era before it acks or links anything; the
/// epoch scopes all id-keyed app state to `(device serial, store epoch)` so a reset can't silently
/// alias months-old ids. The mint rule lives on the device (V3); a random nonce leaks nothing beyond
/// open DIS. Readable **without** encryption.
///
/// ```text
///   version      u16   the protocol version (currently 2)
///   store_epoch  u32   the device's current store-epoch nonce
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VersionRead {
    pub version: u16,
    pub store_epoch: u32,
}

impl VersionRead {
    pub const ENCODED_LEN: usize = 6;

    pub fn encode(&self) -> [u8; Self::ENCODED_LEN] {
        let mut b = [0u8; Self::ENCODED_LEN];
        b[0..2].copy_from_slice(&self.version.to_le_bytes());
        b[2..6].copy_from_slice(&self.store_epoch.to_le_bytes());
        b
    }

    pub fn decode(data: &[u8]) -> Result<Self, DescriptorError> {
        if data.len() < Self::ENCODED_LEN {
            return Err(DescriptorError::Truncated);
        }
        Ok(Self {
            version: u16::from_le_bytes([data[0], data[1]]),
            store_epoch: u32::from_le_bytes([data[2], data[3], data[4], data[5]]),
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
