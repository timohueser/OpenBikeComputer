//! The volume-set differential: a map split into an OBCA §5 set must render **pixel-identically**
//! to the monolithic file it was split from, at every zoom and — the case that matters — from
//! viewports that straddle a shard boundary.
//!
//! This is the tripwire for the whole multi-shard map source. `MapRenderer::render` is generic
//! over `MapScene`, so a `MountedSet` drops in where a `Reader` goes and the two sides differ
//! only in *how the bytes were laid out on the card*. Anything the dispatch loop gets wrong —
//! a shard skipped, a shard served another's chunks, a leaf bbox that no longer matches the
//! anchor base, a candidate visited in a different order — shows up as a different frame.
//!
//! The fixtures are hand-split by `obcm-testkit` (an independent oracle that calls no production
//! serializer), never by a host assembler: a differential whose two sides come from the same
//! producer proves nothing.

use embedded_graphics::pixelcolor::Rgb888;
use embedded_graphics::prelude::*;
use obc_formats::io::ByteSource;
use obc_formats::obcs;
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, MountedSet, Reader, SliceSource};
use obc_render::{MapRenderer, Viewport};
use obcm_testkit::set::{matched_pair, quadrants, SetFixture};
use obcm_testkit::{pack_line16, pack_poly16, seal, Style};

mod common;
use common::Buf;

/// (min_lon, min_lat, max_lon, max_lat). Midpoints are exact, so the four-quadrant split is
/// lossless and each shard's root node bbox equals the monolith's corresponding child bbox.
const ASSEMBLY: (i32, i32, i32, i32) = (0, 0, 4000, 4000);
const COARSE_MPP: f32 = f32::INFINITY;
/// The fine rung's ceiling, chosen so the zooms below straddle it: at ≈0.111 m per µdeg of
/// latitude, `mpp = 0.111 / zoom`, so zooms ≥ 0.075 select the split (fine) rung and zooms below
/// it fall back to the whole-assembly coarse shard.
const FINE_MPP: f32 = 1.5;
const CHUNK: usize = 4096;

// Four distinguishable colours: a mis-dispatch that served the wrong shard's chunk would land the
// wrong colour in the frame, not merely a differently-shaped one.
const NW_565: u16 = 0x07E0; // green
const NE_565: u16 = 0xF800; // red
const SW_565: u16 = 0x001F; // blue
const SE_565: u16 = 0xFFE0; // yellow
const COARSE_565: u16 = 0x07FF; // cyan

const STYLES: &[Style] = &[
    (1, 0, NW_565, 1, 1, false, None),
    (2, 0, NE_565, 1, 1, false, None),
    (3, 0, SW_565, 1, 1, false, None),
    (4, 0, SE_565, 1, 1, false, None),
    (5, 0, COARSE_565, 3, 1, false, None),
];

fn color(c: u16) -> Rgb888 {
    let (r, g, b) = rgb565_to_rgb888(c);
    Rgb888::new(r, g, b)
}

/// The four fine chunks, one per quadrant. Anchors are **relative to the leaf bbox**, which is the
/// quadrant in both layouts, so the packed bytes are identical on either side of the split.
///
/// Each quadrant carries a filled square well inside it plus a line that deliberately **overhangs**
/// the quadrant's edge. The overhang is the interesting one: a feature belongs to exactly one leaf
/// but its geometry may cross that leaf's bbox, so the monolith and the set must agree on when the
/// overhang is drawn (whenever the *owning* leaf/shard bbox meets the view) and when it is not.
fn fine_chunks() -> [Vec<u8>; 4] {
    let mut out = Vec::new();
    for (style, over) in [(1u8, (900i16, 900i16)), (2, (-900, 900)), (3, (900, -900)), (4, (-900, -900))] {
        let mut chunk = pack_poly16(style, 400, 400, &[(1200, 0), (0, 1200), (-1200, 0)]);
        // A 3-segment line starting inside the quadrant and running past one corner of it.
        chunk.extend_from_slice(&pack_line16(style, 1000, 1000, &[(over.0, 0), (0, over.1)]));
        out.push(seal(chunk, CHUNK));
    }
    [out[0].clone(), out[1].clone(), out[2].clone(), out[3].clone()]
}

/// One coarse chunk over the whole assembly — §5.1's single whole-assembly coarse shard, the file
/// a zoomed-out viewport reads and the only one it reads.
fn coarse_chunk() -> Vec<u8> {
    let mut chunk = pack_line16(5, 200, 200, &[(3600, 0), (0, 3600), (-3600, 0), (0, -3600)]);
    chunk.extend_from_slice(&pack_line16(5, 200, 3800, &[(3600, -3600)]));
    seal(chunk, CHUNK)
}

fn pair() -> (Vec<u8>, SetFixture) {
    matched_pair(ASSEMBLY, STYLES, (COARSE_MPP, coarse_chunk(), CHUNK), (FINE_MPP, fine_chunks(), CHUNK))
}

fn render_monolith(bytes: &[u8], vp: &Viewport) -> Buf {
    let mut buf = Buf::new(220, 220);
    let cache = MapCache::new();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("the monolith parses");
    let reader = Reader::new(&src, &tables, &cache);
    MapRenderer::new().render(&mut buf, &reader, vp, Rgb888::BLACK, color);
    buf
}

fn render_set(fixture: &SetFixture, vp: &Viewport) -> Buf {
    let mut buf = Buf::new(220, 220);
    let sources: Vec<SliceSource> = fixture.sources().into_iter().map(SliceSource).collect();
    let refs: Vec<&dyn ByteSource> = sources.iter().map(|s| s as &dyn ByteSource).collect();
    let manifest = obcs::parse(&fixture.manifest).expect("the manifest is valid");
    let core = MapTables::parse(&sources[manifest.core_shard as usize]).expect("the core parses");
    let cache = MapCache::new();
    let set = MountedSet::mount(manifest, &refs, &core, &cache).expect("a complete set mounts");
    MapRenderer::new().render(&mut buf, &set, vp, Rgb888::BLACK, color);
    buf
}

/// FNV-1a over the frame, so a failure message names a difference instead of dumping 48 400 pixels.
fn frame_hash(buf: &Buf) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for pixel in &buf.px {
        for byte in [pixel.r(), pixel.g(), pixel.b()] {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
    }
    hash
}

fn assert_identical(monolith: &[u8], fixture: &SetFixture, vp: &Viewport, what: &str) {
    let mono = render_monolith(monolith, vp);
    let set = render_set(fixture, vp);
    assert_eq!(
        frame_hash(&mono),
        frame_hash(&set),
        "{what}: the set's frame differs from the monolith's (mono painted {} px, set {} px)",
        mono.px.iter().filter(|&&p| p != Rgb888::BLACK).count(),
        set.px.iter().filter(|&&p| p != Rgb888::BLACK).count()
    );
    assert_eq!(mono.px, set.px, "{what}: pixel-for-pixel");
    assert!(mono.px.iter().any(|&p| p != Rgb888::BLACK), "{what}: the frame must not be empty to be meaningful");
}

/// The headline: the same viewport, both layouts, at a zoom that selects the split (fine) LOD, with
/// the camera at the assembly centre — the corner where **all four** geometry shards meet.
#[test]
fn a_viewport_straddling_all_four_shards_renders_identically() {
    let (monolith, fixture) = pair();
    let vp = Viewport::new(220.0, 220.0, 2000, 2000, 0.09);
    assert_identical(&monolith, &fixture, &vp, "four-way straddle at the assembly centre");

    // …and the frame really does carry all four shards' colours, or the test would pass vacuously.
    let frame = render_set(&fixture, &vp);
    for (name, rgb) in [("NW", NW_565), ("NE", NE_565), ("SW", SW_565), ("SE", SE_565)] {
        assert!(frame.count(color(rgb)) > 0, "the {name} shard contributed pixels");
    }
}

/// Every two-shard seam, walked one at a time: the vertical one (NW|NE), the horizontal one
/// (NW|SW), and the diagonal (NW|SE, which meet only at the centre point).
#[test]
fn every_shard_seam_renders_identically() {
    let (monolith, fixture) = pair();
    let seams = [
        ("vertical seam NW|NE", Viewport::new(220.0, 220.0, 2000, 3000, 0.12)),
        ("horizontal seam NW|SW", Viewport::new(220.0, 220.0, 1000, 2000, 0.12)),
        ("bottom-right seam SW|SE", Viewport::new(220.0, 220.0, 2000, 1000, 0.12)),
        ("top-right seam NE, edge of assembly", Viewport::new(220.0, 220.0, 3800, 3800, 0.12)),
    ];
    for (what, vp) in seams {
        assert_identical(&monolith, &fixture, &vp, what);
    }
}

/// A viewport wholly inside one quadrant: only that shard dispatches, and the neighbours'
/// **overhanging** geometry must stay absent on both sides. This is the direction a naive
/// "dispatch to everything" implementation would get wrong in the other direction — drawing an
/// overhang the monolith's quadtree walk never visits.
#[test]
fn a_viewport_inside_one_shard_renders_identically() {
    let (monolith, fixture) = pair();
    for (what, lon, lat) in
        [("inside NW", 800, 3200), ("inside NE", 3200, 3200), ("inside SW", 800, 800), ("inside SE", 3200, 800)]
    {
        let vp = Viewport::new(220.0, 220.0, lon, lat, 0.12);
        assert_identical(&monolith, &fixture, &vp, what);
    }
}

/// §5.6's payoff at the other end of the ladder: a zoomed-out viewport covers the whole map and
/// reads exactly one file — the coarse shard — and must still match the monolith, whose same
/// bytes sit in LOD 0 of a single file.
#[test]
fn the_coarse_ladder_rung_renders_identically() {
    let (monolith, fixture) = pair();
    for zoom in [0.05f32, 0.02, 0.01] {
        let vp = Viewport::new(220.0, 220.0, 2000, 2000, zoom);
        assert_identical(&monolith, &fixture, &vp, &format!("coarse rung at zoom {zoom}"));
    }
    // The coarse rung really is the one selected (not the fine one drawn small).
    let frame = render_set(&fixture, &Viewport::new(220.0, 220.0, 2000, 2000, 0.02));
    assert!(frame.count(color(COARSE_565)) > 0, "the coarse shard's geometry is on screen");
    assert_eq!(frame.count(color(NW_565)), 0, "and the fine rung is not");
}

/// A camera panned off the assembly entirely: both layouts draw nothing, and neither panics or
/// reads a shard it has no business opening.
#[test]
fn a_viewport_off_the_assembly_renders_identically() {
    let (monolith, fixture) = pair();
    let vp = Viewport::new(220.0, 220.0, 40_000, 40_000, 0.12);
    let mono = render_monolith(&monolith, &vp);
    let set = render_set(&fixture, &vp);
    assert_eq!(mono.px, set.px);
    assert_eq!(set.count(Rgb888::BLACK), (set.w * set.h) as usize, "nothing is drawn off the map");
}

/// The differential's own control. Swap two geometry shards' chunks — a set that still validates,
/// still mounts, and differs from the monolith only in *which file serves which quadrant* — and the
/// frames must come apart. Without this, a dispatch bug that quietly drew nothing would sail
/// through every assertion above.
#[test]
fn the_differential_detects_a_cross_served_shard() {
    use obc_formats::obcs::Role;
    use obcm_testkit::set::{build_set, empty_lod, ShardSpec};
    use obcm_testkit::LodSpec;

    let (monolith, _) = pair();
    let chunks = fine_chunks();
    // NW and NE keep their bboxes but trade payloads.
    let swapped = [chunks[1].clone(), chunks[0].clone(), chunks[2].clone(), chunks[3].clone()];
    let mut shards = vec![
        ShardSpec { role: Role::Core, bbox: ASSEMBLY, lods: vec![empty_lod(COARSE_MPP), empty_lod(FINE_MPP)] },
        ShardSpec {
            role: Role::Coarse,
            bbox: ASSEMBLY,
            lods: vec![
                LodSpec { max_mpp: COARSE_MPP, index: vec![0], chunks: vec![coarse_chunk()], chunk_size: CHUNK },
                empty_lod(FINE_MPP),
            ],
        },
    ];
    for (bbox, chunk) in quadrants(ASSEMBLY).into_iter().zip(swapped) {
        shards.push(ShardSpec {
            role: Role::Geometry,
            bbox,
            lods: vec![
                empty_lod(COARSE_MPP),
                LodSpec { max_mpp: FINE_MPP, index: vec![0], chunks: vec![chunk], chunk_size: CHUNK },
            ],
        });
    }
    let fixture = build_set(ASSEMBLY, STYLES, 0, &shards);
    let vp = Viewport::new(220.0, 220.0, 2000, 2000, 0.09);
    assert_ne!(
        render_monolith(&monolith, &vp).px,
        render_set(&fixture, &vp).px,
        "a cross-served shard must change the frame — otherwise the differential proves nothing"
    );
}

/// The §5.5 single-file fast path is the same map: a set of one renders identically to the
/// monolithic file, at every zoom. This is the common case — every selection below country-plus
/// scale — so it is the one that must never regress.
#[test]
fn the_single_file_fast_path_renders_identically() {
    use obc_formats::obcs::Role;
    use obcm_testkit::set::{build_set, ShardSpec};
    use obcm_testkit::{LodSpec, BRANCH_BIT};

    let (monolith, _) = pair();
    let fixture = build_set(
        ASSEMBLY,
        STYLES,
        0,
        &[ShardSpec {
            role: Role::Core,
            bbox: ASSEMBLY,
            lods: vec![
                LodSpec { max_mpp: COARSE_MPP, index: vec![0], chunks: vec![coarse_chunk()], chunk_size: CHUNK },
                LodSpec {
                    max_mpp: FINE_MPP,
                    index: vec![BRANCH_BIT | 1, 0, 1, 2, 3],
                    chunks: fine_chunks().to_vec(),
                    chunk_size: CHUNK,
                },
            ],
        }],
    );
    assert_eq!(fixture.shards[0], monolith, "a set of one holds the monolithic file unchanged");
    for zoom in [0.02f32, 0.09, 0.12] {
        let vp = Viewport::new(220.0, 220.0, 2000, 2000, zoom);
        assert_identical(&monolith, &fixture, &vp, &format!("single-file fast path at zoom {zoom}"));
    }
}
