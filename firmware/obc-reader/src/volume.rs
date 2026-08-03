//! Volume sets: one logical map spread over 1..32 OBCM shards (`OBCA_Spec.md` §5).
//!
//! A [`MountedSet`] is a [`MapScene`] like a single [`Reader`] is, so the renderer needs no
//! changes: a viewport query fans out to every shard whose bbox intersects the view, and the
//! §5.6 mount-time empty-LOD cache means a shard that does not carry the requested LOD costs
//! **no I/O** to skip. Nav and POI queries never fan out — they always go to the core shard
//! (§5.1), which is why routing never crosses a file.
//!
//! # What is resident, and what is not
//!
//! The expensive part of a parsed map is the 256-slot style table, and §4.7 stamps the *same*
//! skin into every shard of a set. So a mount keeps exactly one [`MapTables`] — the core's —
//! and holds each extra shard as a [`ShardTables`]: its header bbox plus its LOD table plus the
//! empty-LOD bitmask. [`ShardTables::parse`] never touches the style region, so mounting a shard
//! also avoids `parse_styles`'s 2 KiB stack scratch; the whole mount path stays shallow.
//!
//! The ≈277 KB [`MapCache`] is shared by the whole set. That is safe because every per-shard
//! reader borrows the **core's** [`MapTables`] — so the whole set presents one parse generation
//! and no shard clears the cache the previous one filled — while the reader tags every cache key
//! with the shard index, so no shard is served another's chunks.
//!
//! # Where the shard table lives is the caller's decision, not this module's
//!
//! The per-shard records are the only part of a mount that is *big*: [`ShardTables`] is 408 B on
//! the device (a `heapless::Vec<Lod, 16>` is at capacity whatever the ladder's length — the
//! format's 16-rung maximum is what the type reserves), a [`Mounted`] record 440 B, and the
//! 32-shard array therefore **14,084 B**. That must not travel through a caller's frame: an
//! embassy task allocates every local at entry and keeps it for the task's life (#270), so a set
//! mounted into a local would cost the task frame 14 KB even while nothing is mounted, against a
//! ~36 KB stack.
//!
//! So the array is [`SetShards`], which the caller places — a `static`/`.bss` cell on the device,
//! an ordinary local on a host — and [`MountedSet::mount`] fills **in place**. What comes back is
//! four machine words plus a compact core index ([`MountedSet`] is 20 B on the device), and the
//! const assertions at the bottom of this file pin every one of those numbers on both targets.
//! The parsed manifest is needed only while validating the mount; it is not retained afterwards.
//!
//! [`SetShards`] is generic over how many shards it can hold, which is how a device states its own
//! ceiling: the manifest's cap is 32, but a board can only mount as many shards as it has FAT file
//! handles for, and a set past that is refused at mount with [`MountError::Handles`] naming the cap
//! rather than discovered as a failed open halfway through a ride.

use heapless::Vec;
use obc_formats::io::ByteSource;
use obc_formats::obcs::{ManifestError, Role, SetBBox, SetManifest, MAX_SHARDS};
use obc_map_scene::{
    BBox, Candidate, CandidateReport, DecodeReport, Diagnostics, FeatureToken, MapScene, ReadError as SceneReadError,
    SelectedFeatures, Style,
};

use crate::reader::{parse_header, parse_lod_table, Lod};
use crate::{Error, MapCache, MapTables, Reader};

/// Highest shard index a [`FeatureToken`] can carry — the five bits stolen from the token's
/// chunk-id high word (see [`tag_token`]). Exactly the §5.2 shard cap.
const MAX_TOKEN_SHARD: usize = MAX_SHARDS - 1;
/// Chunk ids a tagged token can still express. Five of the token's 32 chunk-id bits go to the
/// shard index; 2^27 chunks at the packer's 4 KiB default is 512 GiB of one LOD, three orders
/// of magnitude past the `4 GiB − 1` per-file ceiling §5 exists to respect.
///
/// Checked at **runtime**, not only in debug: an id this large cannot come from a legal file, so
/// it comes from a corrupt one, and silently truncating it would serve the wrong chunk's bytes as
/// map ink. [`MountedSet::visit_candidates`] drops such a chunk and counts it malformed.
const MAX_TOKEN_CHUNK_ID: u32 = (1 << 27) - 1;

/// Why a set did not mount. §5.4 admits no partial mount: a set that fails any of these is
/// *no set at all*, and its shards MUST NOT be offered as standalone maps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountError {
    /// The manifest itself failed §5.3.
    Manifest(ManifestError),
    /// The caller supplied a different number of sources than the manifest names — the shape a
    /// mid-copy set takes when a shard file is simply not there yet.
    ShardCount,
    /// The set names more shards than this caller's [`SetShards`] can hold. Carries the caller's
    /// own cap, because the number a rider needs to hear is "this device mounts N", not "the
    /// format allows 32".
    Handles(u8),
    /// Shard `index` is not the recorded `Bytes`. A shard still being written is the common
    /// case, and it reads as *map incomplete*, never as a smaller map.
    Size(u8),
    /// Shard `index` does not open as OBCM at the manifest's `OBCM Version`.
    Header(u8),
    /// Shard `index`'s OBCM header bbox is not the bbox the manifest records for it.
    Bbox(u8),
    /// Shard `index`'s LOD ladder is not the core's. §5.1 requires every shard to list the **full
    /// ladder** with the rungs it does not carry written empty, and dispatch indexes the *core*'s
    /// chosen LOD into each shard's own table — so a shard with a shorter or differently-ordered
    /// ladder reads the wrong rung rather than reading nothing.
    Ladder(u8),
    /// Shard `index` carries a different style table than the core. Shards of one set are
    /// stamped from one skin (§4.7), so a mismatch means these files are not one map. Also
    /// reported when the comparison itself could not read the bytes.
    Styles(u8),
}

impl From<ManifestError> for MountError {
    fn from(error: ManifestError) -> MountError {
        MountError::Manifest(error)
    }
}

/// Everything a non-core shard needs resident: its header bbox, its OBCM version, its LOD table,
/// and the §5.6 empty-LOD predicate. No style table, no POI directory, no nav directory — a
/// geometry or coarse shard has none of those (§5.1), and the skin is the core's.
pub struct ShardTables {
    bbox: BBox,
    version: u8,
    lods: Vec<Lod, 16>,
    /// Bit `k` set ⇔ LOD `k` has `Index Node Count == 0`. Seven bits at the v1 ladder (§5.6);
    /// a `u16` covers the format's 16-LOD maximum and still costs two bytes per file.
    empty: u16,
}

impl ShardTables {
    /// Parse a shard's header + LOD table, and nothing else.
    pub fn parse(src: &dyn ByteSource) -> Result<ShardTables, Error> {
        let total = src.len() as usize;
        if total < obc_formats::obcm::HEADER_LEN {
            return Err(Error::TooShort);
        }
        let mut header = [0u8; obc_formats::obcm::HEADER_LEN];
        src.read_at(0, &mut header).map_err(Error::Source)?;
        let parsed = parse_header(&header)?;
        let lod_count = header[25] as usize;
        let lod_table_offset = obc_formats::io::rd_u32(&header, 26) as usize;
        if lod_count == 0 {
            return Err(Error::BadOffset);
        }
        let lod_table_end = lod_count
            .checked_mul(obc_formats::obcm::LOD_ENTRY_LEN)
            .and_then(|len| lod_table_offset.checked_add(len))
            .ok_or(Error::BadOffset)?;
        if lod_table_end > total {
            return Err(Error::BadOffset);
        }
        let lods = parse_lod_table(src, lod_table_offset, lod_count, total)?;
        Ok(ShardTables { bbox: parsed.bbox, version: parsed.version, empty: empty_mask(&lods), lods })
    }

    #[inline]
    pub fn bbox(&self) -> BBox {
        self.bbox
    }

    /// The OBCM version from this shard's own header byte 4 — checked at mount against the
    /// manifest's `OBCM Version`, which §5.3 pins equal across the whole set.
    #[inline]
    pub fn version(&self) -> u8 {
        self.version
    }

    #[inline]
    pub fn lods(&self) -> &[Lod] {
        &self.lods
    }

    /// The §5.6 predicate: this file writes LOD `lod` empty, so a query for it can be skipped
    /// with **no I/O**. It is not a statement about band membership or role (§3.1: a
    /// legitimately empty cell is indistinguishable from an out-of-band one).
    #[inline]
    pub fn lod_is_empty(&self, lod: usize) -> bool {
        lod >= self.lods.len() || self.empty & (1 << lod.min(15)) != 0
    }
}

fn empty_mask(lods: &[Lod]) -> u16 {
    let mut mask = 0u16;
    for (index, lod) in lods.iter().enumerate().take(16) {
        if lod.node_count == 0 {
            mask |= 1 << index;
        }
    }
    mask
}

/// Whether `shard` lists the same ladder as the core: the same number of rungs, each with the same
/// `max_mpp`. §5.1 makes that a producer obligation, and dispatch depends on it — the core picks
/// the LOD index and every shard is asked for *that* index, so a shard whose rung `k` means a
/// different scale answers the wrong question with a perfectly valid-looking read.
fn ladder_matches(core: &[Lod], shard: &[Lod]) -> bool {
    core.len() == shard.len()
        && core.iter().zip(shard).all(|(a, b)| a.max_mpp == b.max_mpp || (a.max_mpp.is_nan() && b.max_mpp.is_nan()))
}

/// One mounted shard: where its bytes are, what the manifest says about it, and the tables the
/// dispatch loop needs.
struct Mounted<'a> {
    src: &'a dyn ByteSource,
    bbox: SetBBox,
    role: Role,
    /// `None` for the core shard, whose tables are the set's shared [`MapTables`].
    tables: Option<ShardTables>,
}

/// The per-shard records of a mount, in the storage the **caller** chose.
///
/// This is the 14 KB of a mounted set (see the module docs): a device declares one `static` of
/// these in `.bss` and mounts into it, so nothing large ever crosses a stack frame — least of all
/// an embassy task frame, which would hold it for the task's whole life (#270). `N` is the caller's
/// own shard ceiling: a board with 16 FAT handles cannot mount a spec-legal 32-shard set, and
/// saying so in the type means [`MountedSet::mount`] refuses it with [`MountError::Handles`]
/// instead of failing an open mid-ride.
pub struct SetShards<'a, const N: usize = MAX_SHARDS> {
    inner: Vec<Mounted<'a>, N>,
}

/// A [`SetShards`] at the format's full §5.2 capacity — what a host, a simulator or a test uses,
/// where file handles are not the scarce thing. A device names its own smaller `N` instead, and
/// [`MountedSet::mount`] then refuses a larger set with [`MountError::Handles`].
///
/// (A named alias rather than the struct's default parameter: a defaulted const generic still has
/// to be inferred at a call site, so `SetShards::new()` alone does not compile.)
pub type FullSetShards<'a> = SetShards<'a, MAX_SHARDS>;

impl<'a, const N: usize> SetShards<'a, N> {
    /// An empty store. `const` so it can initialise a `static` directly, with no runtime
    /// constructor and no 14 KB temporary anywhere.
    pub const fn new() -> SetShards<'a, N> {
        SetShards { inner: Vec::new() }
    }

    /// How many shards this store can hold — the caller's ceiling, and the number
    /// [`MountError::Handles`] reports.
    #[inline]
    pub const fn capacity(&self) -> usize {
        N
    }
}

impl<const N: usize> Default for SetShards<'_, N> {
    fn default() -> Self {
        SetShards::new()
    }
}

/// A mounted volume set, ready to render — pointers into the core's tables, shared cache and the
/// caller's [`SetShards`] store, plus the validated core index. The manifest is deliberately not
/// retained: all dispatch metadata was copied into the shard records at mount, total bytes remain
/// available from the retained sources, and keeping an ~832 B manifest resident buys nothing.
pub struct MountedSet<'a> {
    core: &'a MapTables,
    cache: &'a MapCache,
    shards: &'a [Mounted<'a>],
    core_index: u8,
}

impl<'a> MountedSet<'a> {
    /// Mount a set **in place** into `store`: the §5.3 obligations a *reader* owns — every shard
    /// present, exactly the recorded size, opening as OBCM at the recorded version with the
    /// recorded header bbox — plus the ladder and cross-shard style checks §5.1/§4.7 imply.
    ///
    /// `sources` must be in manifest index order; `core` must be the [`MapTables`] parsed from
    /// `sources[manifest.core_shard()]`. Nothing here allocates; the largest stack temporary is one
    /// 64-byte style-compare window, and the per-shard records are written straight into `store`.
    ///
    /// There is deliberately no partial success. A mid-copy set — a missing shard, a shard
    /// still growing — fails, and the caller reports *map incomplete* rather than mounting a
    /// map with holes in it (§5.4). `store` is left cleared on every failure, so a refused mount
    /// never leaves half a set behind for the next attempt to trip over.
    ///
    /// The mount borrows `store` for `'s`, *shorter* than the `'a` of the bytes it points at. That
    /// split is deliberate: `&'a mut SetShards<'a, _>` would borrow the store for its own whole
    /// lifetime, which makes even dropping it a use-after-borrow, and the store is an ordinary local
    /// on a host. `'s` ends with the mount.
    pub fn mount<'s, const N: usize>(
        store: &'s mut SetShards<'a, N>,
        manifest: &SetManifest,
        sources: &[&'a dyn ByteSource],
        core: &'a MapTables,
        cache: &'a MapCache,
    ) -> Result<MountedSet<'s>, MountError>
    where
        'a: 's,
    {
        store.inner.clear();
        if let Err(error) = fill(&mut store.inner, manifest, sources, core) {
            store.inner.clear();
            return Err(error);
        }
        Ok(MountedSet { core, cache, shards: &store.inner, core_index: manifest.core_shard() as u8 })
    }

    /// The number of physical files held open for this mount.
    #[inline]
    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }

    /// A reader over the **core** shard. Nav, POI and hours queries go here and only here
    /// (§5.1) — the nav graph is whole, in one file, so A\* is untouched by sharding.
    #[inline]
    pub fn core_reader(&self) -> Reader<'_> {
        let index = self.core_index as usize;
        Reader::new_in_set(self.shards[index].src, self.core, self.cache, index as u8, None)
    }

    /// The set's assembly bbox (§4.2).
    #[inline]
    pub fn bbox(&self) -> BBox {
        self.core.bbox
    }

    /// Total bytes across every shard — the only size figure a UI may show (§5.4).
    #[inline]
    pub fn total_bytes(&self) -> u64 {
        self.shards.iter().map(|shard| shard.src.len() as u64).sum()
    }

    /// Roles, for diagnostics. Dispatch is deliberately role-blind (§5.1/§5.6).
    pub fn role_of(&self, shard: usize) -> Option<Role> {
        self.shards.get(shard).map(|mounted| mounted.role)
    }

    /// A reader over shard `index`, or `None` if there is no such shard.
    ///
    /// **Geometry only.** A non-core shard's reader borrows the core's [`MapTables`] (that is the
    /// whole RAM argument of a set), so its nav, POI and hours accessors would describe the core
    /// file's offsets against a shard's bytes. They therefore answer *empty* — see
    /// [`Reader::is_set_shard`] — and the real answers come from [`MountedSet::core_reader`] (§5.1).
    pub fn shard_reader(&self, index: usize) -> Option<Reader<'_>> {
        let mounted = self.shards.get(index)?;
        Some(self.reader_over(index, mounted))
    }

    /// A reader over `mounted`, which is shard `index` of this set.
    #[inline]
    fn reader_over<'b>(&'b self, index: usize, mounted: &'b Mounted<'a>) -> Reader<'b> {
        Reader::new_in_set(mounted.src, self.core, self.cache, index as u8, mounted.tables.as_ref())
    }

    /// Whether `mounted` can contribute to a `lod`/`view` query at all, decided from resident
    /// bytes only: a bbox test (nanoseconds, against milliseconds of SD I/O) plus the §5.6
    /// empty-LOD bit. This is why role-blind dispatch is free rather than merely correct — the core
    /// and the unsplit coarse shard intersect *every* viewport, and this skips them without a
    /// single read.
    fn dispatches(&self, mounted: &Mounted<'a>, lod: usize, view: &SetBBox) -> bool {
        if !mounted.bbox.intersects(view) {
            return false;
        }
        match &mounted.tables {
            Some(tables) => !tables.lod_is_empty(lod),
            None => !self.core.lod_is_empty(lod),
        }
    }
}

/// The per-shard half of [`MountedSet::mount`], writing straight into the caller's store.
fn fill<'a, const N: usize>(
    shards: &mut Vec<Mounted<'a>, N>,
    manifest: &SetManifest,
    sources: &[&'a dyn ByteSource],
    core: &'a MapTables,
) -> Result<(), MountError> {
    obc_formats::obcs::validate(manifest)?;
    if sources.len() != manifest.shard_count() {
        return Err(MountError::ShardCount);
    }
    if manifest.shard_count() > N {
        return Err(MountError::Handles(N.min(u8::MAX as usize) as u8));
    }
    let core_index = manifest.core_shard();
    // The core's style region, resolved once: it is the same file on every comparison, and
    // re-reading its header per shard would put 31 pointless reads in the mount of a full set.
    let core_styles = style_region(sources[core_index], 0)?;

    for (index, (record, &src)) in manifest.shards().iter().zip(sources).enumerate() {
        let at = index as u8;
        if src.len() != record.bytes {
            return Err(MountError::Size(at));
        }
        let bbox = BBox {
            min_lon: record.bbox.min_lon,
            min_lat: record.bbox.min_lat,
            max_lon: record.bbox.max_lon,
            max_lat: record.bbox.max_lat,
        };
        let tables = if index == core_index {
            if core.version != manifest.obcm_version {
                return Err(MountError::Header(at));
            }
            if core.bbox != bbox {
                return Err(MountError::Bbox(at));
            }
            None
        } else {
            let parsed = ShardTables::parse(src).map_err(|_| MountError::Header(at))?;
            // Symmetric with the core's check above, and with the board scan's: every shard's own
            // header must carry the version the manifest pins for the whole set (§5.3). Today the
            // reader parses exactly one OBCM version so this is also transitively true — which is
            // the reason to state it, not the reason to leave it out.
            if parsed.version() != manifest.obcm_version {
                return Err(MountError::Header(at));
            }
            if parsed.bbox != bbox {
                return Err(MountError::Bbox(at));
            }
            if !ladder_matches(core.lods(), parsed.lods()) {
                return Err(MountError::Ladder(at));
            }
            // §4.7: every shard is stamped from one skin, so the tables are byte-identical.
            // Validating beats re-loading — a 4 KiB style table per shard is exactly the
            // resident cost this design exists to avoid.
            if !style_tables_match(sources[core_index], core_styles, src, at)? {
                return Err(MountError::Styles(at));
            }
            Some(parsed)
        };
        // `shards` was length-checked against `N` above, so this cannot fail.
        let _ = shards.push(Mounted { src, bbox: record.bbox, role: record.role, tables });
    }
    Ok(())
}

/// Compare two shards' style regions byte for byte, streaming through a 64-byte stack window.
/// Runs once per shard at mount, never in the render loop. `core_region` is the core's
/// `(offset, length)`, resolved once by the caller.
fn style_tables_match(
    core: &dyn ByteSource,
    core_region: (usize, usize),
    other: &dyn ByteSource,
    at: u8,
) -> Result<bool, MountError> {
    let (core_offset, core_len) = core_region;
    let (other_offset, other_len) = style_region(other, at)?;
    if core_len != other_len {
        return Ok(false);
    }
    let mut a = [0u8; 64];
    let mut b = [0u8; 64];
    let mut done = 0usize;
    while done < core_len {
        let take = (core_len - done).min(64);
        core.read_at((core_offset + done) as u32, &mut a[..take]).map_err(|_| MountError::Styles(at))?;
        other.read_at((other_offset + done) as u32, &mut b[..take]).map_err(|_| MountError::Styles(at))?;
        if a[..take] != b[..take] {
            return Ok(false);
        }
        done += take;
    }
    Ok(true)
}

/// `(offset, length)` of a file's style region: the count byte plus its `count × 8` records.
fn style_region(src: &dyn ByteSource, at: u8) -> Result<(usize, usize), MountError> {
    let mut header = [0u8; obc_formats::obcm::HEADER_LEN];
    src.read_at(0, &mut header).map_err(|_| MountError::Styles(at))?;
    let offset = obc_formats::io::rd_u32(&header, 21) as usize;
    let mut count = [0u8; 1];
    src.read_at(offset as u32, &mut count).map_err(|_| MountError::Styles(at))?;
    Ok((offset, 1 + count[0] as usize * obc_formats::obcm::STYLE_RECORD_LEN))
}

/// Stamp a shard index into a per-shard token's chunk-id high word. The renderer treats the
/// three words as opaque, so the set is free to use five of them for the file the candidate
/// came from — which is what makes pass B able to find a pass-A candidate again across shards.
///
/// `None` when the chunk id is too large to carry a tag ([`MAX_TOKEN_CHUNK_ID`]) — impossible in a
/// legal file, so it means a corrupt one, and truncating the id would hand pass B a *different*
/// chunk's bytes.
#[inline]
fn tag_token(token: FeatureToken, shard: usize) -> Option<FeatureToken> {
    let [lo, hi, offset] = token.source_words();
    if shard > MAX_TOKEN_SHARD || hi >= (1 << 11) {
        return None;
    }
    Some(FeatureToken::from_source_words([lo, (hi & 0x07FF) | ((shard as u16) << 11), offset]))
}

/// Undo [`tag_token`]: `(shard, chunk id, offset within the chunk)`.
#[inline]
fn untag_token(token: FeatureToken) -> (usize, u32, usize) {
    let [lo, hi, offset] = token.source_words();
    ((hi >> 11) as usize, (((hi & 0x07FF) as u32) << 16) | lo as u32, offset as usize)
}

fn read_error(error: crate::MapReadError) -> SceneReadError {
    match error {
        crate::MapReadError::Source(_) => SceneReadError::Source,
        crate::MapReadError::Cache(crate::CacheError::Busy) => SceneReadError::CacheBusy,
        crate::MapReadError::Malformed => SceneReadError::Malformed,
    }
}

fn set_bbox(view: &BBox) -> SetBBox {
    SetBBox { min_lat: view.min_lat, min_lon: view.min_lon, max_lat: view.max_lat, max_lon: view.max_lon }
}

impl MapScene for MountedSet<'_> {
    #[inline]
    fn lod_count(&self) -> usize {
        // Every shard lists the **full ladder**, with the LODs it does not carry written empty
        // (§5.1) — and `mount` refuses a shard whose ladder is not the core's — so the core's
        // table is the set's.
        self.core.lods().len()
    }

    #[inline]
    fn select_lod_for_mpp(&self, mpp: f32) -> usize {
        self.core_reader().select_lod_for_mpp(mpp)
    }

    #[inline]
    fn style(&self, id: u8) -> Option<&Style> {
        self.core.styles()[id as usize].as_ref()
    }

    #[inline]
    fn marker_color(&self) -> u16 {
        self.core.marker_color
    }

    #[inline]
    fn backdrop_style(&self) -> Option<&Style> {
        self.core.backdrop_style()
    }

    fn diagnostics(&self) -> Result<Option<Diagnostics>, SceneReadError> {
        self.core_reader()
            .try_chunk_cache_stats()
            .map(|stats| {
                Some(Diagnostics {
                    chunk_hits: stats.chunk_hits,
                    chunk_misses: stats.chunk_misses,
                    source_reads: stats.sd_reads,
                    bytes_read: stats.bytes_read,
                })
            })
            .map_err(|crate::CacheError::Busy| SceneReadError::CacheBusy)
    }

    fn visit_candidates<const P: usize, const R: usize>(
        &self,
        lod: usize,
        view: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        should_decode: impl Fn(u8) -> bool,
        mut visit: impl FnMut(Candidate<'_>),
    ) -> CandidateReport {
        let mut report = CandidateReport::default();
        let box_of_view = set_bbox(view);
        for (index, mounted) in self.shards.iter().enumerate() {
            if !self.dispatches(mounted, lod, &box_of_view) {
                continue;
            }
            let reader = self.reader_over(index, mounted);
            let walk = reader.for_each_chunk(lod, view, |cid, node| {
                report.chunks_visited += 1;
                // A chunk id past the tag budget cannot be addressed again in pass B, so it is
                // dropped whole rather than served under a truncated identity.
                if cid > MAX_TOKEN_CHUNK_ID {
                    report.malformed_features = report.malformed_features.saturating_add(1);
                    return;
                }
                match reader.for_each_feature_filtered(lod, cid, &node, points, ring_lens, &should_decode, |feature| {
                    let words = [cid as u16, (cid >> 16) as u16, feature.offset() as u16];
                    let Some(token) = tag_token(FeatureToken::from_source_words(words), index) else {
                        return;
                    };
                    visit(Candidate { token, feature: crate::scene::scene_feature(&feature) });
                }) {
                    Ok(status) => {
                        report.capacity_dropped = report.capacity_dropped.saturating_add(status.capacity_dropped);
                        report.malformed_features = report.malformed_features.saturating_add(status.malformed);
                    }
                    Err(error) => report.read_failures.record(read_error(error)),
                }
            });
            if let Err(error) = walk {
                report.read_failures.record(read_error(error));
            }
        }
        report
    }

    fn decode_selected<const P: usize, const R: usize>(
        &self,
        lod: usize,
        view: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        selected: &mut impl SelectedFeatures,
    ) -> DecodeReport {
        let mut report = DecodeReport::default();
        if selected.is_empty() {
            return report;
        }
        let box_of_view = set_bbox(view);
        for (index, mounted) in self.shards.iter().enumerate() {
            if !self.dispatches(mounted, lod, &box_of_view) {
                continue;
            }
            let reader = self.reader_over(index, mounted);
            let walk = reader.for_each_chunk(lod, view, |cid, node| {
                let mut refetched = false;
                for slot in 0..selected.len() {
                    if !selected.is_pending(slot) {
                        continue;
                    }
                    let Some(token) = selected.token(slot) else {
                        continue;
                    };
                    let (shard, wanted, offset) = untag_token(token);
                    if shard != index || wanted != cid {
                        continue;
                    }
                    match reader.decode_feature_at(lod, cid, offset, &node, points, ring_lens) {
                        Ok(feature) => {
                            refetched |= selected.decoded(slot, crate::scene::scene_feature(&feature));
                        }
                        Err(error) => {
                            let _ = selected.failed(slot, feature_error(error));
                        }
                    }
                }
                if refetched {
                    report.chunks_refetched += 1;
                }
            });
            if let Err(error) = walk {
                report.read_failures.record(read_error(error));
            }
        }
        report
    }
}

fn feature_error(error: crate::FeatureReadError) -> obc_map_scene::FeatureError {
    use obc_map_scene::{CapacityError as SceneCapacityError, FeatureError as SceneFeatureError};
    match error {
        crate::FeatureReadError::Decode(crate::FeatureDecodeError::Capacity(crate::CapacityError::Points)) => {
            SceneFeatureError::Capacity(SceneCapacityError::Points)
        }
        crate::FeatureReadError::Decode(crate::FeatureDecodeError::Capacity(crate::CapacityError::Rings)) => {
            SceneFeatureError::Capacity(SceneCapacityError::Rings)
        }
        crate::FeatureReadError::Decode(crate::FeatureDecodeError::Malformed) => SceneFeatureError::Malformed,
        crate::FeatureReadError::Read(error) => SceneFeatureError::Read(read_error(error)),
    }
}

const _: () = assert!(MAX_TOKEN_SHARD == 31);
const _: () = assert!(MAX_TOKEN_CHUNK_ID == 0x07FF_FFFF);

// The RAM argument of the module docs, pinned so it cannot rot. Both targets are asserted: the
// device figures are what the epic's cost review is about, and the host figures keep a `cargo test`
// honest about the same shapes.
//
// `ShardTables` is **invariant** in the ladder's length — `heapless::Vec<Lod, 16>` reserves all 16
// rungs whatever the schema uses — so there is no "260 B at the v1 ladder, 500 B at the maximum".
// There is one number.
#[cfg(target_pointer_width = "32")]
mod device_sizes {
    use super::{Mounted, MountedSet, SetShards, ShardTables};
    use core::mem::size_of;

    const _: () = assert!(size_of::<ShardTables>() == 408);
    const _: () = assert!(size_of::<Mounted>() == 440);
    const _: () = assert!(size_of::<SetShards>() == 14_084);
    // Four machine words plus a compact core index. The point of the whole `SetShards` split: a
    // mount is cheap to move, and the 14 KB is somewhere the caller chose.
    const _: () = assert!(size_of::<MountedSet>() == 20);
}

#[cfg(target_pointer_width = "64")]
mod host_sizes {
    use super::{Mounted, MountedSet, SetShards, ShardTables};
    use core::mem::size_of;

    const _: () = assert!(size_of::<ShardTables>() == 800);
    const _: () = assert!(size_of::<Mounted>() == 848);
    const _: () = assert!(size_of::<SetShards>() == 27_144);
    const _: () = assert!(size_of::<MountedSet>() == 40);
}
