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
    HEADER_LEN, LOD_ENTRY_LEN, MAGIC, STYLE_DASHED_BIT, STYLE_HAS_COLOR2_BIT, STYLE_PRIORITY_MASK, STYLE_RECORD_LEN,
    VERSION,
};
use sha2::{Digest, Sha256};

use crate::graft::{self, LodPlan};
use crate::grid::AlignedBox;
use crate::input::Cell;
use crate::nav::MergedNav;
use crate::poi::PoiSection;
use crate::schema::{BandRole, StyleRecord};
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
// The argument list is one shard's whole input: the plan, the cells it grafts, the three rebuilt
// pieces every shard shares, and the sink. Bundling them into a struct would move the noise rather
// than remove it — the same call the packer's `serialize_lods_streaming` makes, for the same reason.
#[allow(clippy::too_many_arguments)]
pub fn write(
    plan: &ShardPlan,
    cells: &[Cell<'_>],
    styles: &[StyleRecord],
    marker_color: u16,
    poi: &PoiSection,
    nav: &MergedNav,
    profile_table: &[u8],
    sink: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<(u64, [u8; 32])> {
    let style_bytes = pack_style_table(styles);
    let poi_bytes_len =
        if plan.core { poi.section_len() } else { crate::poi::empty_layout(plan.box_.ubox()).section_len() };
    let nav_len = if plan.core {
        nav.section_len(profile_table)
    } else {
        MergedNav::empty(Default::default()).section_len(profile_table)
    };
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
    let (min_lon, min_lat, max_lon, max_lat) = plan.box_.ubox();
    let mut head = Vec::with_capacity(HEADER_LEN);
    head.extend_from_slice(&MAGIC);
    head.push(VERSION);
    head.extend_from_slice(&(min_lat as i32).to_le_bytes());
    head.extend_from_slice(&(min_lon as i32).to_le_bytes());
    head.extend_from_slice(&(max_lat as i32).to_le_bytes());
    head.extend_from_slice(&(max_lon as i32).to_le_bytes());
    head.extend_from_slice(&(HEADER_LEN as u32).to_le_bytes());
    head.push(plan.lods.len() as u8);
    head.extend_from_slice(&(l.lod_table_offset as u32).to_le_bytes());
    head.extend_from_slice(&marker_color.to_le_bytes());
    head.extend_from_slice(&(l.poi_offset as u32).to_le_bytes());
    head.extend_from_slice(&(l.nav_offset as u32).to_le_bytes());
    debug_assert_eq!(head.len(), HEADER_LEN);
    out(&head)?;

    // 2. Style table (the skin, §4.7) and 3. the LOD table.
    out(&style_bytes)?;
    let mut table = Vec::with_capacity(plan.lods.len() * LOD_ENTRY_LEN);
    for (p, &offset) in plan.lods.iter().zip(&l.lod_offsets) {
        table.extend_from_slice(&p.max_mpp.map_or(f32::INFINITY, |v| v as f32).to_le_bytes());
        table.extend_from_slice(&(offset as u32).to_le_bytes());
        table.extend_from_slice(&p.node_count.to_le_bytes());
        table.extend_from_slice(&(p.chunk_size as u16).to_le_bytes());
        table.extend_from_slice(&p.chunk_count.to_le_bytes());
    }
    out(&table)?;

    // 4. Each LOD region: fresh upper tree, relocated cell blocks, offset table, chunk bytes.
    for p in &plan.lods {
        graft::emit_lod(p, cells, &mut out)?;
    }

    // 5/6. The POI and nav sections — the core's rebuilt ones, or a legal empty pair (§5.1).
    if plan.core {
        out(&crate::poi::serialize(poi, l.poi_offset))?;
        out(&crate::nav::serialize(nav, profile_table, l.nav_offset))?;
    } else {
        out(&crate::poi::serialize(&crate::poi::empty_layout(plan.box_.ubox()), l.poi_offset))?;
        out(&crate::nav::serialize(&MergedNav::empty(Default::default()), profile_table, l.nav_offset))?;
    }

    debug_assert_eq!(written, l.total, "the projection and the write must agree byte for byte");
    Ok((written, hasher.finalize().into()))
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
        out.push(s.id);
        out.push(s.z_index as u8);
        out.extend_from_slice(&s.color.to_le_bytes());
        out.push(s.weight);
        out.push(flags);
        out.extend_from_slice(&s.color2.unwrap_or(0).to_le_bytes());
    }
    out
}

/// The OBCS set manifest (§5.2): `72 + 56 × Shard Count` bytes, fixed-layout and little-endian, so a
/// device parses it with no allocation.
pub fn manifest(shards: &[ShardPlan], assembly: AlignedBox, schema_revision: u32, name: &str) -> Result<Vec<u8>> {
    if shards.is_empty() || shards.len() > MAX_SHARDS {
        return Err(Error::Capacity(format!("a set holds 1..={MAX_SHARDS} shards, not {}", shards.len())));
    }
    let core = shards
        .iter()
        .position(|s| s.core)
        .ok_or_else(|| Error::Verify("the set has no core shard (OBCA §5.3)".into()))?;

    // `Set Id` is a content identity: two assemblies of the same cells with the same skin produce
    // the same id, which is what lets an upload notice the set is already present.
    let mut id_hash = Sha256::new();
    for s in shards {
        id_hash.update(s.sha256);
    }
    let set_id: [u8; 32] = id_hash.finalize().into();

    let (min_lon, min_lat, max_lon, max_lat) = assembly.ubox();
    let mut out = Vec::with_capacity(MANIFEST_HEADER_LEN + shards.len() * MANIFEST_SHARD_LEN);
    out.extend_from_slice(b"OBCS");
    out.push(1); // manifest version
    out.push(VERSION); // the OBCM version of every shard
    out.push(shards.len() as u8);
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

    for s in shards {
        let (min_lon, min_lat, max_lon, max_lat) = s.box_.ubox();
        out.push(s.role.wire());
        out.push(0); // flags
        out.extend_from_slice(&[0, 0]); // reserved
        out.extend_from_slice(&(min_lat as i32).to_le_bytes());
        out.extend_from_slice(&(min_lon as i32).to_le_bytes());
        out.extend_from_slice(&(max_lat as i32).to_le_bytes());
        out.extend_from_slice(&(max_lon as i32).to_le_bytes());
        out.extend_from_slice(&(s.bytes as u32).to_le_bytes());
        out.extend_from_slice(&s.sha256);
    }
    debug_assert_eq!(out.len(), MANIFEST_HEADER_LEN + shards.len() * MANIFEST_SHARD_LEN);
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
        let shards = vec![plan(0, BandRole::Core, true), plan(1, BandRole::Coarse, false)];
        let bx = AlignedBox { min_lat: 47_185_920, min_lon: 7_340_032, span_log2: 21 };
        let m = manifest(&shards, bx, 7, "Switzerland").expect("manifest");
        assert_eq!(m.len(), MANIFEST_HEADER_LEN + 2 * MANIFEST_SHARD_LEN);
        assert_eq!(&m[0..4], b"OBCS");
        assert_eq!(m[4], 1);
        assert_eq!(m[5], VERSION);
        assert_eq!(m[6], 2, "shard count");
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
        assert!(manifest(&shards, bx, 1, "x").is_err());
    }

    #[test]
    fn filenames_are_eight_three_safe() {
        assert_eq!(shard_filename(7, 0), "MS7S00.OBM");
        assert_eq!(shard_filename(999, 31), "MS999S31.OBM");
        assert_eq!(manifest_filename(7), "MS7.OBS");
        assert!(shard_filename(999, 31).split('.').next().unwrap().len() <= 8, "8.3 short name");
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
    }
}
