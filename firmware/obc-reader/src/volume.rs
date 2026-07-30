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
//! empty-LOD bitmask, a few hundred bytes. [`ShardTables::parse`] never touches the style
//! region, so mounting a shard also avoids `parse_styles`'s 2 KiB stack scratch; the whole
//! mount path stays shallow.
//!
//! The ≈277 KB [`MapCache`] is shared by the whole set. That is safe because
//! [`MapTables::parse_member`] gives every shard the same parse generation (so no shard clears
//! the cache the previous one filled) while [`Reader::new_in_set`] tags every cache key with
//! the shard index (so no shard is served another's chunks).

use heapless::Vec;
use obc_formats::io::ByteSource;
use obc_formats::obcs::{ManifestError, Role, SetBBox, SetManifest, MAX_SHARDS};
use obc_map_scene::{
    Candidate, CandidateReport, DecodeReport, Diagnostics, Feature, FeatureToken, MapScene, ReadError as SceneReadError,
    SelectedFeatures,
};

use crate::reader::{parse_header, parse_lod_table, Lod};
use crate::{BBox, Error, MapCache, MapTables, Reader, Style};

/// Highest shard index a [`FeatureToken`] can carry — the five bits stolen from the token's
/// chunk-id high word (see [`tag_token`]). Exactly the §5.2 shard cap.
const MAX_TOKEN_SHARD: usize = MAX_SHARDS - 1;
/// Chunk ids a tagged token can still express. Five of the token's 32 chunk-id bits go to the
/// shard index; 2^27 chunks at the packer's 4 KiB default is 512 GiB of one LOD, three orders
/// of magnitude past the `4 GiB − 1` per-file ceiling §5 exists to respect.
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
    /// Shard `index` is not the recorded `Bytes`. A shard still being written is the common
    /// case, and it reads as *map incomplete*, never as a smaller map.
    Size(u8),
    /// Shard `index` does not open as OBCM at the manifest's `OBCM Version`.
    Header(u8),
    /// Shard `index`'s OBCM header bbox is not the bbox the manifest records for it.
    Bbox(u8),
    /// Shard `index` carries a different style table than the core. Shards of one set are
    /// stamped from one skin (§4.7), so a mismatch means these files are not one map.
    Styles(u8),
    /// The core's [`MapTables`] were not parsed from the core shard's source.
    CoreMismatch,
}

impl From<ManifestError> for MountError {
    fn from(error: ManifestError) -> MountError {
        MountError::Manifest(error)
    }
}

/// Everything a non-core shard needs resident: its header bbox, its LOD table, and the §5.6
/// empty-LOD predicate. No style table, no POI directory, no nav directory — a geometry or
/// coarse shard has none of those (§5.1), and the skin is the core's.
pub struct ShardTables {
    bbox: BBox,
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
        Ok(ShardTables { bbox: parsed.bbox, empty: empty_mask(&lods), lods })
    }

    #[inline]
    pub fn bbox(&self) -> BBox {
        self.bbox
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

/// One mounted shard: where its bytes are, what the manifest says about it, and the tables the
/// dispatch loop needs.
struct Mounted<'a> {
    src: &'a dyn ByteSource,
    bbox: SetBBox,
    role: Role,
    /// `None` for the core shard, whose tables are the set's shared [`MapTables`].
    tables: Option<ShardTables>,
}

/// A mounted volume set, ready to render. See the module docs for the RAM argument.
pub struct MountedSet<'a> {
    manifest: SetManifest,
    core: &'a MapTables,
    cache: &'a MapCache,
    shards: Vec<Mounted<'a>, MAX_SHARDS>,
}

impl<'a> MountedSet<'a> {
    /// Mount a set: the §5.3 obligations a *reader* owns — every shard present, exactly the
    /// recorded size, opening as OBCM at the recorded version with the recorded header bbox —
    /// plus the guideline's cross-shard style check.
    ///
    /// `sources` must be in manifest index order; `core` must be the [`MapTables`] parsed from
    /// `sources[manifest.core_shard]`. Nothing here allocates, and the largest stack temporary
    /// is one 64-byte style-compare window.
    ///
    /// There is deliberately no partial success. A mid-copy set — a missing shard, a shard
    /// still growing — fails, and the caller reports *map incomplete* rather than mounting a
    /// map with holes in it (§5.4).
    pub fn mount(
        manifest: SetManifest,
        sources: &[&'a dyn ByteSource],
        core: &'a MapTables,
        cache: &'a MapCache,
    ) -> Result<MountedSet<'a>, MountError> {
        obc_formats::obcs::validate(&manifest)?;
        if sources.len() != manifest.shard_count() {
            return Err(MountError::ShardCount);
        }
        let core_index = manifest.core_shard as usize;
        let mut shards: Vec<Mounted<'a>, MAX_SHARDS> = Vec::new();

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
                if parsed.bbox != bbox {
                    return Err(MountError::Bbox(at));
                }
                // §4.7: every shard is stamped from one skin, so the tables are byte-identical.
                // Validating beats re-loading — a 4 KiB style table per shard is exactly the
                // resident cost this design exists to avoid.
                if !style_tables_match(sources[core_index], src)? {
                    return Err(MountError::Styles(at));
                }
                Some(parsed)
            };
            // `shards` has the manifest's own capacity, so this cannot fail.
            let _ = shards.push(Mounted { src, bbox: record.bbox, role: record.role, tables });
        }
        Ok(MountedSet { manifest, core, cache, shards })
    }

    /// The manifest this set mounted from.
    #[inline]
    pub fn manifest(&self) -> &SetManifest {
        &self.manifest
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
        let index = self.manifest.core_shard;
        Reader::new_in_set(self.shards[index as usize].src, self.core, self.cache, index, None)
    }

    /// The set's assembly bbox (§4.2).
    #[inline]
    pub fn bbox(&self) -> BBox {
        self.core.bbox
    }

    /// Total bytes across every shard — the only size figure a UI may show (§5.4).
    #[inline]
    pub fn total_bytes(&self) -> u64 {
        self.manifest.total_bytes()
    }

    /// Roles, for diagnostics. Dispatch is deliberately role-blind (§5.1/§5.6).
    pub fn role_of(&self, shard: usize) -> Option<Role> {
        self.shards.get(shard).map(|mounted| mounted.role)
    }

    /// A reader over shard `index`, or `None` if there is no such shard.
    fn shard_reader(&self, index: usize) -> Option<Reader<'_>> {
        let mounted = self.shards.get(index)?;
        Some(Reader::new_in_set(mounted.src, self.core, self.cache, index as u8, mounted.tables.as_ref()))
    }

    /// Whether shard `index` can contribute to a `lod`/`view` query at all, decided from
    /// resident bytes only: a bbox test (nanoseconds, against milliseconds of SD I/O) plus the
    /// §5.6 empty-LOD bit. This is why role-blind dispatch is free rather than merely correct —
    /// the core and the unsplit coarse shard intersect *every* viewport, and this skips them
    /// without a single read.
    fn dispatches(&self, index: usize, lod: usize, view: &SetBBox) -> bool {
        let Some(mounted) = self.shards.get(index) else {
            return false;
        };
        if !mounted.bbox.intersects(view) {
            return false;
        }
        match &mounted.tables {
            Some(tables) => !tables.lod_is_empty(lod),
            None => self.core.lod_is_empty(lod) == false,
        }
    }
}

/// Compare two shards' style regions byte for byte, streaming through a 64-byte stack window.
/// Runs once per shard at mount, never in the render loop.
fn style_tables_match(core: &dyn ByteSource, other: &dyn ByteSource) -> Result<bool, MountError> {
    let (core_offset, core_len) = style_region(core)?;
    let (other_offset, other_len) = style_region(other)?;
    if core_len != other_len {
        return Ok(false);
    }
    let mut a = [0u8; 64];
    let mut b = [0u8; 64];
    let mut done = 0usize;
    while done < core_len {
        let take = (core_len - done).min(64);
        core.read_at((core_offset + done) as u32, &mut a[..take]).map_err(|_| MountError::Styles(0))?;
        other.read_at((other_offset + done) as u32, &mut b[..take]).map_err(|_| MountError::Styles(0))?;
        if a[..take] != b[..take] {
            return Ok(false);
        }
        done += take;
    }
    Ok(true)
}

/// `(offset, length)` of a file's style region: the count byte plus its `count × 8` records.
fn style_region(src: &dyn ByteSource) -> Result<(usize, usize), MountError> {
    let mut header = [0u8; obc_formats::obcm::HEADER_LEN];
    src.read_at(0, &mut header).map_err(|_| MountError::Styles(0))?;
    let offset = obc_formats::io::rd_u32(&header, 21) as usize;
    let mut count = [0u8; 1];
    src.read_at(offset as u32, &mut count).map_err(|_| MountError::Styles(0))?;
    Ok((offset, 1 + count[0] as usize * obc_formats::obcm::STYLE_RECORD_LEN))
}

/// Stamp a shard index into a per-shard token's chunk-id high word. The renderer treats the
/// three words as opaque, so the set is free to use five of them for the file the candidate
/// came from — which is what makes pass B able to find a pass-A candidate again across shards.
#[inline]
fn tag_token(token: FeatureToken, shard: usize) -> FeatureToken {
    let [lo, hi, offset] = token.source_words();
    debug_assert!(shard <= MAX_TOKEN_SHARD && hi < (1 << 11), "chunk id too large to tag with a shard");
    FeatureToken::from_source_words([lo, (hi & 0x07FF) | ((shard as u16) << 11), offset])
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
        // (§5.1), so the core's table is the set's.
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
        for index in 0..self.shards.len() {
            if !self.dispatches(index, lod, &box_of_view) {
                continue;
            }
            let Some(reader) = self.shard_reader(index) else {
                continue;
            };
            let walk = reader.for_each_chunk(lod, view, |cid, node| {
                report.chunks_visited += 1;
                match reader.for_each_feature_filtered(lod, cid, &node, points, ring_lens, &should_decode, |feature| {
                    visit(Candidate {
                        token: tag_token(
                            FeatureToken::from_source_words([cid as u16, (cid >> 16) as u16, feature.offset() as u16]),
                            index,
                        ),
                        feature: Feature::new(
                            feature.style_id,
                            feature.kind,
                            feature.points(),
                            feature.ring_lens(),
                            feature.bbox(),
                        ),
                    });
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
        for index in 0..self.shards.len() {
            if !self.dispatches(index, lod, &box_of_view) {
                continue;
            }
            let Some(reader) = self.shard_reader(index) else {
                continue;
            };
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
                            refetched |= selected.decoded(
                                slot,
                                Feature::new(
                                    feature.style_id,
                                    feature.kind,
                                    feature.points(),
                                    feature.ring_lens(),
                                    feature.bbox(),
                                ),
                            );
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
