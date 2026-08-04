//! Volume sets: planning the shards, writing one shard's bytes, and the OBCS manifest
//! ([`OBCA_Spec.md`](../../../specs/OBCA_Spec.md) §5).
//!
//! One *logical* map is a small manifest plus 1..N physical OBCM files. Two independent 4 GiB
//! ceilings make that necessary — FAT32's per-file cap and OBCM's own `uint32` offsets — so sets are
//! the shape from day one and a small map is a **set of one** (§5.5).
//!
//! The split obeys one ordering principle and obeys it everywhere: **the core file holds only what
//! cannot be split by bbox, and everything that can be is moved out of it.** The core is the one
//! file that cannot scale horizontally (it holds the single unified nav graph), so its headroom is
//! the scarcest resource in the design and nothing else may spend it.

use obc_formats::obcm::{
    HEADER_LEN, LOD_ENTRY_LEN, MAGIC, STYLE_DASHED_BIT, STYLE_FIXED_WIDTH_BIT, STYLE_HAS_COLOR2_BIT,
    STYLE_PRIORITY_MASK, STYLE_RECORD_LEN, STYLE_TERRAIN_LAYER_BIT, VERSION,
};
use sha2::{Digest, Sha256};

use crate::graft::{self, LodPlan};
use crate::grid::AlignedBox;
use crate::input::Cell;
use crate::nav::MergedNav;
use crate::poi::PoiSection;
use crate::schema::{BandRole, StyleRecord};
use crate::scratch::ScratchStore;
use crate::{Error, Result};

/// The hard per-file ceiling: FAT32's `4 GiB − 1 B`, which is also `u32::MAX` and therefore also
/// OBCM's own offset ceiling (§5).
pub const FILE_CEILING: u64 = u32::MAX as u64;

/// Where a producer SHOULD warn about the core (§5.7): ≈ 3.5 GiB, naming the nav graph.
pub const CORE_WARN: u64 = 3_758_096_384;

/// Manifest sizes (§5.2).
pub const MANIFEST_HEADER_LEN: usize = 72;
pub const MANIFEST_SHARD_LEN: usize = 56;
/// A set holds `1..=32` shards; readers reject `0` or more (§5.2/§5.3).
pub const MAX_SHARDS: usize = 32;
/// Largest card id the §5.2 filenames hold: `MS<id>S<kk>.OBM` is 8.3-safe only while `<id>` is at
/// most three digits, because the firmware's FAT layer creates **short names only**. A four-digit id
/// produces a nine-character basename, which a device silently mangles into a name the derived-name
/// lookup no longer finds.
pub const MAX_CARD_ID: u16 = 999;

/// §5.2's card-id range, checked where the id enters rather than where the string is built.
pub fn check_card_id(card_id: u16) -> Result<()> {
    if card_id > MAX_CARD_ID {
        return Err(Error::Input(format!(
            "card id {card_id} needs four digits; `MS<id>S<kk>.OBM` is 8.3-safe only up to {MAX_CARD_ID} (OBCA §5.2)"
        )));
    }
    Ok(())
}

/// One physical file of the set, planned before a byte is written.
pub struct ShardPlan {
    pub index: usize,
    pub role: BandRole,
    pub box_: AlignedBox,
    /// One entry per ladder level; a level this shard does not carry is [`LodPlan::empty`].
    pub lods: Vec<LodPlan>,
    /// Whether this shard carries the nav graph and the POIs — true for exactly one shard, the core.
    pub core: bool,
    /// Total bytes, computable before the write and re-checked after it (§5.7).
    pub bytes: u64,
    /// Filled by [`write`].
    pub sha256: [u8; 32],
}

impl ShardPlan {
    /// Layout cursor: where each region starts, given the fixed prefix.
    fn layout(&self, style_len: usize, poi_len: usize, nav_len: usize) -> Layout {
        let lod_table_offset = HEADER_LEN + style_len;
        let mut cursor = lod_table_offset + self.lods.len() * LOD_ENTRY_LEN;
        let mut lod_offsets = Vec::with_capacity(self.lods.len());
        for l in &self.lods {
            lod_offsets.push(cursor);
            cursor += l.region_bytes() as usize;
        }
        let poi_offset = cursor;
        let nav_offset = poi_offset + poi_len;
        Layout { lod_table_offset, lod_offsets, poi_offset, nav_offset, total: (nav_offset + nav_len) as u64 }
    }
}

struct Layout {
    lod_table_offset: usize,
    lod_offsets: Vec<usize>,
    poi_offset: usize,
    nav_offset: usize,
    total: u64,
}

/// Compute a shard's total size without writing it — §5.7's projection, applied to the assembler's
/// own output so an over-size file is refused rather than emitted.
pub fn projected_bytes(plan: &ShardPlan, style_len: usize, poi_len: usize, nav_len: usize) -> u64 {
    plan.layout(style_len, poi_len, nav_len).total
}

/// Write one shard: header, style table, LOD table, every LOD region, the POI section, the nav
/// section. Returns `(bytes, sha256)`.
///
/// Nothing is back-patched. Every offset in the header and the LOD table is known before the first
/// byte goes out, because the graft plan and both rebuilt sections were sized first — which is what
/// lets the output stream straight into a file (or a browser's download stream) rather than a buffer.
///
/// `cells` are the shard's own grafted cells; `nav_cells` are the `network` cells the §4.6 merge
/// read, in the order it read them. They are a second list because the merged graph holds its edge
/// records as *addresses* into those cells (§4.6.6) — the core's nav section is streamed out of them
/// here rather than out of a pool the merge would otherwise have had to keep.
///
/// `scratch` must be the store the §4.6 merge spilled into: since #1116 D4 the nav section's index,
/// chunks and pool plan live there too, and they stay valid until `MergedNav::release`.
// The argument list is one shard's whole input: the plan, the cells it grafts, the three rebuilt
// pieces every shard shares, and the sink. Bundling them into a struct would move the noise rather
// than remove it — the same call the packer's `serialize_lods_streaming` makes, for the same reason.
#[allow(clippy::too_many_arguments)]
pub fn write(
    plan: &ShardPlan,
    cells: &[Cell<'_>],
    nav_cells: &[&Cell<'_>],
    styles: &[StyleRecord],
    marker_color: u16,
    poi: &PoiSection,
    nav: &MergedNav,
    profile_table: &[u8],
    scratch: &dyn ScratchStore,
    sink: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<(u64, [u8; 32])> {
    let style_bytes = pack_style_table(styles);
    // The empty pair a non-core shard carries (§5.1), built **once**: it is written at the end and
    // its length is needed at the start, and building it twice was one section's worth of allocation
    // per shard for no reason.
    let empty_poi = if plan.core { None } else { Some(crate::poi::empty_layout(plan.box_.ubox())) };
    let empty_nav = if plan.core { None } else { Some(MergedNav::empty(Default::default())) };
    let poi_bytes_len = empty_poi.as_ref().map_or_else(|| poi.section_len(), |p| p.section_len());
    let nav_len = empty_nav.as_ref().map_or(nav, |n| n).section_len(profile_table);
    let l = plan.layout(style_bytes.len(), poi_bytes_len, nav_len);
    if l.total > FILE_CEILING {
        return Err(Error::Capacity(format!(
            "shard {} would be {} bytes, past the {FILE_CEILING}-byte FAT32/uint32 ceiling — reduce the coverage \
             (OBCA §5.7)",
            plan.index, l.total
        )));
    }

    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    let mut out = |buf: &[u8]| -> Result<()> {
        hasher.update(buf);
        written += buf.len() as u64;
        sink(buf)
    };

    // 1. Header (bbox stored lat, lon, lat, lon — `OBCM_Spec.md` §1).
    out(&header_bytes(
        plan.box_,
        plan.lods.len(),
        marker_color,
        l.lod_table_offset as u32,
        l.poi_offset as u32,
        l.nav_offset as u32,
    ))?;

    // 2. Style table (the skin, §4.7) and 3. the LOD table.
    out(&style_bytes)?;
    let mut table = Vec::with_capacity(plan.lods.len() * LOD_ENTRY_LEN);
    for (p, &offset) in plan.lods.iter().zip(&l.lod_offsets) {
        push_lod_entry(&mut table, p.max_mpp, offset as u32, p.node_count, p.chunk_size, p.chunk_count);
    }
    out(&table)?;

    // 4. Each LOD region: fresh upper tree, relocated cell blocks, offset table, chunk bytes.
    for p in &plan.lods {
        graft::emit_lod(p, cells, &mut out)?;
    }

    // 5/6. The POI and nav sections — the core's rebuilt ones, or a legal empty pair (§5.1).
    out(&crate::poi::serialize(empty_poi.as_ref().unwrap_or(poi), l.poi_offset))?;
    crate::nav::serialize(
        empty_nav.as_ref().unwrap_or(nav),
        profile_table,
        l.nav_offset,
        nav_cells,
        scratch,
        &mut out,
    )?;

    // §4.8.6: the write must land exactly where §5.7's projection said it would. A `debug_assert`
    // would leave a release build emitting a file whose recorded `Bytes` and header offsets are a
    // sentence about a different layout — and the manifest would be written over it.
    if written != l.total {
        return Err(Error::Verify(format!(
            "shard {} projected to {} bytes but wrote {written} — the §5.7 projection and the write disagree",
            plan.index, l.total
        )));
    }
    Ok((written, hasher.finalize().into()))
}

/// The 40-byte OBCM header (`OBCM_Spec.md` §1), byte-for-byte the packer's `header_bytes`. Split out
/// because it is a **restatement** of `obc-pack`'s serializer, and `tests/pinning.rs` compares the
/// two outputs directly rather than trusting that two copies of a table stay in step.
pub fn header_bytes(
    box_: AlignedBox,
    lod_count: usize,
    marker_color: u16,
    lod_table_offset: u32,
    poi_offset: u32,
    nav_offset: u32,
) -> Vec<u8> {
    let (min_lon, min_lat, max_lon, max_lat) = box_.ubox();
    let mut head = Vec::with_capacity(HEADER_LEN);
    head.extend_from_slice(&MAGIC);
    head.push(VERSION);
    head.extend_from_slice(&(min_lat as i32).to_le_bytes());
    head.extend_from_slice(&(min_lon as i32).to_le_bytes());
    head.extend_from_slice(&(max_lat as i32).to_le_bytes());
    head.extend_from_slice(&(max_lon as i32).to_le_bytes());
    head.extend_from_slice(&(HEADER_LEN as u32).to_le_bytes());
    head.push(lod_count as u8);
    head.extend_from_slice(&lod_table_offset.to_le_bytes());
    head.extend_from_slice(&marker_color.to_le_bytes());
    head.extend_from_slice(&poi_offset.to_le_bytes());
    head.extend_from_slice(&nav_offset.to_le_bytes());
    debug_assert_eq!(head.len(), HEADER_LEN);
    head
}

/// Append one 18-byte LOD-table entry (`OBCM_Spec.md` §3), byte-for-byte the packer's
/// `push_lod_entry`: `Max Meters/Pixel` (`None` ⇒ `+inf`), index offset, node count, chunk capacity,
/// chunk count. Pinned against the packer alongside the header.
pub fn push_lod_entry(
    table: &mut Vec<u8>,
    max_mpp: Option<f64>,
    index_offset: u32,
    node_count: u32,
    chunk_size: usize,
    chunk_count: u32,
) {
    table.extend_from_slice(&max_mpp.map_or(f32::INFINITY, |v| v as f32).to_le_bytes());
    table.extend_from_slice(&index_offset.to_le_bytes());
    table.extend_from_slice(&node_count.to_le_bytes());
    table.extend_from_slice(&(chunk_size as u16).to_le_bytes());
    table.extend_from_slice(&chunk_count.to_le_bytes());
}

/// The style table (`OBCM_Spec.md` §2): `Count` then one 8-byte record per style, id ascending.
pub fn pack_style_table(styles: &[StyleRecord]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + styles.len() * STYLE_RECORD_LEN);
    out.push(styles.len() as u8);
    for s in styles {
        let mut flags = (s.priority.clamp(1, 4) - 1) & STYLE_PRIORITY_MASK;
        if s.dashed {
            flags |= STYLE_DASHED_BIT;
        }
        if s.color2.is_some() {
            flags |= STYLE_HAS_COLOR2_BIT;
        }
        if s.fixed_width {
            flags |= STYLE_FIXED_WIDTH_BIT;
        }
        if s.terrain_layer {
            flags |= STYLE_TERRAIN_LAYER_BIT;
        }
        out.push(s.id);
        out.push(s.z_index as u8);
        out.extend_from_slice(&s.color.to_le_bytes());
        out.push(s.weight);
        out.push(flags);
        out.extend_from_slice(&s.color2.unwrap_or(0).to_le_bytes());
    }
    out
}

/// The set's terrain shard as the manifest records it (§5.2's `terrain` role) — everything the
/// record needs and nothing about how it was built.
#[derive(Clone, Copy, Debug)]
pub struct TerrainRecord {
    pub bytes: u64,
    pub sha256: [u8; 32],
}

/// The OBCS set manifest (§5.2): `72 + 56 × Shard Count` bytes, fixed-layout and little-endian, so a
/// device parses it with no allocation.
///
/// `terrain` is the optional `Role == 3` record, and it is written **last** — the invariant that
/// lets every reader treat the leading records as the OBCM shards without a role filter.
pub fn manifest(
    shards: &[ShardPlan],
    terrain: Option<TerrainRecord>,
    assembly: AlignedBox,
    schema_revision: u32,
    name: &str,
) -> Result<Vec<u8>> {
    let records = shards.len() + terrain.is_some() as usize;
    if shards.is_empty() || records > MAX_SHARDS {
        return Err(Error::Capacity(format!(
            "a set holds 1..={MAX_SHARDS} records; this one needs {records} ({} shard(s){})",
            shards.len(),
            if terrain.is_some() { " plus terrain" } else { "" }
        )));
    }
    let core = shards
        .iter()
        .position(|s| s.core)
        .ok_or_else(|| Error::Verify("the set has no core shard (OBCA §5.3)".into()))?;
    // §5.3: past the single-file fast path a set MUST carry at least one shard of each non-core
    // role. A reader has no schema to consult, so "the schema names no coarse band" is not an
    // excuse it can hear — it sees a manifest missing `Role == 2` and refuses to mount, and every
    // shard the host wrote is dead weight on the card. Catch it here, where the set is still ours.
    if shards.len() > 1 {
        for (role, what) in [(BandRole::Geometry, "geometry"), (BandRole::Coarse, "coarse")] {
            if !shards.iter().any(|s| !s.core && s.role == role) {
                return Err(Error::Verify(format!(
                    "a multi-shard set has no {what} shard, so no reader will mount it (OBCA §5.3). The schema's band \
                     table must name a band with role {what:?} for selections past the single-file threshold."
                )));
            }
        }
    }

    // `Set Id` is a content identity: two assemblies of the same cells with the same skin produce
    // the same id, which is what lets an upload notice the set is already present. Terrain is in the
    // chain — a set that gained or re-baked its raster is a different set on the card.
    let mut id_hash = Sha256::new();
    for s in shards {
        id_hash.update(s.sha256);
    }
    if let Some(t) = &terrain {
        id_hash.update(t.sha256);
    }
    let set_id: [u8; 32] = id_hash.finalize().into();

    let (min_lon, min_lat, max_lon, max_lat) = assembly.ubox();
    let mut out = Vec::with_capacity(MANIFEST_HEADER_LEN + records * MANIFEST_SHARD_LEN);
    out.extend_from_slice(b"OBCS");
    out.push(obc_formats::obcs::VERSION); // manifest version
    out.push(VERSION); // the OBCM version of every OBCM shard
    out.push(records as u8);
    out.push(core as u8);
    out.extend_from_slice(&schema_revision.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // flags, reserved
    out.extend_from_slice(&(min_lat as i32).to_le_bytes());
    out.extend_from_slice(&(min_lon as i32).to_le_bytes());
    out.extend_from_slice(&(max_lat as i32).to_le_bytes());
    out.extend_from_slice(&(max_lon as i32).to_le_bytes());
    out.extend_from_slice(&set_id[..16]);
    out.extend_from_slice(&fold_name(name));
    debug_assert_eq!(out.len(), MANIFEST_HEADER_LEN);

    let push_record = |out: &mut Vec<u8>, role: u8, box_: AlignedBox, bytes: u64, sha256: &[u8; 32]| {
        let (min_lon, min_lat, max_lon, max_lat) = box_.ubox();
        out.push(role);
        out.push(0); // flags
        out.extend_from_slice(&[0, 0]); // reserved
        out.extend_from_slice(&(min_lat as i32).to_le_bytes());
        out.extend_from_slice(&(min_lon as i32).to_le_bytes());
        out.extend_from_slice(&(max_lat as i32).to_le_bytes());
        out.extend_from_slice(&(max_lon as i32).to_le_bytes());
        out.extend_from_slice(&(bytes as u32).to_le_bytes());
        out.extend_from_slice(sha256);
    };
    for s in shards {
        push_record(&mut out, s.role.wire(), s.box_, s.bytes, &s.sha256);
    }
    // §5.2: the terrain record last, spanning the assembly bbox. Last is not a convention — a
    // reader takes the leading records as the OBCM shards and their index as the `S<kk>` in a
    // derived filename, so a raster anywhere else would rename every shard after it.
    if let Some(t) = &terrain {
        push_record(&mut out, obc_formats::obcs::Role::Terrain.id(), assembly, t.bytes, &t.sha256);
    }
    debug_assert_eq!(out.len(), MANIFEST_HEADER_LEN + records * MANIFEST_SHARD_LEN);
    // The set is about to be written; the format authority is what decides whether it is one
    // (§5.3). Parsing our own bytes back is cheap and catches a role, an ordering or a bbox the
    // planner got wrong here rather than on a card.
    obc_formats::obcs::parse(&out)
        .map_err(|e| Error::Verify(format!("the manifest this assembly wrote does not validate: {e:?} (OBCA §5.3)")))?;
    Ok(out)
}

/// A 24-byte display name: printable ASCII, `0xFF`-padded — the `OBCM_Spec.md` §7.3 name convention.
fn fold_name(name: &str) -> [u8; 24] {
    let mut out = [0xFFu8; 24];
    for (slot, b) in out.iter_mut().zip(name.bytes()) {
        *slot = if (0x20..=0x7E).contains(&b) { b } else { b'?' };
    }
    out
}

/// The filename a shard lives under at the card root (§5.2): `MS<id>S<kk>.OBM`. Derived, never
/// stored — a stored name is a second source of truth that can disagree with the directory.
pub fn shard_filename(card_id: u16, index: usize) -> String {
    format!("MS{card_id}S{index:02}.OBM")
}

/// The manifest's own filename: `MS<id>.OBS` (§5.2).
pub fn manifest_filename(card_id: u16) -> String {
    format!("MS{card_id}.OBS")
}

/// The terrain shard's filename: `MS<id>.OBD` (§5.2) — no `S<kk>`, because there is at most one.
/// It is also the `OBCT_Spec.md` §4.6 sidecar of `MS<id>.OBS`, so a host that resolves terrain by
/// sidecar and one that reads the manifest role open the same file.
pub fn terrain_filename(card_id: u16) -> String {
    format!("MS{card_id}.OBD")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(index: usize, role: BandRole, core: bool) -> ShardPlan {
        ShardPlan {
            index,
            role,
            box_: AlignedBox { min_lat: 47_185_920, min_lon: 7_340_032, span_log2: 20 },
            lods: Vec::new(),
            core,
            bytes: 1234,
            sha256: [index as u8; 32],
        }
    }

    #[test]
    fn manifest_layout_is_the_spec_table() {
        let shards =
            vec![plan(0, BandRole::Core, true), plan(1, BandRole::Coarse, false), plan(2, BandRole::Geometry, false)];
        let bx = AlignedBox { min_lat: 47_185_920, min_lon: 7_340_032, span_log2: 20 };
        let m = manifest(&shards, None, bx, 7, "Switzerland").expect("manifest");
        assert_eq!(m.len(), MANIFEST_HEADER_LEN + 3 * MANIFEST_SHARD_LEN);
        assert_eq!(&m[0..4], b"OBCS");
        assert_eq!(m[4], 2, "manifest v2 — the terrain role is a hard cut");
        assert_eq!(m[5], VERSION);
        assert_eq!(m[6], 3, "shard count");
        assert_eq!(m[7], 0, "core shard index");
        assert_eq!(u32::from_le_bytes(m[8..12].try_into().unwrap()), 7, "schema revision");
        assert_eq!(u32::from_le_bytes(m[12..16].try_into().unwrap()), 0, "flags reserved");
        assert_eq!(i32::from_le_bytes(m[16..20].try_into().unwrap()), 47_185_920, "min lat first");
        assert_eq!(&m[48..59], b"Switzerland");
        assert_eq!(m[59], 0xFF, "the name pads with 0xFF");
        // Shard records: role byte, then the bbox in the header's own order, size, digest.
        assert_eq!(m[72], 0, "core role");
        assert_eq!(m[72 + MANIFEST_SHARD_LEN], 2, "coarse role");
        assert_eq!(u32::from_le_bytes(m[92..96].try_into().unwrap()), 1234);
        assert_eq!(m[96], 0, "the first shard's digest");
    }

    #[test]
    fn a_set_with_no_core_is_refused() {
        let shards = vec![plan(0, BandRole::Geometry, false)];
        let bx = AlignedBox { min_lat: 0, min_lon: 0, span_log2: 20 };
        assert!(manifest(&shards, None, bx, 1, "x").is_err());
    }

    /// §5.3: a reader has no schema, so a multi-shard set missing a whole role does not mount. The
    /// host must not write one — a schema whose band table names no `coarse` band would otherwise
    /// produce a manifest that looks valid and is not.
    #[test]
    fn a_multi_shard_set_missing_a_role_is_refused() {
        let bx = AlignedBox { min_lat: 47_185_920, min_lon: 7_340_032, span_log2: 20 };
        let no_coarse = vec![plan(0, BandRole::Core, true), plan(1, BandRole::Geometry, false)];
        let err = format!("{}", manifest(&no_coarse, None, bx, 1, "x").expect_err("no coarse shard"));
        assert!(err.contains("no coarse shard"), "got: {err}");
        let no_geometry = vec![plan(0, BandRole::Core, true), plan(1, BandRole::Coarse, false)];
        let err = format!("{}", manifest(&no_geometry, None, bx, 1, "x").expect_err("no geometry shard"));
        assert!(err.contains("no geometry shard"), "got: {err}");
        // …and the single-file fast path is exempt, because §5.5 says the one shard is the core.
        manifest(&[plan(0, BandRole::Core, true)], None, bx, 1, "x").expect("a set of one is legal");
    }

    /// §5.2's `terrain` role: the last record, spanning the assembly, counted by `Shard Count`,
    /// and in the `Set Id` chain — so a set that gains a raster is a different set on the card.
    #[test]
    fn the_terrain_record_is_last_and_changes_the_set_id() {
        let bx = AlignedBox { min_lat: 47_185_920, min_lon: 7_340_032, span_log2: 20 };
        let shards = vec![plan(0, BandRole::Core, true)];
        let terrain = TerrainRecord { bytes: 6_192, sha256: [0x77; 32] };
        let plain = manifest(&shards, None, bx, 7, "Grimsel").expect("no raster is a complete map");
        let with = manifest(&shards, Some(terrain), bx, 7, "Grimsel").expect("core + terrain");

        assert_eq!(plain.len(), MANIFEST_HEADER_LEN + MANIFEST_SHARD_LEN);
        assert_eq!(with.len(), MANIFEST_HEADER_LEN + 2 * MANIFEST_SHARD_LEN);
        assert_eq!(with[6], 2, "Shard Count counts every record");
        assert_eq!(with[7], 0, "…and the core is still record 0");
        let base = MANIFEST_HEADER_LEN + MANIFEST_SHARD_LEN;
        assert_eq!(with[base], obc_formats::obcs::Role::Terrain.id());
        assert_eq!(i32::from_le_bytes(with[base + 4..base + 8].try_into().unwrap()) as i64, bx.min_lat);
        assert_eq!(u32::from_le_bytes(with[base + 20..base + 24].try_into().unwrap()), 6_192);
        assert_eq!(&with[base + 24..base + 56], &[0x77; 32]);
        assert_ne!(&with[32..48], &plain[32..48], "the Set Id covers the raster");

        // The parsed manifest keeps the raster out of the OBCM shard list, which is what every
        // mount path iterates.
        let parsed = obc_formats::obcs::parse(&with).expect("valid");
        assert_eq!(parsed.shard_count(), 1);
        assert_eq!(parsed.record_count(), 2);
        assert_eq!(parsed.terrain().map(|t| t.bytes), Some(6_192));
        assert!(parsed.is_single_file(), "terrain is its own file, so this is still §5.5's fast path");
    }

    #[test]
    fn filenames_are_eight_three_safe() {
        assert_eq!(terrain_filename(7), "MS7.OBD");
        assert_eq!(terrain_filename(999), "MS999.OBD");
        assert_eq!(shard_filename(7, 0), "MS7S00.OBM");
        assert_eq!(shard_filename(MAX_CARD_ID, 31), "MS999S31.OBM");
        assert_eq!(manifest_filename(7), "MS7.OBS");
        assert!(shard_filename(MAX_CARD_ID, 31).split('.').next().unwrap().len() <= 8, "8.3 short name");
        // The bound itself, checked where the id enters: one more digit breaks the short name.
        check_card_id(MAX_CARD_ID).expect("three digits fit");
        let err = format!("{}", check_card_id(MAX_CARD_ID + 1).expect_err("four digits do not"));
        assert!(err.contains("8.3-safe"), "got: {err}");
        assert_eq!(shard_filename(1000, 0).split('.').next().unwrap().len(), 9, "…which is what it would look like");
    }

    #[test]
    fn style_records_are_eight_bytes_with_the_flag_bits() {
        let s = StyleRecord {
            id: 3,
            z_index: -2,
            color: 0x1234,
            weight: 5,
            priority: 2,
            dashed: true,
            color2: Some(0xBEEF),
            fixed_width: false,
            terrain_layer: false,
        };
        let bytes = pack_style_table(&[s]);
        assert_eq!(bytes.len(), 1 + STYLE_RECORD_LEN);
        assert_eq!(bytes[0], 1);
        assert_eq!(bytes[1], 3);
        assert_eq!(bytes[2] as i8, -2);
        assert_eq!(u16::from_le_bytes([bytes[3], bytes[4]]), 0x1234);
        assert_eq!(bytes[5], 5);
        assert_eq!(bytes[6], 1 | STYLE_DASHED_BIT | STYLE_HAS_COLOR2_BIT, "priority 2 ⇒ bits 0-1 = 1");
        assert_eq!(u16::from_le_bytes([bytes[7], bytes[8]]), 0xBEEF);

        // #1095: a stamped skin carries the two new bits through to the record it writes, so a
        // restyled cell tree keeps a contour hairline and terrain-layer-tagged instead of quietly
        // clearing them back to a ramped road (bits 6-7 stay reserved and written 0).
        let terrain = StyleRecord { fixed_width: true, terrain_layer: true, ..s };
        let bytes = pack_style_table(&[terrain]);
        assert_eq!(
            bytes[6],
            1 | STYLE_DASHED_BIT | STYLE_HAS_COLOR2_BIT | STYLE_FIXED_WIDTH_BIT | STYLE_TERRAIN_LAYER_BIT
        );
        assert_eq!(bytes[6] & obc_formats::obcm::STYLE_RESERVED_MASK, 0, "bits 6-7 stay 0");
    }
}
