//! OBCS volume-set manifest codec from `OBCA_Spec.md` §5.2 / §5.3.
//!
//! One *logical* map is a **set**: this small manifest plus `1..=32` physical OBCM files
//! (§5). The manifest is parsed on the device, so the layout is fixed, little-endian, and
//! needs no allocation — a parsed [`SetManifest`] is a plain value with a fixed-capacity
//! shard array (24 B per shard resident; the 32-byte digests stay in the caller's byte
//! buffer and are reached through [`shard_digest`], because a device defers hashing).
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
pub const VERSION: u8 = 1;
/// Fixed manifest header width (§5.2).
pub const HEADER_LEN: usize = 72;
/// Fixed per-shard record width (§5.2).
pub const SHARD_RECORD_LEN: usize = 56;
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
/// Longest derived manifest filename, `MS999.OBS` (§5.2).
pub const MAX_MANIFEST_NAME_LEN: usize = 9;
/// The filename prefix that marks a volume set, kept clear of single-map `MP<id>.OBM` (§5.2).
pub const SET_PREFIX: &str = "MS";
/// The manifest extension (§5.2).
pub const MANIFEST_EXT: &str = ".OBS";
/// The shard extension — the same one a single received map uses (§5.2).
pub const SHARD_EXT: &str = ".OBM";
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

/// One shard as the device keeps it resident: role, bbox, size. The 32-byte digest is
/// deliberately *not* here — §5.3 lets a device defer the SHA-256 check, so keeping it would
/// be 1 KiB of RAM nothing reads. Hosts reach it with [`shard_digest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shard {
    pub role: Role,
    pub bbox: SetBBox,
    /// Shard size in bytes. `uint32` is exactly right: the FAT32 ceiling is `4 GiB − 1`.
    pub bytes: u32,
}

impl Default for Shard {
    fn default() -> Self {
        Shard { role: Role::Core, bbox: SetBBox::default(), bytes: 0 }
    }
}

/// A parsed, §5.3-validated set manifest.
///
/// Resident cost is `72 + 24 × 32 ≈ 840 B` at full capacity — the fixed array avoids both an
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
    shard_count: u8,
    shards: [Shard; MAX_SHARDS],
}

impl SetManifest {
    /// The shards, in index order — the order the derived filenames count in.
    #[inline]
    pub fn shards(&self) -> &[Shard] {
        &self.shards[..self.shard_count as usize]
    }

    #[inline]
    pub fn shard_count(&self) -> usize {
        self.shard_count as usize
    }

    /// Index of the core shard (§5.1); always `< shard_count`, by construction.
    #[inline]
    pub fn core_shard(&self) -> usize {
        self.core_shard as usize
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

    /// Whether this is the single-file fast path (§5.5): one shard, which is the core and
    /// carries everything.
    #[inline]
    pub fn is_single_file(&self) -> bool {
        self.shard_count == 1
    }

    /// Total bytes of the set — the only size figure a UI may show (§5.4).
    pub fn total_bytes(&self) -> u64 {
        self.shards().iter().map(|shard| shard.bytes as u64).sum()
    }

    /// Serialized length of this manifest.
    #[inline]
    pub fn encoded_len(&self) -> usize {
        manifest_len(self.shard_count as usize)
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
    /// Length is not exactly `72 + 56 × Shard Count`.
    Length,
    /// `Shard Count` outside `1..=32`, or `Core Shard` out of range.
    ShardCount,
    /// A role byte outside `0..=2`, no core, more than one core, or a role the schema's bands
    /// name with no shard at all.
    Roles,
    /// A bbox with `min > max`, a shard outside the assembly, a core whose bbox is not the
    /// assembly bbox, or a role whose shards do not tile the assembly bbox.
    Geometry,
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
    let shard_count = byte_at(bytes, 6)? as usize;
    if shard_count == 0 || shard_count > MAX_SHARDS {
        return Err(ManifestError::ShardCount);
    }
    if bytes.len() != manifest_len(shard_count) {
        return Err(ManifestError::Length);
    }
    let core_shard = byte_at(bytes, 7)?;
    if core_shard as usize >= shard_count {
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
    for (index, shard) in shards.iter_mut().enumerate().take(shard_count) {
        let base = HEADER_LEN + index * SHARD_RECORD_LEN;
        let role = Role::from_id(byte_at(bytes, base)?).ok_or(ManifestError::Roles)?;
        if array_at::<3>(bytes, base + 1)? != [0, 0, 0] {
            return Err(ManifestError::Layout);
        }
        *shard = Shard { role, bbox: read_bbox(bytes, base + 4)?, bytes: checked_rd_u32(bytes, base + 20)? };
    }

    let manifest = SetManifest {
        obcm_version,
        core_shard,
        schema_revision,
        bbox,
        set_id,
        name,
        shard_count: shard_count as u8,
        shards,
    };
    validate(&manifest)?;
    Ok(manifest)
}

/// The §5.3 semantic rules, over an already-decoded manifest. Split out so a host assembler
/// can check a manifest it is about to write without serializing it first.
pub fn validate(manifest: &SetManifest) -> Result<(), ManifestError> {
    let shards = manifest.shards();
    let core_index = manifest.core_shard as usize;
    // Bounds-checked rather than indexed: `validate` is the *authority*, so it may not assume the
    // invariant it exists to establish.
    let core = shards.get(core_index).ok_or(ManifestError::ShardCount)?;

    // Exactly one core, and it is the one the header names.
    let cores = shards.iter().filter(|shard| shard.role == Role::Core).count();
    if cores != 1 || core.role != Role::Core {
        return Err(ManifestError::Roles);
    }
    // A role with no shard is a map missing whole zoom levels — unless this is the §5.5
    // single-file fast path, where the one shard is the core and carries everything.
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
    for shard in shards {
        if !shard.bbox.is_ordered() || shard.bbox.area() == 0 || !shard.bbox.is_inside(&manifest.bbox) {
            return Err(ManifestError::Geometry);
        }
    }
    if core.bbox != manifest.bbox {
        return Err(ManifestError::Geometry);
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
    let shards = manifest.shards();
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
    }
    Ok(len)
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
        shard_count: parts.len() as u8,
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

    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len as usize]
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
    Some(plan)
}

/// Length of [`delete_plan`]'s list: the manifest plus every shard name the cap can express.
pub const DELETE_PLAN_LEN: usize = MAX_SHARDS + 1;

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
const _: () = assert!(SHARD_RECORD_LEN == 1 + 1 + 2 + 16 + 4 + DIGEST_LEN);
const _: () = assert!(MAX_MANIFEST_LEN == 1864);

#[cfg(test)]
mod tests {
    use super::*;

    const WORLD: SetBBox = SetBBox { min_lat: 47_000_000, min_lon: 7_000_000, max_lat: 48_000_000, max_lon: 9_000_000 };

    fn name_field(text: &str) -> [u8; NAME_LEN] {
        let mut out = [0xFFu8; NAME_LEN];
        out[..text.len()].copy_from_slice(text.as_bytes());
        out
    }

    fn core(bbox: SetBBox, bytes: u32) -> Shard {
        Shard { role: Role::Core, bbox, bytes }
    }

    /// A three-shard set: core + coarse over the whole box, two geometry halves that tile it.
    fn split_set() -> SetManifest {
        let mid = (WORLD.min_lon + WORLD.max_lon) / 2;
        let west = SetBBox { max_lon: mid, ..WORLD };
        let east = SetBBox { min_lon: mid, ..WORLD };
        build(
            11,
            0,
            7,
            WORLD,
            [0xA5; SET_ID_LEN],
            name_field("Alpen"),
            &[
                core(WORLD, 1_000),
                Shard { role: Role::Coarse, bbox: WORLD, bytes: 2_000 },
                Shard { role: Role::Geometry, bbox: west, bytes: 3_000 },
                Shard { role: Role::Geometry, bbox: east, bytes: 4_000 },
            ],
        )
        .unwrap()
    }

    fn encode(manifest: &SetManifest) -> [u8; MAX_MANIFEST_LEN] {
        let digests = [[0x11u8; DIGEST_LEN]; MAX_SHARDS];
        let mut out = [0u8; MAX_MANIFEST_LEN];
        let len = serialize(manifest, &digests[..manifest.shard_count()], &mut out).unwrap();
        assert_eq!(len, manifest.encoded_len());
        out
    }

    #[test]
    fn record_widths_pin_spec_arithmetic() {
        assert_eq!(HEADER_LEN, 72);
        assert_eq!(SHARD_RECORD_LEN, 56);
        assert_eq!(manifest_len(1), 128);
        assert_eq!(manifest_len(32), MAX_MANIFEST_LEN);
        assert_eq!(Role::Core.id(), 0);
        assert_eq!(Role::Geometry.id(), 1);
        assert_eq!(Role::Coarse.id(), 2);
        assert_eq!(Role::from_id(3), None);
    }

    #[test]
    fn header_bytes_land_on_the_spec_offsets() {
        let manifest = split_set();
        let bytes = encode(&manifest);
        assert_eq!(&bytes[0..4], b"OBCS");
        assert_eq!(bytes[4], 1); // Version
        assert_eq!(bytes[5], 11); // OBCM Version
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

        let mut bad = good;
        bad[4] = 2;
        assert_eq!(parse(&bad[..len]), Err(ManifestError::Version));

        let mut bad = good;
        bad[12] = 1; // non-zero reserved Flags
        assert_eq!(parse(&bad[..len]), Err(ManifestError::Layout));

        // Length must be exactly `72 + 56 × Shard Count`.
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
                    Shard { role: Role::Coarse, bbox: WORLD, bytes: 1 },
                    Shard { role: Role::Geometry, bbox: WORLD, bytes: 1 },
                    Shard { role: Role::Geometry, bbox: line, bytes: 1 },
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
        assert_eq!(plan.len(), MAX_SHARDS + 1);
        assert_eq!(plan[0].as_str(), "MS7.OBS", "the manifest goes first (§5.4)");
        assert_eq!(plan[1].as_str(), "MS7S00.OBM");
        assert_eq!(plan[MAX_SHARDS].as_str(), "MS7S31.OBM", "the sweep runs to the cap, not the set's count");
        for (index, name) in plan[1..].iter().enumerate() {
            assert_eq!(parse_shard_name(name.as_bytes()), Some((7, index)));
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
        bad[HEADER_LEN + SHARD_RECORD_LEN] = 3; // unknown role byte
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
                    Shard { role: Role::Geometry, bbox: SetBBox { max_lon: mid, ..WORLD }, bytes: 1 },
                    Shard { role: Role::Geometry, bbox: SetBBox { min_lon: mid, ..WORLD }, bytes: 1 },
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
        let coarse = Shard { role: Role::Coarse, bbox: WORLD, bytes: 1 };
        let make = |parts: &[Shard]| build(11, 0, 1, WORLD, [0; SET_ID_LEN], name_field("x"), parts);

        // Core bbox must equal the assembly bbox.
        assert_eq!(
            make(&[core(west, 1), coarse, Shard { role: Role::Geometry, bbox: WORLD, bytes: 1 }]),
            Err(ManifestError::Geometry)
        );
        // A shard outside the assembly bbox.
        let outside = SetBBox { max_lon: WORLD.max_lon + 1, ..WORLD };
        assert_eq!(
            make(&[core(WORLD, 1), coarse, Shard { role: Role::Geometry, bbox: outside, bytes: 1 }]),
            Err(ManifestError::Geometry)
        );
        // Overlapping geometry shards.
        let wide = SetBBox { max_lon: mid + 1, ..WORLD };
        assert_eq!(
            make(&[
                core(WORLD, 1),
                coarse,
                Shard { role: Role::Geometry, bbox: wide, bytes: 1 },
                Shard { role: Role::Geometry, bbox: east, bytes: 1 },
            ]),
            Err(ManifestError::Geometry)
        );
        // A gap: the two halves do not cover the assembly.
        assert_eq!(
            make(&[core(WORLD, 1), coarse, Shard { role: Role::Geometry, bbox: west, bytes: 1 }]),
            Err(ManifestError::Geometry)
        );
        // Abutting halves are legal — an antichain shares edges, not interiors.
        assert!(make(&[
            core(WORLD, 1),
            coarse,
            Shard { role: Role::Geometry, bbox: west, bytes: 1 },
            Shard { role: Role::Geometry, bbox: east, bytes: 1 },
        ])
        .is_ok());
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
                assert_eq!(parse_shard_name(name.as_bytes()), Some((id, index)));
            }
            assert_eq!(parse_manifest_name(manifest_name(id).unwrap().as_bytes()), Some(id));
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
