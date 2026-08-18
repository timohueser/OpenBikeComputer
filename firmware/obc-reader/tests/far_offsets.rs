//! The read seam past 4 GiB: a map whose every section lives beyond a `u32`, parsed and decoded.
//!
//! This is the one thing FS7.5-seam changed that anybody can observe. Until it landed,
//! `ByteSource` was `read_at(offset: u32) / len() -> u32`, so a file above 4 GiB was one OBCM v14
//! could express (§1.1's interior is `2^32 × U` = 64 GiB at the default scale) and nothing in this
//! tree could open. `obcm_assemble::FILE_CEILING` was the `min` of the two walls and the reader's
//! bound. Everything else about this slice is a type change; **this** is the capability.
//!
//! The map is a real one — `two_lod_file`'s bytes, feature for feature — relocated so that its
//! style table, LOD table, quadtree indexes, chunk data, POI directory and nav section all begin
//! past `BASE`. The header stays at byte 0, because that is the one offset the format fixes; the
//! §1.2 gap between it and the body is filler, which is exactly what a gap is. Nothing about the
//! *content* moves, so the assertions compare the far map's decode against the near map's: the
//! seam may change which byte a read names, never which byte it returns.

use obc_formats::io::Error as IoError;
use obc_map_scene::{BBox, Kind};
use obc_reader::{ByteSource, MapCache, MapTables, Reader, SliceSource};
use obcm_testkit::{
    build_file, empty_nav_directory, empty_poi_directory, pack_line, pack_poly_hole, resolve_offset, scaled, seal,
    LodSpec, Style, FILLER, STYLE_OFFSET, UNIT,
};
use std::cell::Cell;

mod common;
use common::decode_chunk;

const CS: usize = 64;
const GLOBAL: (i32, i32, i32, i32) = (0, 0, 1000, 1000);
const STYLES: &[Style] = &[(1, 3, 0xF800, 2, 3, false, None), (2, -1, 0x07E0, 1, 3, false, None)];

/// Where the relocated body begins: **5 GiB**, comfortably past `u32::MAX` and a whole number of
/// units at the default scale, so every offset that names it is still a legal scaled `uint32`
/// (5 GiB / 16 = 335,544,320 units, well inside the `2^32` §1.1 allows).
const BASE: usize = 5 << 30;
const _: () = assert!(BASE > u32::MAX as usize, "the whole point is to be past the old wall");
const _: () = assert!(BASE.is_multiple_of(UNIT), "a scaled offset cannot name a byte off the unit boundary");

fn two_lod_file() -> Vec<u8> {
    let line = seal(pack_line(1, 100, 200, &[(10, 0), (0, 10)]), CS);
    let poly = seal(
        pack_poly_hole(2, 100, 100, &[(100, 0), (0, 100), (-100, 0)], &[(25, 25), (50, 0), (0, 50), (-50, 0)]),
        CS,
    );
    build_file(
        GLOBAL,
        STYLES,
        &[
            LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![line], chunk_size: CS },
            LodSpec { max_mpp: 50.0, index: vec![0], chunks: vec![poly], chunk_size: CS },
        ],
    )
}

/// Rewrite every **absolute** offset in `near` to `BASE + itself`, leaving the bytes they name
/// where they are. The result is not a file — it is the body of one, to be served at `BASE` by
/// [`FarSource`].
///
/// The rewrite is exhaustive by construction rather than by search, which is why it is spelled out
/// rather than done with a scan: OBCM has exactly four absolute offsets in the header, one per LOD
/// table entry, and the two section directories carry their own. Everything else in the format is
/// *relative* — a LOD's chunk-offset table counts units from that LOD's `data_start`, a feature's
/// deltas count microdegrees from its anchor — which is precisely why relocating a map is
/// tractable at all, and is the same property `obcm-assemble`'s graft leans on.
fn relocate(near: &[u8]) -> Vec<u8> {
    let mut far = near.to_vec();
    let lod_tab = resolve_offset(near, 26);
    let poi_off = resolve_offset(near, 32);
    let nav_off = resolve_offset(near, 36);
    let lod_count = near[25] as usize;

    // The four header offsets: style table, LOD table, POI section, nav section.
    for at in [21usize, 26, 32, 36] {
        let moved = scaled(BASE + resolve_offset(near, at));
        far[at..at + 4].copy_from_slice(&moved.to_le_bytes());
    }
    // Each LOD entry's `Index Offset` (byte 4 of a 20-byte entry).
    for k in 0..lod_count {
        let at = lod_tab + k * obc_formats::obcm::LOD_ENTRY_LEN + 4;
        let moved = scaled(BASE + resolve_offset(near, at));
        far[at..at + 4].copy_from_slice(&moved.to_le_bytes());
    }
    // The two directories name offsets inside themselves, so they are **regenerated** at the moved
    // section base rather than patched field by field. `BASE` is a multiple of `U`, so each is
    // byte-for-byte the same length as the one it replaces — the §1.2 filler runs a directory
    // computes depend on `section_off % U` and nothing else.
    let poi = empty_poi_directory(BASE + poi_off);
    let nav = empty_nav_directory(BASE + nav_off);
    assert_eq!(poi.len(), nav_off - poi_off, "a relocated POI directory is the same length");
    assert_eq!(nav.len(), near.len() - nav_off, "…and so is the nav section");
    far[poi_off..poi_off + poi.len()].copy_from_slice(&poi);
    far[nav_off..nav_off + nav.len()].copy_from_slice(&nav);
    far
}

/// The relocated map as a source: the header at byte 0, `BASE - HEADER_LEN` bytes of §1.2 filler,
/// then the body. Sparse rather than allocated — five gibibytes of `0xFF` is a fact about the
/// address space, not something a test needs to own.
struct FarSource<'a> {
    /// The `relocate`d bytes, indexed by the offset they had **before** the move.
    body: &'a [u8],
    /// Reads whose first byte is at or past `BASE`, so a test can prove the far bytes were actually
    /// fetched rather than inferred.
    far_reads: Cell<u32>,
    /// The highest byte offset any read has named.
    high_water: Cell<u64>,
}

impl<'a> FarSource<'a> {
    fn new(body: &'a [u8]) -> Self {
        FarSource { body, far_reads: Cell::new(0), high_water: Cell::new(0) }
    }
}

impl ByteSource for FarSource<'_> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), IoError> {
        let end = offset.checked_add(buf.len() as u64).ok_or(IoError::BadOffset)?;
        if end > self.len() {
            return Err(IoError::BadOffset);
        }
        if offset >= BASE as u64 {
            self.far_reads.set(self.far_reads.get() + 1);
        }
        self.high_water.set(self.high_water.get().max(end));
        for (k, slot) in buf.iter_mut().enumerate() {
            let at = offset + k as u64;
            *slot = if at < obc_formats::obcm::HEADER_LEN as u64 {
                self.body[at as usize]
            } else if at >= BASE as u64 {
                self.body[(at - BASE as u64) as usize]
            } else {
                FILLER
            };
        }
        Ok(())
    }

    fn len(&self) -> u64 {
        BASE as u64 + self.body.len() as u64
    }
}

/// Every table the parse builds must resolve to a byte past the old wall — and the parse must
/// succeed, which before this slice it could not.
#[test]
fn a_map_laid_out_past_four_gibibytes_parses() {
    let near = two_lod_file();
    let far = relocate(&near);
    let src = FarSource::new(&far);

    assert!(src.len() > u32::MAX as u64, "the source itself is past the old wall");

    let tables = MapTables::parse(&src).expect("a map addressed past 4 GiB parses");
    assert_eq!(tables.version, obc_formats::obcm::VERSION);
    assert_eq!(tables.lods().len(), 2);
    for (k, lod) in tables.lods().iter().enumerate() {
        assert!(
            lod.index_offset > u32::MAX as u64,
            "LOD {k}'s quadtree index resolved to {}, which a u32 seam could have addressed",
            lod.index_offset
        );
    }
    assert!(src.far_reads.get() > 0, "the parse actually read past BASE rather than inferring it");
    assert!(src.high_water.get() > u32::MAX as u64, "and named a byte no u32 offset can");
}

/// The geometry is the same geometry. A relocated map's chunks decode feature-for-feature into what
/// the un-relocated one decodes — the seam moved the addressing and nothing else.
#[test]
fn geometry_past_four_gibibytes_decodes_identically_to_the_same_map_at_low_offsets() {
    let near = two_lod_file();
    let far = relocate(&near);

    let near_src = SliceSource(&near);
    let near_tables = MapTables::parse(&near_src).expect("the un-relocated map parses");
    let near_cache = MapCache::new();
    let near_reader = Reader::new(&near_src, &near_tables, &near_cache);

    let far_src = FarSource::new(&far);
    let far_tables = MapTables::parse(&far_src).expect("the relocated map parses");
    let far_cache = MapCache::new();
    let far_reader = Reader::new(&far_src, &far_tables, &far_cache);

    let view = BBox { min_lon: 0, min_lat: 0, max_lon: 1000, max_lat: 1000 };
    for lod in 0..2 {
        let mut near_leaves = Vec::new();
        near_reader.for_each_chunk(lod, &view, |cid, node| near_leaves.push((cid, node))).unwrap();
        let mut far_leaves = Vec::new();
        far_reader.for_each_chunk(lod, &view, |cid, node| far_leaves.push((cid, node))).unwrap();
        assert_eq!(near_leaves, far_leaves, "LOD {lod}'s quadtree walk must not depend on where the index sits");

        for &(cid, node) in &near_leaves {
            let a = decode_chunk(&near_reader, lod, cid, &node);
            let b = decode_chunk(&far_reader, lod, cid, &node);
            assert_eq!(a.len(), b.len(), "LOD {lod} chunk {cid}: feature count");
            for (x, y) in a.iter().zip(b.iter()) {
                assert_eq!(x.style_id, y.style_id);
                assert_eq!(x.kind, y.kind);
                assert_eq!(x.exterior, y.exterior, "LOD {lod} chunk {cid}: a relocated feature's vertices moved");
                assert_eq!(x.interiors, y.interiors, "LOD {lod} chunk {cid}: …or its holes");
                assert_eq!(x.bbox, y.bbox);
            }
        }
    }

    // Not a vacuous pass over an empty map: the fixture's two LODs are a line and a holed polygon.
    let line = decode_chunk(&far_reader, 0, 0, &view);
    assert_eq!(line.len(), 1);
    assert_eq!(line[0].kind, Kind::Line);
    let poly = decode_chunk(&far_reader, 1, 0, &view);
    assert_eq!(poly.len(), 1);
    assert_eq!(poly[0].kind, Kind::Polygon);
}

/// The fail-closed half. A resolved offset past the *source's* length is still refused — the wall
/// moved to where the bytes end rather than disappearing, and a `SliceSource` still cannot serve a
/// byte its host cannot address.
#[test]
fn a_section_past_the_sources_end_is_still_refused() {
    let near = two_lod_file();
    let far = relocate(&near);

    // Served through a source that stops short of `BASE`: every section the header names is now
    // outside it, so the parse must refuse rather than read filler as a style table.
    let truncated = SliceSource(&far[..STYLE_OFFSET]);
    assert!(MapTables::parse(&truncated).is_err(), "a map whose sections lie past the source is refused");

    // And the far map through a plain in-memory slice: `SliceSource` narrows to `usize`, which is
    // the host's address space rather than the format's wall, and it refuses rather than wraps.
    let whole = SliceSource(&far);
    assert!(MapTables::parse(&whole).is_err(), "the relocated offsets do not resolve inside the un-relocated bytes");
}
