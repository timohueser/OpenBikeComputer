//! OBCS volume-set manifest codec from `OBCA_Spec.md` §5.2 / §5.3.
//!
//! One *logical* map is a **set**: this small manifest plus `1..=32` physical members (§5). The
//! manifest is parsed on the device, so the layout is fixed, little-endian, and needs no
//! allocation — a parsed [`SetManifest`] is a plain value with a fixed-capacity shard array
//! (32 B per shard resident; the 32-byte digests stay in the caller's byte buffer and are
//! reached through [`shard_digest`], because a device defers hashing).
//!
//! ## A member is named by identity, not by name (manifest v3, FS7)
//!
//! Every record carries its member's **`ObjectId`** — the flat store's store-global, never-reused
//! object identity (`FLAT_Store_Format.md` §3) — the terrain record included. That is what a mount
//! resolves a set through: it opens eight ids, not eight filenames it computed from a set number and
//! an ordinal. The derived `MS<id>S<kk>.OBM` names of §5.2 remain, because the FAT card the board
//! still reads has nothing else, but they are no longer the only way to reach a member and they are
//! not the way the flat store reaches one.
//!
//! An id is not known when a set is *assembled* — the store mints ids, and an assembler has never
//! spoken to one — so a manifest has two states and the format says which it is in:
//!
//! - **Unbound**: every member id is `0`, the reserved id that names no object. This is what an
//!   assembler writes. It is a complete, §5.3-valid manifest; it simply names no objects yet.
//! - **Bound**: every member id is non-zero and no two records share one. A client reaches this by
//!   committing each member, learning the id the store assigned, and writing it into the buffer it
//!   is still assembling with [`bind_member`] — all of it before the manifest itself is committed,
//!   which §5.4 already required to be the last write of a set.
//!
//! **Binding edits a staging buffer, and only a staging buffer.** §5.2 makes that normative:
//! binding MUST complete before the manifest is committed, and a committed manifest MUST NOT be
//! patched. The reason is that this is the one rule here a validator cannot enforce — an interrupted
//! 8-byte id write leaves a value that is neither `0` nor a duplicate, so [`validate`] accepts it,
//! [`SetManifest::is_bound`] answers `true`, and the mount resolves a member to an `ObjectId` naming
//! nothing or the wrong object. §5.4's magic-last-write is safe precisely because its torn shape *is*
//! recognisable; an id's is not.
//!
//! A half-bound manifest is refused ([`ManifestError::Members`]): it would name some members and
//! silently lose the rest. Per §5.2 it means **the set never existed** — a reader treats it as §5.4
//! treats any failed validation (not a map, no partial acceptance), and a client discards the whole
//! set and sends it again rather than repairing it. Which of the two *legal* states a mount will
//! accept is the mount's rule, not the codec's, and it depends on how that reader resolves members —
//! see [`SetManifest::is_bound`].
//!
//! Member ids are deliberately **not** in the `Set Id` digest chain. `Set Id` is a *content*
//! identity (§5.2: the same cells and skin produce the same id), and ids are properties of one
//! card's store. Keeping them out is also what lets [`bind_member`] write eight bytes into a staging
//! buffer instead of forcing a re-serialization.
//!
//! ## Two kinds of record, one array (manifest v2, EL4)
//!
//! Most records name an **OBCM shard**. At most one names the set's **terrain shard**
//! ([`Role::Terrain`], `OBCT_Spec.md` §4) — a raster, not a map, and therefore not something a
//! reader may open as OBCM, tile against the geometry roles, or count as a zoom level. That
//! record is required to be **last** in the array, which is what lets [`SetManifest::shards`]
//! keep meaning exactly what it meant before terrain existed: the OBCM shards, in the index
//! order their derived `MS<id>S<kk>.OBM` filenames count in. A consumer that never asks about
//! terrain therefore needs no change and cannot accidentally hand a raster to an OBCM parser.
//! [`SetManifest::records`] is the whole array, and it is what the wire `Shard Count`, the
//! `Set Id` digest chain and [`SetManifest::total_bytes`] count.
//!
//! This module is the *codec and validator* only. Mounting — checking that every derived
//! filename exists, has the recorded size, and opens as an OBCM file with the recorded
//! header bbox — is the reader's job (§5.3's last bullet), because only it has the files.

use crate::io::{checked_put_i32, checked_put_u32, checked_rd_i32, checked_rd_u32, validate_prefix, DecodeError};

/// `b"OBCS"` — the manifest magic (§5.2).
///
/// Two unrelated card sidecars in `obc-app` happen to tag themselves with the same four bytes
/// (`store_meta`'s `MAP.SEL` and `ride`'s `SYNCED.SET`). Different files, never fed to this
/// parser — noted only so a `grep` for the magic does not read as a collision.
pub const MAGIC: [u8; 4] = *b"OBCS";
/// The one accepted manifest version; readers reject any other value (§5.2).
///
/// `3` since FS7 (#1389) gave every record its member's `ObjectId`. A **hard cut**, per the
/// pre-release rule and exactly as the v1 → v2 cut was: the record width changed, so a v2 reader
/// handed v3 bytes would not merely miss the new field — it would read every record but the first at
/// the wrong offset. The version byte refuses the file instead, in both directions.
pub const VERSION: u8 = 3;
/// Fixed manifest header width (§5.2). Unchanged by v3: the new field is per member.
pub const HEADER_LEN: usize = 72;
/// Fixed per-shard record width (§5.2).
///
/// `64` since v3 appended the 8-byte member id. Appended rather than fitted into the three reserved
/// bytes because eight will not fit in three, and appended rather than inserted so that every field
/// v2 defined keeps the offset it had — the whole byte-level diff is "eight more bytes at the end of
/// each record". The width is also a power of two over an 8-aligned header, so a member id is
/// 8-aligned in any buffer that is.
pub const SHARD_RECORD_LEN: usize = 64;
/// Offset of the member `ObjectId` inside a record (§5.2), for a caller binding a staged manifest.
pub const MEMBER_ID_OFFSET: usize = 56;
/// The reserved member id that names no object (`FLAT_Store_Format.md` §3) — an **unbound** record.
pub const MEMBER_ID_NONE: u64 = 0;
/// `1..=32` shards per set (§5.2); readers reject `0` or `> 32`.
pub const MAX_SHARDS: usize = 32;
/// Display-name field width, `0xFF`-padded like an OBCM name (§5.2).
pub const NAME_LEN: usize = 24;
/// `Set Id` width — the first 16 bytes of SHA-256 over the shard digests (§5.2).
pub const SET_ID_LEN: usize = 16;
/// Per-shard SHA-256 digest width (§5.2).
pub const DIGEST_LEN: usize = 32;
/// Largest legal manifest, i.e. the buffer a mount must be able to read into.
pub const MAX_MANIFEST_LEN: usize = HEADER_LEN + SHARD_RECORD_LEN * MAX_SHARDS;

/// Longest derived shard filename, `MS999S31.OBM` (§5.2) — every name is 8.3-safe.
pub const MAX_SHARD_NAME_LEN: usize = 12;
/// The filename prefix that marks a volume set, kept clear of single-map `MP<id>.OBM` (§5.2).
pub const SET_PREFIX: &str = "MS";
/// The manifest extension (§5.2).
pub const MANIFEST_EXT: &str = ".OBS";
/// The shard extension — the same one a single received map uses (§5.2).
pub const SHARD_EXT: &str = ".OBM";
/// The terrain shard's extension: the 8.3 spelling of `.obcd` (`OBCT_Spec.md` §4.6). Deliberately
/// **not** `.OBT` — a device's recorded ride log already owns `.obct`.
pub const TERRAIN_EXT: &str = ".OBD";
/// Largest card id the derived 8.3 names can express (`MS999S31.OBM` is eight characters).
pub const MAX_SET_ID: u16 = 999;

/// A shard's role (§5.1). The ordering principle: the core holds only what cannot be split
/// by bbox, and everything that can be is moved out of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Role {
    /// Exactly one per set: the style table, marker color, unified nav graph, and POIs.
    /// Carries no ladder LOD at all, except in the single-file fast path (§5.5).
    Core = 0,
    /// The `mid`- and `fine`-band LODs and nothing else.
    Geometry = 1,
    /// The `coarse`-band LODs and nothing else; one by default, spanning the assembly.
    Coarse = 2,
    /// The set's **terrain shard** — an OBCT container, not an OBCM file (`OBCT_Spec.md` §4).
    /// At most one per set, spanning the assembly bbox, and always the **last** record.
    Terrain = 3,
}

impl Role {
    #[inline]
    pub const fn id(self) -> u8 {
        self as u8
    }

    #[inline]
    pub const fn from_id(id: u8) -> Option<Role> {
        Some(match id {
            0 => Role::Core,
            1 => Role::Geometry,
            2 => Role::Coarse,
            3 => Role::Terrain,
            _ => return None,
        })
    }
}

/// A shard bbox in microdegrees, in the OBCM header's field order (lat, lon, lat, lon).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SetBBox {
    pub min_lat: i32,
    pub min_lon: i32,
    pub max_lat: i32,
    pub max_lon: i32,
}

impl SetBBox {
    /// `min ≤ max` on both axes (§5.3).
    #[inline]
    pub const fn is_ordered(&self) -> bool {
        self.min_lat <= self.max_lat && self.min_lon <= self.max_lon
    }

    /// Whether `self` lies inside `outer` (§5.3).
    #[inline]
    pub const fn is_inside(&self, outer: &SetBBox) -> bool {
        self.min_lat >= outer.min_lat
            && self.min_lon >= outer.min_lon
            && self.max_lat <= outer.max_lat
            && self.max_lon <= outer.max_lon
    }

    /// Whether the two boxes share *interior* area. A tiling antichain (§5.1) has boxes that
    /// abut along an edge, which is not an overlap.
    #[inline]
    pub const fn overlaps_interior(&self, other: &SetBBox) -> bool {
        self.min_lat < other.max_lat
            && other.min_lat < self.max_lat
            && self.min_lon < other.max_lon
            && other.min_lon < self.max_lon
    }

    /// Area in µdeg². `i64` is exact: the world box is ≤ `360e6 × 180e6 ≈ 6.5e16`, and 32 of
    /// those still sit two orders of magnitude below `i64::MAX`.
    #[inline]
    pub const fn area(&self) -> i64 {
        let lat = (self.max_lat as i64) - (self.min_lat as i64);
        let lon = (self.max_lon as i64) - (self.min_lon as i64);
        lat * lon
    }

    /// Whether a viewport box intersects this one, closed on both ends — the dispatch predicate
    /// (§5.1: a viewport query goes to every shard whose bbox intersects it).
    #[inline]
    pub const fn intersects(&self, other: &SetBBox) -> bool {
        self.min_lat <= other.max_lat
            && other.min_lat <= self.max_lat
            && self.min_lon <= other.max_lon
            && other.min_lon <= self.max_lon
    }
}

/// One shard as the device keeps it resident: role, bbox, size, member id. The 32-byte digest is
/// deliberately *not* here — §5.3 lets a device defer the SHA-256 check, so keeping it would
/// be 1 KiB of RAM nothing reads. Hosts reach it with [`shard_digest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shard {
    pub role: Role,
    pub bbox: SetBBox,
    /// Shard size in bytes. `uint32` is exactly right: the FAT32 ceiling is `4 GiB − 1`.
    pub bytes: u32,
    /// The member's `ObjectId` in the flat store, or [`MEMBER_ID_NONE`] while the manifest is
    /// unbound (see the module header).
    ///
    /// A bare `u64` and not the store's `ObjectId` newtype on purpose: obc-formats is the
    /// dependency-free format floor and the store is a platform adapter above it, so the dependency
    /// runs the wrong way to import the type — the same reasoning that makes `obc-link` declare its
    /// own. The floor carries the *number*, and each layer wraps it in the identity type it owns.
    pub object_id: u64,
}

impl Default for Shard {
    fn default() -> Self {
        Shard { role: Role::Core, bbox: SetBBox::default(), bytes: 0, object_id: MEMBER_ID_NONE }
    }
}

/// A parsed, §5.3-validated set manifest.
///
/// Resident cost is `72 + 32 × 32 ≈ 1,096 B` at full capacity — the fixed array avoids both an
/// allocator and a `heapless` dependency in the format floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetManifest {
    /// The OBCM version of **every** shard.
    pub obcm_version: u8,
    /// The schema revision every cell was baked at (§6.3).
    pub schema_revision: u32,
    /// The assembly bbox (§4.2).
    pub bbox: SetBBox,
    /// Content identity: first 16 bytes of SHA-256 over the shard digests in index order.
    pub set_id: [u8; SET_ID_LEN],
    /// Display name, `0xFF`-padded; read it through [`SetManifest::name`].
    pub name: [u8; NAME_LEN],
    /// Index of the core shard. **Private on purpose**: it is an index into `shards`, and a public
    /// field would let a caller move it out of range between construction and use, turning every
    /// `core*` accessor and [`validate`] into a panic site. Both constructors range-check it, and
    /// [`SetManifest::core_shard`] reads it back.
    core_shard: u8,
    /// The wire `Shard Count`: **every** record, terrain included.
    record_count: u8,
    shards: [Shard; MAX_SHARDS],
}

impl SetManifest {
    /// The **OBCM** shards, in index order — the order the derived `MS<id>S<kk>.OBM` filenames
    /// count in. A terrain record is not one and is never in here (see the module header).
    #[inline]
    pub fn shards(&self) -> &[Shard] {
        &self.shards[..self.shard_count()]
    }

    /// How many OBCM shards the set has.
    #[inline]
    pub fn shard_count(&self) -> usize {
        self.record_count as usize - self.has_terrain() as usize
    }

    /// Every record the manifest carries, terrain included — the wire `Shard Count`'s array, the
    /// order the `Set Id` digest chain runs in, and what [`serialize`] wants digests for.
    #[inline]
    pub fn records(&self) -> &[Shard] {
        &self.shards[..self.record_count as usize]
    }

    /// The wire `Shard Count`.
    #[inline]
    pub fn record_count(&self) -> usize {
        self.record_count as usize
    }

    #[inline]
    fn has_terrain(&self) -> bool {
        matches!(self.shards.get(self.record_count as usize - 1), Some(s) if s.role == Role::Terrain)
    }

    /// The set's terrain shard, or `None` when it carries no raster — which is a complete,
    /// ordinary map (`OBCC_Spec.md` §13: elevation degrades to "none is known here").
    #[inline]
    pub fn terrain(&self) -> Option<&Shard> {
        self.records().last().filter(|s| s.role == Role::Terrain)
    }

    /// Index of the core shard (§5.1); always `< shard_count`, by construction.
    #[inline]
    pub fn core_shard(&self) -> usize {
        self.core_shard as usize
    }

    /// Whether every member id names an object (§5.2). `false` means every id is
    /// [`MEMBER_ID_NONE`] — the manifest is **unbound**; a half-bound one never parses.
    ///
    /// This is the precondition of resolving a set *by identity*, and therefore the one thing a
    /// flat-store mount must check that a §5.3 validation does not: an unbound manifest is a valid
    /// manifest that simply names no objects, and opening it would mean opening id `0` eight times.
    /// A reader that resolves members by their derived §5.2 filenames instead — the FAT path the
    /// board still runs — needs no id and is unaffected either way.
    #[inline]
    pub fn is_bound(&self) -> bool {
        self.records().iter().all(|record| record.object_id != MEMBER_ID_NONE)
    }

    /// The **OBCM** shards' member ids, in index order — the ids a mount opens as map files.
    ///
    /// This and [`terrain_id`](Self::terrain_id) are the whole set-resolution seam: they are pure,
    /// they read only an already-validated value, and between them they say which ids a mount needs
    /// *with the raster held separately*, so a caller cannot hand it to an OBCM parser by looping
    /// over one list. Both are `MEMBER_ID_NONE` throughout on an unbound manifest, which
    /// [`is_bound`](Self::is_bound) is there to ask about first.
    #[inline]
    pub fn shard_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.shards().iter().map(|shard| shard.object_id)
    }

    /// The terrain shard's member id, or `None` when the set carries no raster.
    #[inline]
    pub fn terrain_id(&self) -> Option<u64> {
        self.terrain().map(|shard| shard.object_id)
    }

    /// The core shard's record (§5.1) — nav and POI queries always go here.
    ///
    /// Total by construction *and* by code: `core_shard` is private and both constructors reject
    /// an out-of-range value, and the lookup still goes through `get` so no future edit can turn
    /// the accessor into a panic on the device.
    #[inline]
    pub fn core(&self) -> &Shard {
        match self.shards.get(self.core_shard as usize) {
            Some(shard) => shard,
            None => &self.shards[0],
        }
    }

    /// The display name with its `0xFF` padding trimmed, or `None` if it is not printable ASCII.
    pub fn name(&self) -> Option<&str> {
        let end = self.name.iter().position(|&b| b == 0xFF).unwrap_or(NAME_LEN);
        let raw = &self.name[..end];
        if !raw.iter().all(|&b| (0x20..=0x7E).contains(&b)) {
            return None;
        }
        core::str::from_utf8(raw).ok()
    }

    /// Whether this is the single-file fast path (§5.5): one **OBCM** shard, which is the core
    /// and carries everything. A terrain sidecar beside it does not change that — terrain is
    /// always its own file, so the fast path is about the map, not about the file count.
    #[inline]
    pub fn is_single_file(&self) -> bool {
        self.shard_count() == 1
    }

    /// Total bytes of the set — the only size figure a UI may show (§5.4). Terrain included:
    /// it is space on the card either way.
    pub fn total_bytes(&self) -> u64 {
        self.records().iter().map(|shard| shard.bytes as u64).sum()
    }

    /// Serialized length of this manifest.
    #[inline]
    pub fn encoded_len(&self) -> usize {
        manifest_len(self.record_count as usize)
    }
}

/// Serialized length of a manifest with `shard_count` shards (§5.2).
#[inline]
pub const fn manifest_len(shard_count: usize) -> usize {
    HEADER_LEN + SHARD_RECORD_LEN * shard_count
}

/// Why a manifest was rejected. §5.3 has **no partial acceptance**: a set that does not
/// validate does not mount, and the shards that happen to be present are not a map (§5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestError {
    /// Short buffer, wrong magic, unsupported version, or a non-zero reserved field.
    Layout,
    /// Unsupported `Version` byte.
    Version,
    /// Length is not exactly `72 + 64 × Shard Count`.
    Length,
    /// `Shard Count` outside `1..=32`, or `Core Shard` out of range.
    ShardCount,
    /// A role byte outside `0..=2`, no core, more than one core, or a role the schema's bands
    /// name with no shard at all.
    Roles,
    /// A bbox with `min > max`, a shard outside the assembly, a core whose bbox is not the
    /// assembly bbox, or a role whose shards do not tile the assembly bbox.
    Geometry,
    /// The member ids are neither all `0` nor all distinct and non-zero (§5.2/§5.3) — a manifest
    /// that is half-bound, or one that names a single object twice.
    Members,
}

impl From<DecodeError> for ManifestError {
    fn from(error: DecodeError) -> ManifestError {
        match error {
            DecodeError::Version => ManifestError::Version,
            _ => ManifestError::Layout,
        }
    }
}

fn read_bbox(bytes: &[u8], offset: usize) -> Result<SetBBox, DecodeError> {
    Ok(SetBBox {
        min_lat: checked_rd_i32(bytes, offset)?,
        min_lon: checked_rd_i32(bytes, offset + 4)?,
        max_lat: checked_rd_i32(bytes, offset + 8)?,
        max_lon: checked_rd_i32(bytes, offset + 12)?,
    })
}

fn write_bbox(bytes: &mut [u8], offset: usize, bbox: &SetBBox) -> Result<(), DecodeError> {
    checked_put_i32(bytes, offset, bbox.min_lat)?;
    checked_put_i32(bytes, offset + 4, bbox.min_lon)?;
    checked_put_i32(bytes, offset + 8, bbox.max_lat)?;
    checked_put_i32(bytes, offset + 12, bbox.max_lon)
}

/// One byte at `offset`, or [`ManifestError::Layout`] when the buffer is shorter than that.
///
/// Every fixed field goes through this or a `checked_rd_*`: the manifest arrives from an SD card,
/// where a short read is the ordinary shape of a torn write, and a panic in `no_std` is a reset on
/// every boot. [`validate_prefix`] only guarantees five bytes, so bytes 5..8 are *not* implied by
/// it — that is exactly the gap a 5-, 6- or 7-byte file falls through.
#[inline]
fn byte_at(bytes: &[u8], offset: usize) -> Result<u8, ManifestError> {
    bytes.get(offset).copied().ok_or(ManifestError::Layout)
}

/// A fixed-width array field at `offset`, or [`ManifestError::Layout`] for a short buffer.
#[inline]
fn array_at<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], ManifestError> {
    let end = offset.checked_add(N).ok_or(ManifestError::Layout)?;
    let field: &[u8; N] =
        bytes.get(offset..end).ok_or(ManifestError::Layout)?.try_into().ok().ok_or(ManifestError::Layout)?;
    Ok(*field)
}

/// Parse and fully validate a manifest per §5.3.
///
/// **Total**: every read is bounds-checked and every rejection is a typed [`ManifestError`]. No
/// input — truncated, mid-copy, or actively hostile — makes this panic, which is the whole reason
/// the format authority owns the parse rather than each consumer.
///
/// The caller owns `bytes` (at most [`MAX_MANIFEST_LEN`]); nothing here allocates and nothing
/// borrows, so the returned value outlives the buffer.
pub fn parse(bytes: &[u8]) -> Result<SetManifest, ManifestError> {
    validate_prefix(bytes, &MAGIC, VERSION, VERSION)?;

    let obcm_version = byte_at(bytes, 5)?;
    let record_count = byte_at(bytes, 6)? as usize;
    if record_count == 0 || record_count > MAX_SHARDS {
        return Err(ManifestError::ShardCount);
    }
    if bytes.len() != manifest_len(record_count) {
        return Err(ManifestError::Length);
    }
    let core_shard = byte_at(bytes, 7)?;
    if core_shard as usize >= record_count {
        return Err(ManifestError::ShardCount);
    }
    let schema_revision = checked_rd_u32(bytes, 8)?;
    if checked_rd_u32(bytes, 12)? != 0 {
        return Err(ManifestError::Layout);
    }
    let bbox = read_bbox(bytes, 16)?;

    let set_id: [u8; SET_ID_LEN] = array_at(bytes, 32)?;
    let name: [u8; NAME_LEN] = array_at(bytes, 48)?;

    let mut shards = [Shard::default(); MAX_SHARDS];
    for (index, shard) in shards.iter_mut().enumerate().take(record_count) {
        let base = HEADER_LEN + index * SHARD_RECORD_LEN;
        let role = Role::from_id(byte_at(bytes, base)?).ok_or(ManifestError::Roles)?;
        if array_at::<3>(bytes, base + 1)? != [0, 0, 0] {
            return Err(ManifestError::Layout);
        }
        *shard = Shard {
            role,
            bbox: read_bbox(bytes, base + 4)?,
            bytes: checked_rd_u32(bytes, base + 20)?,
            object_id: u64::from_le_bytes(array_at::<8>(bytes, base + MEMBER_ID_OFFSET)?),
        };
    }

    let manifest = SetManifest {
        obcm_version,
        core_shard,
        schema_revision,
        bbox,
        set_id,
        name,
        record_count: record_count as u8,
        shards,
    };
    validate(&manifest)?;
    Ok(manifest)
}

/// The §5.3 semantic rules, over an already-decoded manifest. Split out so a host assembler
/// can check a manifest it is about to write without serializing it first.
pub fn validate(manifest: &SetManifest) -> Result<(), ManifestError> {
    let records = manifest.records();
    // A terrain record is legal only as the **last** one. That is what makes `shards()` a plain
    // prefix slice and therefore what keeps every OBCM mount path free of a role filter — so it is
    // checked here, once, rather than trusted everywhere else.
    if records.iter().rev().skip(1).any(|shard| shard.role == Role::Terrain) {
        return Err(ManifestError::Roles);
    }
    // Bound or unbound, never in between (§5.2). A half-bound manifest is the shape a client that
    // died mid-`bind_member` leaves, and it is the one shape that is actively dangerous: it looks
    // resolvable, and a mount that trusted it would open the members that were patched and silently
    // lose the ones that were not.
    let named = records.iter().filter(|record| record.object_id != MEMBER_ID_NONE).count();
    if named != 0 && named != records.len() {
        return Err(ManifestError::Members);
    }
    // Distinct, once bound. Two records sharing an id is one object claiming two roles, which no
    // store can make true — the ids come from a never-reused monotonic cursor. They are *not*
    // required to ascend: a set that shares a shard with one already on the card reuses that
    // object, so an older id beside a newer one is the dedup working, not a fault.
    for (index, record) in records.iter().enumerate() {
        if record.object_id != MEMBER_ID_NONE
            && records[index + 1..].iter().any(|other| other.object_id == record.object_id)
        {
            return Err(ManifestError::Members);
        }
    }
    let shards = manifest.shards();
    let core_index = manifest.core_shard as usize;
    // Bounds-checked rather than indexed: `validate` is the *authority*, so it may not assume the
    // invariant it exists to establish. Indexed into the OBCM prefix, so a `Core Shard` pointing at
    // the terrain record is refused rather than mounted as a map.
    let core = shards.get(core_index).ok_or(ManifestError::ShardCount)?;

    // Exactly one core, and it is the one the header names.
    let cores = shards.iter().filter(|shard| shard.role == Role::Core).count();
    if cores != 1 || core.role != Role::Core {
        return Err(ManifestError::Roles);
    }
    // A role with no shard is a map missing whole zoom levels — unless this is the §5.5
    // single-file fast path, where the one shard is the core and carries everything. Counted over
    // the OBCM shards alone: a terrain record adds a raster, never a zoom level, so a one-shard map
    // with terrain is still the fast path.
    if shards.len() > 1 {
        for role in [Role::Geometry, Role::Coarse] {
            if !shards.iter().any(|shard| shard.role == role) {
                return Err(ManifestError::Roles);
            }
        }
    }

    // Ordered **and** non-degenerate. A zero-area box is not merely useless: it is invisible to
    // both halves of the tiling proof below — `overlaps_interior` is strict, so it collides with
    // nothing, and it contributes 0 to the area sum — so a manifest could otherwise pair a
    // full-assembly shard with any number of degenerate ones and still "tile". Ground with no
    // area is not ground.
    if !manifest.bbox.is_ordered() || manifest.bbox.area() == 0 {
        return Err(ManifestError::Geometry);
    }
    for shard in records {
        if !shard.bbox.is_ordered() || shard.bbox.area() == 0 || !shard.bbox.is_inside(&manifest.bbox) {
            return Err(ManifestError::Geometry);
        }
    }
    if core.bbox != manifest.bbox {
        return Err(ManifestError::Geometry);
    }
    // The terrain shard spans the whole assembly, like the core: it is one raster over the map's
    // ground, and a partial one would leave elevation silently absent in part of a map that looks
    // complete. (Splitting it by bbox is a later problem — see `OBCA_Spec.md` §5.1.)
    if let Some(terrain) = manifest.terrain() {
        if terrain.bbox != manifest.bbox {
            return Err(ManifestError::Geometry);
        }
    }

    // Each non-core role tiles the assembly: pairwise interior-disjoint, and the areas sum to
    // the assembly's. Disjoint + inside + equal total area ⇒ the union is the assembly bbox,
    // which is the §5.1 antichain property without needing polygon arithmetic.
    for role in [Role::Geometry, Role::Coarse] {
        let mut area = 0i64;
        let mut seen = 0usize;
        for (index, shard) in shards.iter().enumerate() {
            if shard.role != role {
                continue;
            }
            seen += 1;
            area += shard.bbox.area();
            for other in &shards[index + 1..] {
                if other.role == role && shard.bbox.overlaps_interior(&other.bbox) {
                    return Err(ManifestError::Geometry);
                }
            }
        }
        if seen > 0 && area != manifest.bbox.area() {
            return Err(ManifestError::Geometry);
        }
    }
    Ok(())
}

/// The 32-byte SHA-256 digest of shard `index`, read straight out of the manifest bytes.
/// A device MAY defer this check (§5.3); a host writing a set MUST verify every digest.
pub fn shard_digest(bytes: &[u8], index: usize) -> Option<&[u8; DIGEST_LEN]> {
    let base = HEADER_LEN.checked_add(index.checked_mul(SHARD_RECORD_LEN)?)?.checked_add(24)?;
    bytes.get(base..base.checked_add(DIGEST_LEN)?)?.try_into().ok()
}

/// Serialize `manifest` into `out`, returning the written length. `digests` supplies one
/// SHA-256 per shard in index order. The manifest is validated first: §5.4 makes the manifest
/// the atomicity token, so writing an invalid one is never useful.
pub fn serialize(manifest: &SetManifest, digests: &[[u8; DIGEST_LEN]], out: &mut [u8]) -> Result<usize, ManifestError> {
    validate(manifest)?;
    let shards = manifest.records();
    if digests.len() != shards.len() {
        return Err(ManifestError::ShardCount);
    }
    let len = manifest.encoded_len();
    let out = out.get_mut(..len).ok_or(ManifestError::Layout)?;
    out.fill(0);

    out[0..4].copy_from_slice(&MAGIC);
    out[4] = VERSION;
    out[5] = manifest.obcm_version;
    out[6] = shards.len() as u8;
    out[7] = manifest.core_shard;
    checked_put_u32(out, 8, manifest.schema_revision)?;
    checked_put_u32(out, 12, 0)?;
    write_bbox(out, 16, &manifest.bbox)?;
    out[32..32 + SET_ID_LEN].copy_from_slice(&manifest.set_id);
    out[48..48 + NAME_LEN].copy_from_slice(&manifest.name);

    for (index, (shard, digest)) in shards.iter().zip(digests).enumerate() {
        let base = HEADER_LEN + index * SHARD_RECORD_LEN;
        out[base] = shard.role.id();
        write_bbox(out, base + 4, &shard.bbox)?;
        checked_put_u32(out, base + 20, shard.bytes)?;
        out[base + 24..base + 24 + DIGEST_LEN].copy_from_slice(digest);
        out[base + MEMBER_ID_OFFSET..base + SHARD_RECORD_LEN].copy_from_slice(&shard.object_id.to_le_bytes());
    }
    Ok(len)
}

/// Write record `index`'s member id into a **staging** manifest buffer (§5.2).
///
/// This is how a client binds a set: it commits each member, learns the id the store assigned, and
/// writes it here — one 8-byte field, without re-serializing and without disturbing `Set Id`, which
/// covers the digests and never the ids. `id` must name an object; passing [`MEMBER_ID_NONE`] would
/// *un*bind a record, which is the half-bound shape [`validate`] exists to refuse, so it is rejected
/// here rather than left to be discovered at the next parse.
///
/// **`bytes` MUST be a buffer the client still owns, never a committed manifest.** §5.2 makes that
/// normative and it is not a style preference: a committed manifest is bytes a reader may resolve a
/// set through, and an id write interrupted halfway leaves a value neither `0` nor duplicated —
/// which [`validate`] accepts, [`SetManifest::is_bound`] calls bound, and a mount follows to an
/// object that is not there. Bind every record first; commit once, afterwards.
///
/// The bytes are otherwise unexamined — this reads `Shard Count` to bound the index and nothing
/// else. Whether the result is a valid, fully bound manifest is [`parse`]'s answer, and a client
/// SHOULD ask it before the manifest is committed.
pub fn bind_member(bytes: &mut [u8], index: usize, id: u64) -> Result<(), ManifestError> {
    if id == MEMBER_ID_NONE {
        return Err(ManifestError::Members);
    }
    validate_prefix(bytes, &MAGIC, VERSION, VERSION)?;
    let record_count = byte_at(bytes, 6)? as usize;
    if record_count == 0 || record_count > MAX_SHARDS || index >= record_count {
        return Err(ManifestError::ShardCount);
    }
    if bytes.len() != manifest_len(record_count) {
        return Err(ManifestError::Length);
    }
    let base = HEADER_LEN + index * SHARD_RECORD_LEN + MEMBER_ID_OFFSET;
    bytes.get_mut(base..base + 8).ok_or(ManifestError::Layout)?.copy_from_slice(&id.to_le_bytes());
    Ok(())
}

/// Build a manifest value from its parts, validating §5.3 before it can be observed.
pub fn build(
    obcm_version: u8,
    core_shard: u8,
    schema_revision: u32,
    bbox: SetBBox,
    set_id: [u8; SET_ID_LEN],
    name: [u8; NAME_LEN],
    parts: &[Shard],
) -> Result<SetManifest, ManifestError> {
    if parts.is_empty() || parts.len() > MAX_SHARDS {
        return Err(ManifestError::ShardCount);
    }
    if core_shard as usize >= parts.len() {
        return Err(ManifestError::ShardCount);
    }
    let mut shards = [Shard::default(); MAX_SHARDS];
    shards[..parts.len()].copy_from_slice(parts);
    let manifest = SetManifest {
        obcm_version,
        core_shard,
        schema_revision,
        bbox,
        set_id,
        name,
        record_count: parts.len() as u8,
        shards,
    };
    validate(&manifest)?;
    Ok(manifest)
}

/// A derived 8.3 filename, `0`-terminated free (use [`FileName::as_str`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileName {
    buf: [u8; MAX_SHARD_NAME_LEN],
    len: u8,
}

impl FileName {
    #[inline]
    pub fn as_str(&self) -> &str {
        // Every byte written by the formatters below is printable ASCII.
        core::str::from_utf8(&self.buf[..self.len as usize]).unwrap_or("")
    }
}

fn push(buf: &mut [u8; MAX_SHARD_NAME_LEN], len: &mut usize, text: &[u8]) {
    for &byte in text {
        if *len < MAX_SHARD_NAME_LEN {
            buf[*len] = byte;
            *len += 1;
        }
    }
}

fn push_decimal(buf: &mut [u8; MAX_SHARD_NAME_LEN], len: &mut usize, value: u16, pad_to: usize) {
    let mut digits = [0u8; 3];
    let mut count = 0usize;
    let mut rest = value;
    loop {
        digits[count] = b'0' + (rest % 10) as u8;
        count += 1;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    while count < pad_to {
        digits[count] = b'0';
        count += 1;
    }
    for index in (0..count).rev() {
        push(buf, len, &[digits[index]]);
    }
}

/// `MS<id>.OBS` — the manifest of set `id` (§5.2). `None` above [`MAX_SET_ID`].
pub fn manifest_name(id: u16) -> Option<FileName> {
    if id > MAX_SET_ID {
        return None;
    }
    let mut buf = [0u8; MAX_SHARD_NAME_LEN];
    let mut len = 0usize;
    push(&mut buf, &mut len, SET_PREFIX.as_bytes());
    push_decimal(&mut buf, &mut len, id, 1);
    push(&mut buf, &mut len, MANIFEST_EXT.as_bytes());
    Some(FileName { buf, len: len as u8 })
}

/// `MS<id>.OBD` — the terrain shard of set `id` (§5.2). `None` above [`MAX_SET_ID`].
///
/// It carries **no `S<kk>`**, and that is the rule rather than an omission: there is at most one
/// terrain shard per set, so an index would be a number that is always `00` and a second thing to
/// keep in step with the manifest. The name is also exactly the manifest's own stem with the
/// terrain extension, which makes it the `OBCT_Spec.md` §4.6 sidecar of `MS<id>.OBS` — so a host
/// that resolves terrain by sidecar and a host that reads the manifest role land on one file.
pub fn terrain_name(id: u16) -> Option<FileName> {
    if id > MAX_SET_ID {
        return None;
    }
    let mut buf = [0u8; MAX_SHARD_NAME_LEN];
    let mut len = 0usize;
    push(&mut buf, &mut len, SET_PREFIX.as_bytes());
    push_decimal(&mut buf, &mut len, id, 1);
    push(&mut buf, &mut len, TERRAIN_EXT.as_bytes());
    Some(FileName { buf, len: len as u8 })
}

/// `MS<id>S<kk>.OBM` — shard `index` of set `id` (§5.2). Filenames are **derived, not
/// stored**: a stored name is a second source of truth that can disagree with the directory.
pub fn shard_name(id: u16, index: usize) -> Option<FileName> {
    if id > MAX_SET_ID || index >= MAX_SHARDS {
        return None;
    }
    let mut buf = [0u8; MAX_SHARD_NAME_LEN];
    let mut len = 0usize;
    push(&mut buf, &mut len, SET_PREFIX.as_bytes());
    push_decimal(&mut buf, &mut len, id, 1);
    push(&mut buf, &mut len, b"S");
    push_decimal(&mut buf, &mut len, index as u16, 2);
    push(&mut buf, &mut len, SHARD_EXT.as_bytes());
    Some(FileName { buf, len: len as u8 })
}

/// Every filename a whole-set delete must remove, **in the order it must remove them**: the
/// manifest first, then every derived shard name up to the [`MAX_SHARDS`] cap.
///
/// This is the plan, not the execution — a caller runs it against its own filesystem. It is a pure
/// function so the *ordering*, which is normative, can be asserted where tests run rather than only
/// on a device:
///
/// - **The manifest goes first** (§5.4). It is the atomicity token, so a power cut after it is gone
///   leaves *orphans* — files no manifest references, invisible as a map and reclaimable. The
///   reverse order would leave a manifest pointing at files that are gone, which is a broken map
///   rather than an absent one. A caller that cannot delete the manifest MUST stop and leave the
///   shards alone.
/// - **The shard sweep runs to the cap, not to the set's own `Shard Count`**, so replacing a set
///   with a smaller one also reclaims the tail of the old one. Names that are not there fail
///   harmlessly; there is no state in which a stale `MS7S09.OBM` should survive a delete of `MS7`.
///
/// `None` when `id` has no derived 8.3 name at all (above [`MAX_SET_ID`]). The plain array keeps
/// the format floor free of `heapless` (see [`SetManifest`]); at 13 B per name it is 429 B, paid
/// once on a boot-time retire.
pub fn delete_plan(id: u16) -> Option<[FileName; DELETE_PLAN_LEN]> {
    let mut plan = [manifest_name(id)?; DELETE_PLAN_LEN];
    for index in 0..MAX_SHARDS {
        plan[index + 1] = shard_name(id, index)?;
    }
    // The terrain shard is swept unconditionally, exactly like the shard tail: a set that once
    // carried terrain and is replaced by one that does not must not leave megabytes of raster
    // behind that nothing references (§5.4's orphan rule).
    plan[MAX_SHARDS + 1] = terrain_name(id)?;
    Some(plan)
}

/// Length of [`delete_plan`]'s list: the manifest, every shard name the cap can express, and the
/// terrain shard.
pub const DELETE_PLAN_LEN: usize = MAX_SHARDS + 2;

/// Recover the set id from a `MS<id>.OBS` manifest filename, or `None` if `name` is not one.
/// Deliberately strict: no lowercase, no leading zeros beyond a bare `0`, id `≤ 999`.
pub fn parse_manifest_name(name: &[u8]) -> Option<u16> {
    let rest = name.strip_prefix(SET_PREFIX.as_bytes())?;
    let digits = rest.strip_suffix(MANIFEST_EXT.as_bytes())?;
    parse_id(digits)
}

fn parse_id(digits: &[u8]) -> Option<u16> {
    if digits.is_empty() || digits.len() > 3 {
        return None;
    }
    if digits.len() > 1 && digits[0] == b'0' {
        return None;
    }
    let mut value = 0u16;
    for &byte in digits {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + (byte - b'0') as u16;
    }
    Some(value)
}

/// Recover `(set id, shard index)` from a `MS<id>S<kk>.OBM` shard filename. A file that only
/// *looks* like a shard is still an orphan until a manifest names it (§5.4).
pub fn parse_shard_name(name: &[u8]) -> Option<(u16, usize)> {
    let rest = name.strip_prefix(SET_PREFIX.as_bytes())?;
    let body = rest.strip_suffix(SHARD_EXT.as_bytes())?;
    let split = body.iter().rposition(|&byte| byte == b'S')?;
    let (id, index) = body.split_at(split);
    let index = &index[1..];
    if index.len() != 2 || !index.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let index = ((index[0] - b'0') as usize) * 10 + (index[1] - b'0') as usize;
    if index >= MAX_SHARDS {
        return None;
    }
    Some((parse_id(id)?, index))
}

const _: () = assert!(HEADER_LEN == 4 + 1 + 1 + 1 + 1 + 4 + 4 + 16 + SET_ID_LEN + NAME_LEN);
const _: () = assert!(SHARD_RECORD_LEN == 1 + 1 + 2 + 16 + 4 + DIGEST_LEN + 8);
const _: () = assert!(MEMBER_ID_OFFSET == 1 + 1 + 2 + 16 + 4 + DIGEST_LEN);
const _: () = assert!(MAX_MANIFEST_LEN == 2120);
// The member id is 8-aligned inside an 8-aligned buffer, which is what makes the record width a
// power of two worth having rather than a coincidence.
const _: () = assert!(HEADER_LEN.is_multiple_of(8) && SHARD_RECORD_LEN.is_multiple_of(8));
const _: () = assert!(MEMBER_ID_OFFSET.is_multiple_of(8));

#[cfg(test)]
mod tests {
    use super::*;

    const WORLD: SetBBox = SetBBox { min_lat: 47_000_000, min_lon: 7_000_000, max_lat: 48_000_000, max_lon: 9_000_000 };

    fn name_field(text: &str) -> [u8; NAME_LEN] {
        let mut out = [0xFFu8; NAME_LEN];
        out[..text.len()].copy_from_slice(text.as_bytes());
        out
    }

    /// A record of `role`, unbound — the shape an assembler writes. Tests that care about member
    /// ids set `object_id` explicitly.
    fn part(role: Role, bbox: SetBBox, bytes: u32) -> Shard {
        Shard { role, bbox, bytes, object_id: MEMBER_ID_NONE }
    }

    fn core(bbox: SetBBox, bytes: u32) -> Shard {
        part(Role::Core, bbox, bytes)
    }

    /// A three-shard set: core + coarse over the whole box, two geometry halves that tile it.
    /// Bound, with the ids deliberately *not* ascending — see [`validate`].
    fn split_set() -> SetManifest {
        let mid = (WORLD.min_lon + WORLD.max_lon) / 2;
        let west = SetBBox { max_lon: mid, ..WORLD };
        let east = SetBBox { min_lon: mid, ..WORLD };
        build(
            crate::obcm::VERSION,
            0,
            7,
            WORLD,
            [0xA5; SET_ID_LEN],
            name_field("Alpen"),
            &[
                Shard { object_id: 90, ..core(WORLD, 1_000) },
                Shard { object_id: 12, ..part(Role::Coarse, WORLD, 2_000) },
                Shard { object_id: 91, ..part(Role::Geometry, west, 3_000) },
                Shard { object_id: 92, ..part(Role::Geometry, east, 4_000) },
            ],
        )
        .unwrap()
    }

    fn encode(manifest: &SetManifest) -> [u8; MAX_MANIFEST_LEN] {
        let digests = [[0x11u8; DIGEST_LEN]; MAX_SHARDS];
        let mut out = [0u8; MAX_MANIFEST_LEN];
        let len = serialize(manifest, &digests[..manifest.record_count()], &mut out).unwrap();
        assert_eq!(len, manifest.encoded_len());
        out
    }

    #[test]
    fn record_widths_pin_spec_arithmetic() {
        assert_eq!(HEADER_LEN, 72);
        assert_eq!(SHARD_RECORD_LEN, 64);
        assert_eq!(MEMBER_ID_OFFSET, 56);
        assert_eq!(manifest_len(1), 136);
        assert_eq!(manifest_len(32), MAX_MANIFEST_LEN);
        assert_eq!(MAX_MANIFEST_LEN, 2120);
        assert_eq!(Role::Core.id(), 0);
        assert_eq!(Role::Geometry.id(), 1);
        assert_eq!(Role::Coarse.id(), 2);
        assert_eq!(Role::Terrain.id(), 3);
        assert_eq!(Role::from_id(4), None);
        assert_eq!(VERSION, 3, "FS7 (#1389) is a hard cut, not a compatible extension");
    }

    /// The terrain record is the last one, it is not an OBCM shard, and it changes neither the
    /// shard indexing nor the fast path. This is the whole EL4 contract in one test: a consumer
    /// that only ever asks for `shards()` sees exactly what it saw before terrain existed.
    #[test]
    fn a_terrain_record_rides_last_and_is_not_a_shard() {
        let solo = build(
            11,
            0,
            1,
            WORLD,
            [0; SET_ID_LEN],
            name_field("Grimsel"),
            &[core(WORLD, 42), part(Role::Terrain, WORLD, 6_192)],
        )
        .expect("core + terrain is the single-file fast path with a raster beside it");
        assert_eq!(solo.shard_count(), 1, "one OBCM shard");
        assert_eq!(solo.record_count(), 2, "…and two records on the wire");
        assert!(solo.is_single_file(), "terrain is always its own file, so the fast path still applies");
        assert_eq!(solo.shards().len(), 1);
        assert_eq!(solo.shards()[0].role, Role::Core);
        assert_eq!(solo.terrain().map(|t| t.bytes), Some(6_192));
        assert_eq!(solo.total_bytes(), 42 + 6_192, "the card pays for the raster too");

        let bytes = encode(&solo);
        assert_eq!(bytes[4], VERSION, "manifest Version");
        assert_eq!(bytes[6], 2, "wire Shard Count counts every record");
        assert_eq!(bytes[HEADER_LEN + SHARD_RECORD_LEN], Role::Terrain.id());
        assert_eq!(parse(&bytes[..solo.encoded_len()]).unwrap(), solo);

        // A set with no raster is complete and says so.
        let plain = build(11, 0, 1, WORLD, [0; SET_ID_LEN], name_field("Solo"), &[core(WORLD, 42)]).unwrap();
        assert_eq!(plain.terrain(), None);
    }

    /// The invariants that make the prefix slice safe: at most one terrain record, and never
    /// anywhere but last. Both are refused rather than tolerated, because `shards()` would
    /// otherwise hand a raster to an OBCM parser.
    #[test]
    fn a_terrain_record_that_is_not_last_is_refused() {
        let terrain = part(Role::Terrain, WORLD, 1);
        assert_eq!(
            build(11, 1, 1, WORLD, [0; SET_ID_LEN], name_field("x"), &[terrain, core(WORLD, 1)]),
            Err(ManifestError::Roles),
            "terrain first would shift every derived shard index"
        );
        assert_eq!(
            build(11, 0, 1, WORLD, [0; SET_ID_LEN], name_field("x"), &[core(WORLD, 1), terrain, terrain]),
            Err(ManifestError::Roles),
            "one raster per set"
        );
        // `Core Shard` may not name the terrain record: it indexes the OBCM prefix, so a value
        // that lands on the raster is out of range rather than a mountable core.
        assert_eq!(
            build(11, 1, 1, WORLD, [0; SET_ID_LEN], name_field("x"), &[core(WORLD, 1), terrain]),
            Err(ManifestError::ShardCount)
        );
        // …and it spans the whole assembly, like the core.
        let half = SetBBox { max_lon: (WORLD.min_lon + WORLD.max_lon) / 2, ..WORLD };
        assert_eq!(
            build(11, 0, 1, WORLD, [0; SET_ID_LEN], name_field("x"), &[core(WORLD, 1), part(Role::Terrain, half, 1)]),
            Err(ManifestError::Geometry)
        );
    }

    /// A terrain record is invisible to the geometry/coarse tiling proof — it spans the whole
    /// assembly and would otherwise read as an overlapping shard of some role.
    #[test]
    fn terrain_does_not_take_part_in_the_role_tiling() {
        let mid = (WORLD.min_lon + WORLD.max_lon) / 2;
        let manifest = build(
            11,
            0,
            7,
            WORLD,
            [0xA5; SET_ID_LEN],
            name_field("Alpen"),
            &[
                core(WORLD, 1_000),
                part(Role::Coarse, WORLD, 2_000),
                part(Role::Geometry, SetBBox { max_lon: mid, ..WORLD }, 3_000),
                part(Role::Geometry, SetBBox { min_lon: mid, ..WORLD }, 4_000),
                part(Role::Terrain, WORLD, 5_000),
            ],
        )
        .expect("a split set with terrain");
        assert_eq!(manifest.shard_count(), 4);
        assert_eq!(manifest.record_count(), 5);
        assert!(!manifest.is_single_file());
    }

    #[test]
    fn header_bytes_land_on_the_spec_offsets() {
        let manifest = split_set();
        let bytes = encode(&manifest);
        assert_eq!(&bytes[0..4], b"OBCS");
        assert_eq!(bytes[4], 3); // Version
        assert_eq!(bytes[5], crate::obcm::VERSION); // OBCM Version
        assert_eq!(bytes[6], 4); // Shard Count
        assert_eq!(bytes[7], 0); // Core Shard
        assert_eq!(checked_rd_u32(&bytes, 8).unwrap(), 7); // Schema Revision
        assert_eq!(checked_rd_u32(&bytes, 12).unwrap(), 0); // Flags
                                                            // Assembly bbox: lat, lon, lat, lon — the OBCM header order.
        assert_eq!(checked_rd_i32(&bytes, 16).unwrap(), WORLD.min_lat);
        assert_eq!(checked_rd_i32(&bytes, 20).unwrap(), WORLD.min_lon);
        assert_eq!(checked_rd_i32(&bytes, 24).unwrap(), WORLD.max_lat);
        assert_eq!(checked_rd_i32(&bytes, 28).unwrap(), WORLD.max_lon);
        assert_eq!(&bytes[32..48], &[0xA5; 16]);
        assert_eq!(&bytes[48..53], b"Alpen");
        assert_eq!(bytes[53], 0xFF); // name padding
    }

    #[test]
    fn shard_record_bytes_land_on_the_spec_offsets() {
        let manifest = split_set();
        let bytes = encode(&manifest);
        let base = HEADER_LEN + 2 * SHARD_RECORD_LEN;
        assert_eq!(bytes[base], Role::Geometry.id());
        assert_eq!(&bytes[base + 1..base + 4], &[0, 0, 0]); // Flags + Reserved
        assert_eq!(checked_rd_i32(&bytes, base + 4).unwrap(), WORLD.min_lat);
        assert_eq!(checked_rd_u32(&bytes, base + 20).unwrap(), 3_000);
        assert_eq!(shard_digest(&bytes, 2).unwrap(), &[0x11u8; DIGEST_LEN]);
        assert_eq!(shard_digest(&bytes[..manifest.encoded_len()], 4), None);
        // v3's field, at the end of the record and nowhere else — every v2 offset above is
        // unmoved, which is the whole reason it was appended rather than inserted.
        assert_eq!(&bytes[base + MEMBER_ID_OFFSET..base + SHARD_RECORD_LEN], &91u64.to_le_bytes());
    }

    /// The v3 contract end to end: ids survive a round trip, the mount seam hands them back with
    /// the raster held separately, and neither of them is in `Set Id`.
    #[test]
    fn member_ids_round_trip_and_the_mount_seam_separates_the_raster() {
        let mut parts =
            [Shard { object_id: 5, ..core(WORLD, 1) }, Shard { object_id: 6, ..part(Role::Terrain, WORLD, 2) }];
        let bound = build(11, 0, 1, WORLD, [0; SET_ID_LEN], name_field("Grimsel"), &parts).unwrap();
        assert!(bound.is_bound());
        assert!(bound.shard_ids().eq([5u64]), "one OBCM shard, named by its id");
        assert_eq!(bound.terrain_id(), Some(6), "the raster is reached separately, never as a shard");

        let bytes = encode(&bound);
        assert_eq!(parse(&bytes[..bound.encoded_len()]).unwrap(), bound);

        // The same set, unbound: still valid, still the same bytes everywhere but the id fields.
        parts[0].object_id = MEMBER_ID_NONE;
        parts[1].object_id = MEMBER_ID_NONE;
        let unbound = build(11, 0, 1, WORLD, [0; SET_ID_LEN], name_field("Grimsel"), &parts).unwrap();
        assert!(!unbound.is_bound(), "what an assembler writes");
        assert_eq!(unbound.terrain_id(), Some(MEMBER_ID_NONE));
        let mut raw = encode(&unbound);
        let len = unbound.encoded_len();
        assert_eq!(&raw[32..48], &bytes[32..48], "Set Id does not depend on the member ids");

        // Binding writes 8 bytes per record into the staged buffer and moves nothing else.
        bind_member(&mut raw[..len], 0, 5).unwrap();
        bind_member(&mut raw[..len], 1, 6).unwrap();
        assert_eq!(&raw[..len], &bytes[..len], "binding reproduces the manifest byte for byte");
        assert_eq!(parse(&raw[..len]).unwrap(), bound);
    }

    /// Bound or unbound, never in between, and never two records naming one object.
    #[test]
    fn half_bound_and_duplicate_member_ids_are_refused() {
        let half = [Shard { object_id: 7, ..core(WORLD, 1) }, part(Role::Terrain, WORLD, 2)];
        assert_eq!(
            build(11, 0, 1, WORLD, [0; SET_ID_LEN], name_field("x"), &half),
            Err(ManifestError::Members),
            "a client that died mid-bind must not leave a mountable set"
        );
        let twice = [Shard { object_id: 7, ..core(WORLD, 1) }, Shard { object_id: 7, ..part(Role::Terrain, WORLD, 2) }];
        assert_eq!(build(11, 0, 1, WORLD, [0; SET_ID_LEN], name_field("x"), &twice), Err(ManifestError::Members));

        // …and both are caught on the wire, not only through `build`.
        let manifest = split_set();
        let len = manifest.encoded_len();
        let mut bad = encode(&manifest);
        bad[HEADER_LEN + MEMBER_ID_OFFSET..HEADER_LEN + SHARD_RECORD_LEN].copy_from_slice(&0u64.to_le_bytes());
        assert_eq!(parse(&bad[..len]), Err(ManifestError::Members));
        let mut bad = encode(&manifest);
        let second = HEADER_LEN + SHARD_RECORD_LEN + MEMBER_ID_OFFSET;
        bad[second..second + 8].copy_from_slice(&90u64.to_le_bytes()); // the core's id, again
        assert_eq!(parse(&bad[..len]), Err(ManifestError::Members));

        // Non-ascending ids are legal: a set that reuses a shard already on the card carries an
        // older id beside newer ones, which is the dedup working.
        assert!(split_set().shard_ids().eq([90u64, 12, 91, 92]));
    }

    /// `bind_member` refuses everything that would leave bytes a parse must then reject.
    #[test]
    fn binding_is_range_checked_and_never_unbinds() {
        let manifest = split_set();
        let len = manifest.encoded_len();
        let mut bytes = encode(&manifest);
        let raw = &mut bytes[..len];
        assert_eq!(bind_member(raw, 0, MEMBER_ID_NONE), Err(ManifestError::Members), "unbinding is not binding");
        assert_eq!(bind_member(raw, 4, 1), Err(ManifestError::ShardCount), "index == Shard Count");
        assert_eq!(bind_member(raw, 255, 1), Err(ManifestError::ShardCount));
        assert_eq!(bind_member(&mut [], 0, 1), Err(ManifestError::Layout));

        let mut short = [0u8; HEADER_LEN];
        short[..4].copy_from_slice(&MAGIC);
        short[4] = VERSION;
        short[6] = 1;
        assert_eq!(bind_member(&mut short, 0, 1), Err(ManifestError::Length), "a header with no record");

        let mut v2 = bytes;
        v2[4] = 2;
        assert_eq!(bind_member(&mut v2[..len], 0, 1), Err(ManifestError::Version));
    }

    #[test]
    fn round_trip_preserves_every_field() {
        let manifest = split_set();
        let bytes = encode(&manifest);
        let parsed = parse(&bytes[..manifest.encoded_len()]).unwrap();
        assert_eq!(parsed, manifest);
        assert_eq!(parsed.name(), Some("Alpen"));
        assert_eq!(parsed.total_bytes(), 10_000);
        assert_eq!(parsed.core().role, Role::Core);
        assert!(!parsed.is_single_file());
    }

    #[test]
    fn the_single_file_fast_path_is_a_set_of_one() {
        let manifest = build(11, 0, 1, WORLD, [0; SET_ID_LEN], name_field("Solo"), &[core(WORLD, 42)]).unwrap();
        assert!(manifest.is_single_file());
        let bytes = encode(&manifest);
        let parsed = parse(&bytes[..manifest_len(1)]).unwrap();
        assert_eq!(parsed.shard_count(), 1);
        assert_eq!(parsed.core().bytes, 42);
    }

    #[test]
    fn malformed_prefixes_are_rejected() {
        let manifest = split_set();
        let good = encode(&manifest);
        let len = manifest.encoded_len();
        assert_eq!(parse(&good[..4]), Err(ManifestError::Layout));

        let mut bad = good;
        bad[0] = b'X';
        assert_eq!(parse(&bad[..len]), Err(ManifestError::Layout));

        // Every retired layout and every future one, refused by the version byte alone. `2` is the
        // one that matters: its records are 56 bytes, so a reader that guessed would find the
        // second record's role eight bytes early and the last record past the end.
        for retired in [1u8, 2, 4, 255] {
            let mut bad = good;
            bad[4] = retired;
            assert_eq!(parse(&bad[..len]), Err(ManifestError::Version), "version {retired}");
        }

        let mut bad = good;
        bad[12] = 1; // non-zero reserved Flags
        assert_eq!(parse(&bad[..len]), Err(ManifestError::Layout));

        // Length must be exactly `72 + 64 × Shard Count`.
        assert_eq!(parse(&good[..len - 1]), Err(ManifestError::Length));
        assert_eq!(parse(&good[..len + 1]), Err(ManifestError::Length));
    }

    /// Every prefix of a valid manifest must come back as a typed error. The 5-, 6- and 7-byte
    /// cases are the ones `validate_prefix` does *not* cover — it guarantees five bytes, and the
    /// header reads `OBCM Version`, `Shard Count` and `Core Shard` above that. On the device this
    /// buffer comes off an SD card, where a short read is the ordinary shape of a torn write, and a
    /// `no_std` panic is a reset on every boot.
    #[test]
    fn every_truncation_is_an_error_and_never_a_panic() {
        let manifest = split_set();
        let good = encode(&manifest);
        for len in 0..=manifest.encoded_len() + 8 {
            let parsed = parse(&good[..len.min(good.len())]);
            if len == manifest.encoded_len() {
                assert!(parsed.is_ok(), "the exact length parses");
            } else {
                assert!(parsed.is_err(), "length {len} must be rejected, not accepted");
            }
        }
        // Named explicitly, because these three are the reported crash and a range test could
        // drift past them silently.
        assert_eq!(parse(&good[..5]), Err(ManifestError::Layout));
        assert_eq!(parse(&good[..6]), Err(ManifestError::Layout));
        assert_eq!(parse(&good[..7]), Err(ManifestError::Length));

        // A header that claims one shard but carries no record: the length check catches it before
        // any record read, and `Core Shard` is never indexed against a record that is not there.
        let mut header = [0u8; HEADER_LEN];
        header[..4].copy_from_slice(&MAGIC);
        header[4] = VERSION;
        header[6] = 1;
        assert_eq!(parse(&header), Err(ManifestError::Length));
    }

    /// `Core Shard` can no longer be moved out of range after construction — it is private, and
    /// both constructors range-check it. The reachable mutation is a *public* field, and `validate`
    /// must answer it with an error rather than an index panic.
    #[test]
    fn an_out_of_range_core_index_cannot_outlive_construction() {
        let manifest = split_set();
        assert_eq!(manifest.core_shard(), 0);
        assert_eq!(manifest.core().role, Role::Core);
        // Every rejected core index, through both constructors.
        for bad in [4u8, 32, 255] {
            let mut bytes = encode(&manifest);
            bytes[7] = bad;
            assert_eq!(parse(&bytes[..manifest.encoded_len()]), Err(ManifestError::ShardCount));
        }
        assert_eq!(
            build(11, 9, 1, WORLD, [0; SET_ID_LEN], name_field("x"), &[core(WORLD, 1)]),
            Err(ManifestError::ShardCount)
        );
        // The pub fields that *are* mutable post-parse still go through `validate` without panicking.
        let mut mutated = manifest;
        mutated.bbox = SetBBox { min_lat: 1, min_lon: 1, max_lat: 0, max_lon: 0 };
        assert_eq!(validate(&mutated), Err(ManifestError::Geometry));
    }

    /// A zero-area box is invisible to both halves of the tiling proof — it overlaps nothing and
    /// adds nothing to the area sum — so it would let a "tiling" hide any number of empty shards.
    #[test]
    fn degenerate_boxes_are_refused() {
        let line = SetBBox { max_lat: WORLD.min_lat, ..WORLD };
        assert_eq!(line.area(), 0);
        assert!(line.is_ordered(), "ordered but degenerate is exactly the gap");

        // A degenerate assembly.
        assert_eq!(
            build(11, 0, 1, line, [0; SET_ID_LEN], name_field("x"), &[core(line, 1)]),
            Err(ManifestError::Geometry)
        );
        // A degenerate geometry shard riding along with one that really does tile the assembly.
        assert_eq!(
            build(
                11,
                0,
                1,
                WORLD,
                [0; SET_ID_LEN],
                name_field("x"),
                &[
                    core(WORLD, 1),
                    part(Role::Coarse, WORLD, 1),
                    part(Role::Geometry, WORLD, 1),
                    part(Role::Geometry, line, 1),
                ],
            ),
            Err(ManifestError::Geometry)
        );
    }

    /// §5.4's delete ordering, asserted where tests run: the manifest first (it is the atomicity
    /// token), then every derived shard name to the cap so a smaller replacement still sweeps the
    /// old set's tail.
    #[test]
    fn the_delete_plan_removes_the_manifest_first_then_the_whole_tail() {
        let plan = delete_plan(7).expect("set 7 has derived names");
        assert_eq!(plan.len(), DELETE_PLAN_LEN);
        assert_eq!(plan.len(), MAX_SHARDS + 2);
        assert_eq!(plan[0].as_str(), "MS7.OBS", "the manifest goes first (§5.4)");
        assert_eq!(plan[1].as_str(), "MS7S00.OBM");
        assert_eq!(plan[MAX_SHARDS].as_str(), "MS7S31.OBM", "the sweep runs to the cap, not the set's count");
        assert_eq!(plan[MAX_SHARDS + 1].as_str(), "MS7.OBD", "…and the raster goes too (§5.4's orphan rule)");
        for (index, name) in plan[1..=MAX_SHARDS].iter().enumerate() {
            assert_eq!(parse_shard_name(name.as_str().as_bytes()), Some((7, index)));
        }
        assert!(delete_plan(1000).is_none(), "an id with no derived name has no plan");
    }

    #[test]
    fn shard_count_and_core_index_are_range_checked() {
        let manifest = split_set();
        let good = encode(&manifest);
        let len = manifest.encoded_len();

        let mut bad = good;
        bad[6] = 0;
        assert_eq!(parse(&bad[..len]), Err(ManifestError::ShardCount));

        let mut bad = good;
        bad[6] = 33;
        assert_eq!(parse(&bad[..len]), Err(ManifestError::ShardCount));

        let mut bad = good;
        bad[7] = 4; // core index == shard count
        assert_eq!(parse(&bad[..len]), Err(ManifestError::ShardCount));
    }

    #[test]
    fn role_rules_reject_zero_two_and_missing_roles() {
        let manifest = split_set();
        let good = encode(&manifest);
        let len = manifest.encoded_len();

        let mut bad = good;
        bad[HEADER_LEN + SHARD_RECORD_LEN] = 4; // unknown role byte
        assert_eq!(parse(&bad[..len]), Err(ManifestError::Roles));

        let mut bad = good;
        bad[HEADER_LEN + SHARD_RECORD_LEN] = Role::Terrain.id(); // terrain, but not last
        assert_eq!(parse(&bad[..len]), Err(ManifestError::Roles));

        let mut bad = good;
        bad[HEADER_LEN + SHARD_RECORD_LEN] = Role::Core.id(); // a second core
        assert_eq!(parse(&bad[..len]), Err(ManifestError::Roles));

        // A multi-shard set with no coarse shard is a map missing whole zoom levels.
        let mid = (WORLD.min_lon + WORLD.max_lon) / 2;
        assert_eq!(
            build(
                11,
                0,
                1,
                WORLD,
                [0; SET_ID_LEN],
                name_field("No coarse"),
                &[
                    core(WORLD, 1),
                    part(Role::Geometry, SetBBox { max_lon: mid, ..WORLD }, 1),
                    part(Role::Geometry, SetBBox { min_lon: mid, ..WORLD }, 1),
                ],
            ),
            Err(ManifestError::Roles)
        );
    }

    #[test]
    fn geometry_rules_reject_bad_boxes_overlaps_and_gaps() {
        let mid = (WORLD.min_lon + WORLD.max_lon) / 2;
        let west = SetBBox { max_lon: mid, ..WORLD };
        let east = SetBBox { min_lon: mid, ..WORLD };
        let coarse = part(Role::Coarse, WORLD, 1);
        let make = |parts: &[Shard]| build(11, 0, 1, WORLD, [0; SET_ID_LEN], name_field("x"), parts);

        // Core bbox must equal the assembly bbox.
        assert_eq!(make(&[core(west, 1), coarse, part(Role::Geometry, WORLD, 1)]), Err(ManifestError::Geometry));
        // A shard outside the assembly bbox.
        let outside = SetBBox { max_lon: WORLD.max_lon + 1, ..WORLD };
        assert_eq!(make(&[core(WORLD, 1), coarse, part(Role::Geometry, outside, 1)]), Err(ManifestError::Geometry));
        // Overlapping geometry shards.
        let wide = SetBBox { max_lon: mid + 1, ..WORLD };
        assert_eq!(
            make(&[core(WORLD, 1), coarse, part(Role::Geometry, wide, 1), part(Role::Geometry, east, 1),]),
            Err(ManifestError::Geometry)
        );
        // A gap: the two halves do not cover the assembly.
        assert_eq!(make(&[core(WORLD, 1), coarse, part(Role::Geometry, west, 1)]), Err(ManifestError::Geometry));
        // Abutting halves are legal — an antichain shares edges, not interiors.
        assert!(make(&[core(WORLD, 1), coarse, part(Role::Geometry, west, 1), part(Role::Geometry, east, 1),]).is_ok());
        // An inverted assembly bbox.
        let flipped = SetBBox { min_lat: WORLD.max_lat, max_lat: WORLD.min_lat, ..WORLD };
        assert_eq!(
            build(11, 0, 1, flipped, [0; SET_ID_LEN], name_field("x"), &[core(flipped, 1)]),
            Err(ManifestError::Geometry)
        );
    }

    #[test]
    fn derived_filenames_are_eight_three_safe() {
        assert_eq!(manifest_name(7).unwrap().as_str(), "MS7.OBS");
        assert_eq!(manifest_name(999).unwrap().as_str(), "MS999.OBS");
        assert_eq!(manifest_name(1000), None);
        assert_eq!(shard_name(7, 0).unwrap().as_str(), "MS7S00.OBM");
        assert_eq!(shard_name(999, 31).unwrap().as_str(), "MS999S31.OBM");
        assert_eq!(shard_name(999, 32), None);
        for id in [0u16, 9, 10, 999] {
            for index in 0..MAX_SHARDS {
                let name = shard_name(id, index).unwrap();
                let text = name.as_str();
                let (stem, ext) = text.split_at(text.len() - 4);
                assert_eq!(ext, ".OBM");
                assert!(stem.len() <= 8, "{text} is not 8.3-safe");
                assert_eq!(parse_shard_name(text.as_bytes()), Some((id, index)));
            }
            assert_eq!(parse_manifest_name(manifest_name(id).unwrap().as_str().as_bytes()), Some(id));
        }
    }

    #[test]
    fn filename_parsers_reject_neighbouring_conventions() {
        // The legacy single-map convention must never parse as a set.
        assert_eq!(parse_manifest_name(b"MP7.OBM"), None);
        assert_eq!(parse_shard_name(b"MP7.OBM"), None);
        assert_eq!(parse_manifest_name(b"MS7.OBM"), None);
        assert_eq!(parse_shard_name(b"MS7.OBS"), None);
        assert_eq!(parse_shard_name(b"MS7S32.OBM"), None);
        assert_eq!(parse_shard_name(b"MS7S0.OBM"), None);
        assert_eq!(parse_manifest_name(b"MS07.OBS"), None);
        assert_eq!(parse_manifest_name(b"ms7.obs"), None);
        assert_eq!(parse_manifest_name(b"MS.OBS"), None);
        assert_eq!(parse_manifest_name(b"MS1234.OBS"), None);
    }

    #[test]
    fn bbox_predicates_pin_dispatch_semantics() {
        let mid = (WORLD.min_lon + WORLD.max_lon) / 2;
        let west = SetBBox { max_lon: mid, ..WORLD };
        let east = SetBBox { min_lon: mid, ..WORLD };
        assert!(!west.overlaps_interior(&east));
        // A viewport touching the seam dispatches to *both* halves — closed intersection.
        let seam = SetBBox { min_lat: 47_500_000, min_lon: mid, max_lat: 47_500_001, max_lon: mid };
        assert!(west.intersects(&seam) && east.intersects(&seam));
        assert_eq!(west.area() + east.area(), WORLD.area());
    }
}
