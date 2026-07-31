//! The control-plane descriptor codecs — small, typed, fixed-shape messages that ride GATT while
//! the CoC stays raw payload bytes:
//!
//! - [`TransferControl`]: the fixed **12-byte** descriptor the app writes to open / abort a transfer
//!   (protocol v2: `transferControl` is **write-only** — a download's announce rides the `status`
//!   envelope as [`StatusMessage::DownloadAnnounce`], not a notify on this characteristic).
//! - [`StatusMessage`]: the device → app `status` notification envelope — a `u8` discriminator +
//!   fixed body. In v2 it is the **sole** device → app control channel, so the download announce
//!   (`msg = 4`) shares its one subscription / one ordering domain.
//! - [`VersionRead`]: the widened `protocolVersion` read — `version u16 · store_epoch u32 ·
//!   obcm_version u8` (§1), a **length-driven** read served at 7, 6 or 2 bytes.
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
    /// A trip object — a tiny metadata object referencing route object ids in ride order (spec §7.7),
    /// app → device (upload) and device → app (detail read). Trip ids draw from a **separate** device
    /// counter (§4.1), never shared with a route or ride id.
    Trip = 9,
    /// The trip catalog list object (device → app), spec §7.4 — 76-byte entries mirroring `routeList`.
    TripList = 10,
    /// An `.obcm` map (host → device, upload only), introduced by the USB transport (#889).
    ///
    /// **Why this type exists only now:** a map is hundreds of megabytes, so it was never uploadable
    /// over BLE and the type would have been dead weight. A USB bulk endpoint makes it possible, so
    /// USB is what introduces it.
    ///
    /// **Why 16 and not 11:** `11`–`15` are reserved in the spec for the sensor work (M4), and
    /// stepping into a reserved band to save five discriminants would trade a real future collision
    /// for nothing — the byte is a `u8` with 240 values still free. `16` opens the band for
    /// transport-introduced types that BLE could never have carried.
    ///
    /// The transfer layer stays format-blind, as it is for `FwImage`: the payload is opaque bytes.
    Map = 16,
    /// One OBCM **shard** of a volume set (`OBCA_Spec.md` §5.1), host → device, upload only —
    /// **USB only**, for the same reason `Map` is: a set is strictly *larger* than the single map
    /// BLE could never carry.
    ///
    /// A shard is an ordinary OBCM file, so the streaming, the whole-object CRC and the held-back
    /// magic are `Map`'s unchanged. What differs is the one thing the descriptor has to say and
    /// `Map` never needed: **which** file of the set this is. That rides `object_id` as a
    /// [`SetPart`] — not an object id, because a shard has none (§5.2 derives every filename from
    /// the set id and the index, and a set is *one map* in every interface, §5.4).
    MapShard = 17,
    /// The OBCS **set manifest** (`OBCA_Spec.md` §5.2), host → device, upload only — **USB only**.
    ///
    /// New-only like `Map`, so `object_id` is `0xFFFF`. It is the set's atomicity token: §5.4 makes
    /// it the file that must be written **last**, and the device *enforces* that rather than
    /// trusting the order it arrives in — a manifest announced before every shard it will name has
    /// committed is refused before a byte streams.
    MapSet = 18,
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
            9 => Self::Trip,
            10 => Self::TripList,
            // 11–15 stay reserved (sensors, M4) and keep rejecting.
            16 => Self::Map,
            17 => Self::MapShard,
            18 => Self::MapSet,
            other => return Err(DescriptorError::UnknownType(other)),
        })
    }

    /// Whether this type is part of a map upload — the three types the reference firmware streams
    /// straight into their final file with the format magic held back, rather than through the
    /// invisible `UPLOAD.TMP` every small object uses. Also the three types that are **USB only**
    /// (spec §10).
    pub const fn is_map_payload(self) -> bool {
        matches!(self, Self::Map | Self::MapShard | Self::MapSet)
    }
}

/// What a [`ObjectType::MapShard`] descriptor's `object_id` field carries: **which file of the set
/// this is**, as `shard_count` in the high byte and `index` in the low one.
///
/// The field is repurposed rather than added to because the descriptor is a fixed 12 bytes shared
/// by every transport and every object type (§4.2), and a shard has no object id to put there
/// anyway: `OBCA_Spec.md` §5.2 *derives* every filename from the set id and the index, and §5.4
/// makes the whole set one map with one identity. Widening the descriptor for one type would repin
/// every codec, every fixture and both companion apps for a field the other ten types would write
/// zero into.
///
/// Carrying `shard_count` in **every** shard announce, not just the first, buys two things the
/// index alone could not:
///
/// - The device can refuse a set past its own shard ceiling at the **first** announce, before the
///   rider spends minutes uploading shards it will never mount. The alternative — discovering the
///   count when the manifest arrives — refuses after the whole set has moved.
/// - Every announce re-states the set it belongs to, so a host that switches sets mid-transfer is
///   a mismatch the device names, not a set silently assembled out of two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetPart {
    /// How many shards the set has in total (`1..=32`, `OBCA_Spec.md` §5.2).
    pub shard_count: u8,
    /// This shard's index within the set, `< shard_count`.
    pub index: u8,
}

impl SetPart {
    /// The `object_id` bytes for this part.
    pub const fn encode(&self) -> u16 {
        ((self.shard_count as u16) << 8) | self.index as u16
    }

    /// Decode a `mapShard` descriptor's `object_id`, rejecting every pair that cannot name a file
    /// of a set: a zero shard count, and an index at or past it.
    ///
    /// Deliberately does **not** apply the `1..=32` spec cap or a device's own (smaller) shard
    /// ceiling — those are a policy the caller owns and answers with a typed status, exactly as
    /// `fwimage_announce_reject` takes its ceiling rather than linking the DFU crate.
    pub const fn decode(object_id: u16) -> Option<SetPart> {
        let shard_count = (object_id >> 8) as u8;
        let index = (object_id & 0xFF) as u8;
        if shard_count == 0 || index >= shard_count {
            return None;
        }
        Some(SetPart { shard_count, index })
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
    /// A catalog is full — a new-object upload (a route past its cap, a trip past its cap) was
    /// rejected at descriptor-open time.
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

    /// The descriptor-open reject rule for a route **or trip upload**, before any byte streams
    /// (issue #452; extended to trips in epic #526, TR4 #653 — the rule is type-agnostic, the caller
    /// passes the relevant catalog's `catalog_full`/`id_known`).
    ///
    /// A *new* upload — id [`TransferControl::NEW_OBJECT_ID`] (`0xFFFF`) or a named id the device
    /// doesn't hold — grows the catalog, so it is refused when the store can't index another object
    /// (`catalog_full`: the route table is at `MAX_ROUTES` / the trip table at `MAX_TRIPS`, or the
    /// durable id space is exhausted):
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

    /// The announce-time reject for a **map** upload (spec §4.2 / §10; issue #927) — the rule that
    /// keeps a several-hundred-megabyte transfer from starting when it cannot possibly land.
    ///
    /// Three refusals, all **before any byte streams**, because a map that fails at byte
    /// 300,000,000 has cost the rider minutes and the card a wasted write:
    ///
    /// - **Not new** (`object_id != 0xFFFF`) → [`NotFound`](Self::NotFound). A map upload is
    ///   *new-only*: the device never replaces a stored map in place. A replace would have to
    ///   destroy the old map's bytes as the new ones stream (the file is far too large to stage a
    ///   second copy and swap), which breaks §4.2's "a failed CRC never touches the old copy"
    ///   guarantee on the one object the device cannot re-derive. So every named id is, for a map,
    ///   an id this device will not write to — which is exactly `notFound`. Replacing a map is
    ///   "upload the new one, then delete the old one".
    /// - **Too short** (`total_len < min_len`) → [`Error`](Self::Error). `min_len` is the OBCM
    ///   header length; the constant stays out of this crate so the wire codec never links the
    ///   format crate (the `fwimage_announce_reject` convention).
    /// - **Won't fit** → [`StorageFull`](Self::StorageFull), when `free_bytes` is known and
    ///   `total_len + headroom` exceeds it. `headroom` is the device's reserve so a map can never
    ///   fill the card to the last cluster and strand the ride log. `free_bytes = None` means the
    ///   device could not measure free space (a non-FAT32 card, an FSInfo with no cached count):
    ///   the transfer is **allowed**, because refusing every upload on a card whose free count is
    ///   merely unreadable would be worse than failing late on the rare card that is genuinely full.
    ///
    /// `None` = accept (the caller arms the [`Receiver`](crate::Receiver)).
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
/// `command` byte: `setClock` (§4.4, cmd 5) — `utc u32 LE · offset_min i16 LE`; see [`SetClock`].
/// Auto-expiry epic #638 S2 (#642): the phone stamps the device's UTC clock + local offset on every
/// connect, the second trusted clock source after GPS. The epic's draft table numbered it `3`; that
/// predates `installFw`/`forgetBond` taking `3`/`4`, so it lands at `5` (the next-free command, §4.4).
pub const CMD_SET_CLOCK: u8 = 5;
/// `command` byte: `setRouteRetention` (§4.4, cmd 6) — `object_id u16 LE · retention u8`; see
/// [`SetRouteRetention`]. Auto-expiry epic #638 S4 (#644): the phone sets a stored route's retention
/// level without re-uploading it — right after an upload's `transferResult` commits (the result
/// carries the assigned id) and on any user retention edit. The epic's draft table numbered it `4`;
/// that predates `forgetBond`/`setClock` taking `4`/`5`, so it lands at `6` (the next-free command).
pub const CMD_SET_ROUTE_RETENTION: u8 = 6;

/// The largest valid `setRouteRetention` retention byte: `5` (2 months). A write above it is an
/// out-of-range level (§4.4), rejected `error` — decoded here, mirrored by the iOS codec, and pinned
/// by the `command-set-route-retention.bin` vector.
pub const SET_ROUTE_RETENTION_MAX: u8 = 5;

/// The earliest UTC a `setClock` will accept: `2020-01-01T00:00:00Z` (unix `1577836800`). An earlier
/// stamp is an obviously-bogus phone clock (§4.4) and is rejected `error`, so it can never seed a
/// stale set-point that the auto-expiry sweep (#638) would then treat as trusted.
pub const SET_CLOCK_MIN_UTC: u32 = 1_577_836_800;
/// The magnitude bound on a `setClock` UTC offset: ±14 h (±840 min), the real-world offset span
/// (−12:00 Baker Island … +14:00 Kiribati). A write outside it is rejected `error` (§4.4).
pub const SET_CLOCK_MAX_OFFSET_MIN: i16 = 14 * 60;

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

/// The `setClock` command (§4.4, cmd `5`): `cmd u8 = 5 · utc u32 LE · offset_min i16 LE` — the
/// phone stamps the device's wall clock on every connect (auto-expiry epic #638 S2, #642).
///
/// `utc` is the phone's current time in unix seconds; `offset_min` is its local UTC offset in
/// minutes with **DST already applied** (the phone is the timezone oracle — the device carries no tz
/// tables). The device sets its UTC wall-clock set-point, persists the offset, and becomes *trusted*
/// for the boot — the safety gate the retention sweep reads. The app sends it immediately after
/// encryption and **before** `ackRides`, so ride `synced_at` stamping (S3) can assume a trusted clock.
///
/// [`decode`](Self::decode) checks the fixed 7-byte structure **and** the two plausibility gates a
/// bogus phone clock would fail — `utc` no earlier than 2020-01-01 ([`SET_CLOCK_MIN_UTC`]) and
/// `|offset|` within ±14 h ([`SET_CLOCK_MAX_OFFSET_MIN`]) — so a caller answers `error` (§4.3) on any
/// `Err` and `ok` on success. Keeping both checks here (not only the length) means the firmware and
/// the iOS mirror share one definition of "valid", pinned by the `command-set-clock.bin` vector.
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

    /// Decode a full `command` write (starting at the command byte). Errors — each answered `error`
    /// (§4.4) — are a wrong length or command byte ([`Truncated`](DescriptorError::Truncated) /
    /// [`UnknownOp`](DescriptorError::UnknownOp)), a `utc` before [`SET_CLOCK_MIN_UTC`], or an
    /// `offset_min` beyond ±[`SET_CLOCK_MAX_OFFSET_MIN`]. The write must be **exactly** 7 bytes
    /// (unlike `ackRides`, `setClock` carries no variable tail, so trailing bytes are malformed).
    pub fn decode(data: &[u8]) -> Result<Self, DescriptorError> {
        let bytes: [u8; Self::ENCODED_LEN] = data.try_into().map_err(|_| DescriptorError::Truncated)?;
        if bytes[0] != CMD_SET_CLOCK {
            return Err(DescriptorError::UnknownOp(bytes[0]));
        }
        let utc = u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        let offset_min = i16::from_le_bytes([bytes[5], bytes[6]]);
        // `unsigned_abs`, not `abs`: `i16::MIN.abs()` overflows (panics in a debug/test build), and a
        // 2-byte LE field can decode to `i16::MIN`. `unsigned_abs` maps it to `32768` cleanly, well
        // over the bound, so a bogus offset is rejected rather than panicking the decoder.
        if utc < SET_CLOCK_MIN_UTC || offset_min.unsigned_abs() > SET_CLOCK_MAX_OFFSET_MIN as u16 {
            return Err(DescriptorError::Truncated);
        }
        Ok(Self { utc, offset_min })
    }

    /// Encode into `out` (≥ [`ENCODED_LEN`](Self::ENCODED_LEN)); returns the written length or `None`
    /// for a too-small buffer. The app side encodes (its Swift codec mirrors this); the firmware only
    /// decodes — this exists for the shared-vector and round-trip tests.
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

/// The `setRouteRetention` command (§4.4, cmd `6`): `cmd u8 = 6 · object_id u16 LE · retention u8` —
/// the phone sets a stored route's retention level (auto-expiry epic #638 S4, #644).
///
/// `object_id` names a stored route; `retention` is the retention enum byte (`0` never · `1` 1 day ·
/// `2` 1 week · `3` 2 weeks · `4` 1 month · `5` 2 months), mirroring `obc_app::Retention`. The device
/// writes the level into its route-retention sidecar **without touching `last_used`** — changing
/// retention never resets the usage clock — and bumps the **route** store revision only on a real
/// change (setting the same value twice is `ok` with no bump). An unknown `object_id` answers
/// `notFound`; a `retention` above [`SET_ROUTE_RETENTION_MAX`] or a wrong-length write answers `error`.
/// The command is **additive** on protocol v2 — no `protocolVersion` bump.
///
/// [`decode`](Self::decode) folds the §4.4 validation (exact 4-byte length, cmd byte, `retention` in
/// range) so a caller answers `error` on any `Err`, and checks the id against the catalog separately
/// (that needs store state). Keeping the range check here — not only in the handler — means the
/// firmware and the iOS mirror share one definition of "valid", pinned by the
/// `command-set-route-retention.bin` vector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetRouteRetention {
    /// The stored route object id whose retention to set.
    pub object_id: u16,
    /// The retention enum byte (`0..=`[`SET_ROUTE_RETENTION_MAX`]); mirrors `obc_app::Retention`.
    pub retention: u8,
}

impl SetRouteRetention {
    /// The wire length: `cmd u8 · object_id u16 · retention u8`.
    pub const ENCODED_LEN: usize = 4;

    /// Decode a full `command` write (starting at the command byte). Errors — each answered `error`
    /// (§4.4) — are a wrong length or command byte ([`Truncated`](DescriptorError::Truncated) /
    /// [`UnknownOp`](DescriptorError::UnknownOp)) or a `retention` above [`SET_ROUTE_RETENTION_MAX`].
    /// The write must be **exactly** 4 bytes (like `setClock`, it carries no variable tail, so
    /// trailing bytes are malformed). An out-of-range `retention` reuses `Truncated` — the caller maps
    /// every `Err` to `error` regardless of variant, matching the `setClock` precedent.
    pub fn decode(data: &[u8]) -> Result<Self, DescriptorError> {
        let bytes: [u8; Self::ENCODED_LEN] = data.try_into().map_err(|_| DescriptorError::Truncated)?;
        if bytes[0] != CMD_SET_ROUTE_RETENTION {
            return Err(DescriptorError::UnknownOp(bytes[0]));
        }
        let object_id = u16::from_le_bytes([bytes[1], bytes[2]]);
        let retention = bytes[3];
        if retention > SET_ROUTE_RETENTION_MAX {
            return Err(DescriptorError::Truncated);
        }
        Ok(Self { object_id, retention })
    }

    /// Encode into `out` (≥ [`ENCODED_LEN`](Self::ENCODED_LEN)); returns the written length or `None`
    /// for a too-small buffer. The app side encodes (its Swift codec mirrors this); the firmware only
    /// decodes — this exists for the shared-vector and round-trip tests. It does **not** range-check
    /// `retention`, so a negative test can encode an out-of-range byte for [`decode`](Self::decode).
    pub fn encode(object_id: u16, retention: u8, out: &mut [u8]) -> Option<usize> {
        if out.len() < Self::ENCODED_LEN {
            return None;
        }
        out[0] = CMD_SET_ROUTE_RETENTION;
        out[1..3].copy_from_slice(&object_id.to_le_bytes());
        out[3] = retention;
        Some(Self::ENCODED_LEN)
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
/// **and** the device's current **store epoch** — a `u32` TRNG nonce naming the store's id era. The
/// epoch is **card-resident** (#776): it lives on the SD card, so the card carries its own era name.
/// It changes on an id-era reset — a lost RRAM id floor (full-chip reflash, factory reset, a torn
/// id-marks line) or an absent/torn card epoch file. Because it rides the card, a card swap
/// transplants the era, and a card written by a *different* device presents *its own* epoch — a
/// distinct `(serial, epoch)` scope on this device, which closes the former foreign-card hole (#776).
/// The app reads it first on every connect, before any reconcile, so it knows the era before it acks
/// or links anything; the epoch scopes all id-keyed app state to `(device serial, store epoch)` so a
/// reset can't silently alias months-old ids. A device with **no mounted store** has no epoch and
/// serves only the 2-byte version — the app fail-closes the ack (never epoch `0`, a legal value). The
/// mint rule lives on the device (V3); a random nonce leaks nothing beyond open DIS. Readable
/// **without** encryption.
///
/// **`obcm_version`** (E1, #911) is the third field: the **OBCM map-format version this firmware's
/// reader reads** — `10` today, the value of `obc_formats::obcm::VERSION`. It exists because nothing
/// else the device says carries it: [`PROTOCOL_VERSION`](crate::PROTOCOL_VERSION) is the *wire*
/// contract (a different number in a different sequence) and the DIS firmware-revision string maps
/// to a format version only through a table that exists nowhere. A host that offers map artifacts
/// (`OBCC_Spec.md` §6(c)) must not offer one this device cannot read, and this byte is the whole of
/// that decision. The reader supports exactly **one** version at a time (`OBCM_Spec.md` — earlier
/// maps get repacked), so it is a single `u8`, not a range. This crate deliberately does **not**
/// link `obc-formats` to source it — the wire codec stays dependency-free, exactly as it does for
/// the `fwImage` size ceiling — so the *caller* supplies the number from the reader's own constant.
///
/// ```text
///   version      u16   the protocol version (currently 2)
///   store_epoch  u32   the device's current store-epoch nonce      — absent on a store-less device
///   obcm_version u8    the OBCM map-format version the reader reads — absent before E1
/// ```
///
/// **The read is length-driven, and has been since #776** — the 2-byte no-store form is not a
/// degenerate case bolted on, it is how this attribute has always been decoded. E1 adds a third
/// length to the same mechanism:
///
/// | Bytes | Means |
/// | --: | :-- |
/// | 7 | the full read |
/// | 6 | a firmware that predates `obcm_version` → [`obcm_version`](Self::obcm_version) `None` |
/// | 2 | no mounted store → no epoch to name, and therefore no room for the byte after it |
///
/// A trailing field the read did not carry decodes to `None`, **never** to a fabricated default:
/// `obcm_version: Some(0)` would read as "supports OBCM v0" and refuse every real map, exactly the
/// way `store_epoch: 0` would name a legal-but-wrong id era. `None` means *unknown*, and a host
/// that cannot tell takes §6(c)'s no-known-target-firmware branch (offer, stating the version)
/// rather than guessing.
///
/// A store-less device serves 2 bytes even though it knows its OBCM version: the fields are
/// positional and `store_epoch` has no absent encoding, so byte 6 cannot be reached without
/// fabricating bytes 2..6. Serving a 3-byte `version · obcm` form instead would make byte 2 mean
/// two different things depending on total length — decodable, but the kind of positional
/// special-case that outlives the reason for it. A device with no card has nowhere to put a map
/// anyway, so nothing is lost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VersionRead {
    pub version: u16,
    pub store_epoch: u32,
    /// The OBCM map-format version the device's reader reads; `None` when the read carried no such
    /// byte (a firmware predating E1). Never `Some(0)` from a decode of a short read.
    pub obcm_version: Option<u8>,
}

impl VersionRead {
    /// The full read: `version u16 · store_epoch u32 · obcm_version u8`. Also the buffer size a
    /// caller reserves — [`encode`](Self::encode) reports how much of it is live.
    pub const ENCODED_LEN: usize = 7;
    /// The pre-E1 read: everything but the trailing `obcm_version`. Still decoded (as `None`), and
    /// still the shortest length a full [`decode`](Self::decode) accepts.
    pub const ENCODED_LEN_NO_OBCM: usize = 6;

    /// Encode into a fixed buffer; the returned length is the slice to serve (`&buf[..len]`) — 7
    /// bytes with an `obcm_version`, 6 without. The 2-byte no-store form is **not** produced here:
    /// it carries no `store_epoch`, so it is not a `VersionRead` at all (the board writes the bare
    /// `PROTOCOL_VERSION` bytes for it).
    pub fn encode(&self) -> ([u8; Self::ENCODED_LEN], usize) {
        let mut b = [0u8; Self::ENCODED_LEN];
        b[0..2].copy_from_slice(&self.version.to_le_bytes());
        b[2..6].copy_from_slice(&self.store_epoch.to_le_bytes());
        match self.obcm_version {
            Some(v) => {
                b[6] = v;
                (b, Self::ENCODED_LEN)
            }
            None => (b, Self::ENCODED_LEN_NO_OBCM),
        }
    }

    /// Decode an identity read. Accepts 6 bytes (`obcm_version = None`) and any longer read,
    /// taking byte 6 when it is there and ignoring anything past it — the append-only rule that
    /// lets this field land without a `PROTOCOL_VERSION` bump. A read shorter than 6 bytes —
    /// including the 2-byte no-store form — is [`Truncated`](DescriptorError::Truncated): there is
    /// no epoch in it, and inventing one is precisely what the ack fail-closed contract forbids.
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
