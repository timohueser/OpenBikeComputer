//! What a bake produces, checked **through the shared reader** and against a closed-form oracle.
//!
//! Two rules shape every test here.
//!
//! 1. **Nothing in this file decodes an `.obcd`.** Baked bytes are read back with
//!    [`obc_elevation::TerrainReader`], the one normative consumer. A private decoder would prove
//!    that `obc-dem` agrees with `obc-dem`, which is not the claim.
//! 2. **The expected values are computed from the surface, not from the code under test.** The
//!    fixture is an affine plane, and `OBCT_Spec.md` §5.5 guarantees a bilinear sampler reproduces
//!    an affine function exactly — so the oracle is integer arithmetic on the plane's definition,
//!    independent of both the baker and the reader.

mod common;

use std::path::Path;

use common::{write_archive, ArchiveTile, Scratch, SyntheticDem, PIXEL_IS_AREA, PIXEL_IS_POINT};
use obc_dem::bake::{bake_cell, bake_cells, bake_shard, cell_file_name, cell_rect, quantise, BakeParams};
use obc_dem::container::CellRect;
use obc_dem::crest::{cell_window, LiftMap};
use obc_dem::geotiff::{DemMosaic, DemTile};
use obc_dem::reference::ReferenceArchive;
use obc_dem::BboxUdeg;
use obc_elevation::grid::{cell_base_sample, cell_of, lattice_coord, locate};
use obc_elevation::{TerrainReader, TileCache, DEFAULT_TILE_SLOTS};
use obc_formats::io::SliceSource;
use obc_formats::obct::NODATA;

/// The fixture pairing: the v1 posting with a **one-tile** cell, so a whole multi-cell rectangle is
/// a few KB. `OBCT_Spec.md` §1.3 exists to permit exactly this — the posting is the thing that
/// decides the *heights*, and it is the real one.
const POSTING_LOG2: u8 = 9;
const CELL_LOG2: u8 = 13;
/// Samples per cell edge at that pairing: 16, i.e. one tile.
const SPAN: i64 = 1 << (CELL_LOG2 - POSTING_LOG2);
/// Microdegrees per cell edge.
const CELL_UDEG: i64 = 1 << CELL_LOG2;

/// The plane's value at its base lattice point, metres.
const H0: i64 = 500;
/// Metres per posting north, and per posting east. Chosen so the plane's value at **every** lattice
/// point is an exact integer, which is what makes §5.5's exactness usable as an oracle.
const PER_POSTING_LAT: i64 = 1;
const PER_POSTING_LON: i64 = 3;

/// The lattice point the plane is anchored on: the minimum sample of the cell containing 47 °N 8 °E.
fn base_sample_udeg() -> (i32, i32) {
    let at = locate(47_000_000, 8_000_000, POSTING_LOG2).unwrap();
    let ci = cell_of(at.i, POSTING_LOG2, CELL_LOG2);
    let cj = cell_of(at.j, POSTING_LOG2, CELL_LOG2);
    (
        lattice_coord(cell_base_sample(ci, POSTING_LOG2, CELL_LOG2), POSTING_LOG2),
        lattice_coord(cell_base_sample(cj, POSTING_LOG2, CELL_LOG2), POSTING_LOG2),
    )
}

/// The plane, in metres, at a µdeg coordinate — the surface the synthetic source *is*.
fn plane_metres(lat_udeg: f64, lon_udeg: f64) -> f64 {
    let (base_lat, base_lon) = base_sample_udeg();
    H0 as f64
        + (lat_udeg - f64::from(base_lat)) * PER_POSTING_LAT as f64 / 512.0
        + (lon_udeg - f64::from(base_lon)) * PER_POSTING_LON as f64 / 512.0
}

/// What the **reader** must answer at `(lat, lon)`: the plane, rounded half away from zero, in exact
/// integer arithmetic. Derived from §5.2 and §5.5, not from either implementation.
fn oracle(lat_udeg: i32, lon_udeg: i32) -> i16 {
    let (base_lat, base_lon) = base_sample_udeg();
    let a = i64::from(lat_udeg) - i64::from(base_lat);
    let b = i64::from(lon_udeg) - i64::from(base_lon);
    let num = H0 * 512 + a * PER_POSTING_LAT + b * PER_POSTING_LON;
    let h = if num >= 0 { (num + 256) / 512 } else { -((-num + 256) / 512) };
    h as i16
}

/// A source raster of the plane at a 1 arcsecond posting — GLO-30's own spacing, deliberately
/// **not** a multiple of the target posting, so every baked sample is a genuine interpolation
/// rather than a lucky coincidence with a source post.
fn plane_source(raster_type: u16) -> SyntheticDem {
    let (base_lat, base_lon) = base_sample_udeg();
    let step = 1.0 / 3600.0;
    // North-west corner: two cells' worth of margin north of the plane's base, so the three baked
    // cell rows sit comfortably inside coverage.
    let tie_lat = (f64::from(base_lat) + 3.0 * CELL_UDEG as f64 + 2_000.0) / 1e6;
    let tie_lon = (f64::from(base_lon) - 2_000.0) / 1e6;
    SyntheticDem::build(tie_lat, tie_lon, step, 120, 120, raster_type, |lat_deg, lon_deg| {
        plane_metres(lat_deg * 1e6, lon_deg * 1e6) as f32
    })
}

/// The 3 × 3 cell box the fixtures bake: from the plane's base sample to two cells north-east.
fn fixture_bbox() -> BboxUdeg {
    let (base_lat, base_lon) = base_sample_udeg();
    BboxUdeg {
        min_lat: base_lat,
        min_lon: base_lon,
        max_lat: (i64::from(base_lat) + 2 * CELL_UDEG) as i32,
        max_lon: (i64::from(base_lon) + 2 * CELL_UDEG) as i32,
    }
}

fn fixture_params() -> BakeParams {
    BakeParams { posting_log2: POSTING_LOG2, cell_log2: CELL_LOG2, bbox: fixture_bbox() }
}

/// Bake the plane fixture into a shard, in memory.
fn bake_plane_shard(raster_type: u16) -> Vec<u8> {
    let mut mosaic = DemMosaic::default();
    let scratch = Scratch::new("plane-source");
    let path = plane_source(raster_type).write(scratch.path(), "plane");
    mosaic.push(DemTile::open(&path).unwrap());
    let mut out = std::io::Cursor::new(Vec::new());
    bake_shard(&mosaic, fixture_params(), None, &mut out, |_| {}).unwrap();
    out.into_inner()
}

/// Sample a container through the shared reader.
fn sample(bytes: &[u8], lat_udeg: i32, lon_udeg: i32) -> Option<i16> {
    let src = SliceSource(bytes);
    let reader = TerrainReader::parse(&src).unwrap();
    let mut cache = TileCache::<DEFAULT_TILE_SLOTS>::new();
    reader.sample(&mut cache, lat_udeg, lon_udeg)
}

// ===================================================================================================

/// The headline round trip: bake a plane, read it back through `TerrainReader`, and check every
/// sample against the closed-form oracle — on the lattice, off the lattice, and at a tile seam.
#[test]
fn a_baked_plane_reads_back_as_that_plane() {
    let bytes = bake_plane_shard(PIXEL_IS_POINT);
    let (base_lat, base_lon) = base_sample_udeg();

    // Every lattice point of the 3 × 3 rectangle: 48 × 48 samples. A query with both remainders
    // zero collapses to the stored sample, so this checks the *stored raster* exactly.
    for di in 0..3 * SPAN {
        for dj in 0..3 * SPAN {
            let lat = (i64::from(base_lat) + di * 512) as i32;
            let lon = (i64::from(base_lon) + dj * 512) as i32;
            assert_eq!(sample(&bytes, lat, lon), Some(oracle(lat, lon)), "lattice sample ({di}, {dj})");
        }
    }

    // Off-lattice queries, including ones that straddle a tile and a cell seam. §5.5 says an affine
    // surface interpolates exactly, so the same oracle holds at arbitrary coordinates.
    for (dlat, dlon) in [
        (1i64, 1i64),
        (255, 255),
        (256, 256),
        (511, 511),
        (SPAN * 512 - 1, 7),
        (SPAN * 512, SPAN * 512),
        (2 * SPAN * 512 - 3, 2 * SPAN * 512 - 3),
    ] {
        let lat = (i64::from(base_lat) + dlat) as i32;
        let lon = (i64::from(base_lon) + dlon) as i32;
        assert_eq!(sample(&bytes, lat, lon), Some(oracle(lat, lon)), "off-lattice query (+{dlat}, +{dlon}) µdeg");
    }
}

/// `PixelIsArea` moves every post half a step, and the tie point means something different — a
/// fixture written each way must still bake the same surface, because the surface is the same.
#[test]
fn both_geotiff_raster_conventions_bake_the_same_surface() {
    assert_eq!(bake_plane_shard(PIXEL_IS_POINT), bake_plane_shard(PIXEL_IS_AREA));
}

/// **Determinism is a contract.** Two bakes of the same inputs are byte-identical, and the bytes
/// are pinned: a change to the resample, the rounding rule, the layout or the iteration order has
/// to arrive as a deliberate edit to this number.
#[test]
fn the_same_inputs_bake_the_same_bytes_forever() {
    let first = bake_plane_shard(PIXEL_IS_POINT);
    let second = bake_plane_shard(PIXEL_IS_POINT);
    assert_eq!(first, second, "two bakes of one fixture differ — something is reading the environment");

    // 32 header + 9 × 4 directory + 9 × 512 block.
    assert_eq!(first.len(), 32 + 36 + 9 * 512);
    assert_eq!(
        common::sha256_hex(&first),
        "ebe99765e23d1f4b4d237df8984515f87ced1e610c7209bd32b1c245b8f25878",
        "the plane fixture's bytes changed — if that was intended, state why in the PR"
    );
}

/// A cell is a pure function of the mosaic and its own index: the same square baked alone and baked
/// inside a wide shard is the same 512 bytes. This is what makes a published cell (EL3) and an
/// assembled shard (EL4) two views of one raster rather than two rasters.
#[test]
fn a_cell_is_the_same_bytes_alone_as_inside_a_shard() {
    let scratch = Scratch::new("cell-vs-shard");
    let path = plane_source(PIXEL_IS_POINT).write(scratch.path(), "plane");
    let mut mosaic = DemMosaic::default();
    mosaic.push(DemTile::open(&path).unwrap());

    let shard = bake_plane_shard(PIXEL_IS_POINT);
    let rect = cell_rect(fixture_bbox(), POSTING_LOG2, CELL_LOG2).unwrap();
    assert_eq!(rect, CellRect { min_i: rect.min_i, min_j: rect.min_j, rows: 3, cols: 3 });

    for (slot, (ci, cj)) in rect.cells().enumerate() {
        let alone =
            bake_cell(&mosaic, ci, cj, POSTING_LOG2, CELL_LOG2, None).expect("the plane covers every fixture cell");
        let offset = u32::from_le_bytes(shard[32 + slot * 4..36 + slot * 4].try_into().unwrap()) as usize;
        assert_ne!(offset, 0, "cell {ci}/{cj} should be present in the shard");
        assert_eq!(&shard[offset..offset + 512], &alone[..], "cell {ci}/{cj} differs between the two bakes");
    }
}

// === the reference archive ==========================================================================

/// A reference archive over exactly the tiles a list of cells reads, halo included, filled from a
/// surface in µdeg. `None` leaves a pixel absent, which is how a fixture stops coverage mid-cell.
///
/// The heights are whole metres because an archive pixel is: the rule sees the archive's own
/// rounding, not a continuous function.
fn write_reference(root: &Path, cells: &[(u32, u32)], surface: impl Fn(f64, f64) -> Option<f64>) {
    let mut ids = std::collections::BTreeSet::new();
    for &(ci, cj) in cells {
        ids.extend(cell_window(ci, cj, POSTING_LOG2, CELL_LOG2).expect("the fixture pairing").tiles());
    }
    let tiles: Vec<ArchiveTile> = ids
        .into_iter()
        .map(|(ti, tj)| ArchiveTile::filled(ti, tj, |lat, lon| surface(lat as f64, lon as f64).map(quantise)))
        .collect();
    assert!(!tiles.is_empty(), "a fixture archive with no tile proves nothing");
    write_archive(root, "ch", &tiles);
}

/// A cone one posting wide on a **steep** planar slope — the fixture the lift rule is judged on. The
/// slope is what the convexity gate has to leave alone: a coarse lattice under-samples a 40° face
/// honestly, and an ungated rule inflates the whole mountain.
struct Cone {
    peak: (f64, f64),
    height_m: f64,
}

impl Cone {
    /// A cone standing on node `(y, x)` of cell `(ci, cj)`.
    fn on_node(ci: u32, cj: u32, y: i64, x: i64, height_m: f64) -> Cone {
        let (lat, lon) = node_udeg(ci, cj, y, x);
        Cone { peak: (f64::from(lat), f64::from(lon)), height_m }
    }

    /// The slope alone, in metres at a µdeg coordinate — what the native lattice measures.
    fn slope(lat: f64, lon: f64) -> f64 {
        let (base_lat, base_lon) = base_sample_udeg();
        (lat - f64::from(base_lat)) / 512.0 * 40.0 + (lon - f64::from(base_lon)) / 512.0 * 8.0
    }

    /// The reference surface: the slope with the cone on it.
    fn reference(&self, lat: f64, lon: f64) -> f64 {
        let d = ((lat - self.peak.0).powi(2) + (lon - self.peak.1).powi(2)).sqrt() / 512.0;
        Cone::slope(lat, lon) + (self.height_m - self.height_m * d).max(0.0)
    }

    /// The native lattice: the slope, quantised, with no cone in it at all.
    fn native(&self) -> impl Fn(i32, i32) -> i16 {
        |lat: i32, lon: i32| quantise(Cone::slope(f64::from(lat), f64::from(lon)))
    }
}

/// The µdeg coordinate of node `(y, x)` of cell `(ci, cj)`.
fn node_udeg(ci: u32, cj: u32, y: i64, x: i64) -> (i32, i32) {
    let base_i = cell_base_sample(ci, POSTING_LOG2, CELL_LOG2);
    let base_j = cell_base_sample(cj, POSTING_LOG2, CELL_LOG2);
    (lattice_coord(base_i + y as u32, POSTING_LOG2), lattice_coord(base_j + x as u32, POSTING_LOG2))
}

/// The lift the rule gives node `(y, x)` of cell `(ci, cj)`.
fn lift_at(map: &LiftMap, ci: u32, cj: u32, y: i64, x: i64) -> i16 {
    let (lat, lon) = node_udeg(ci, cj, y, x);
    map.at(lat, lon)
}

/// A 120 m cone one posting wide, standing on the node the fixture's first two cells share — the
/// eastern seam of cell `(0, 0)`, half way up it. Everywhere else the reference **is** the plane the
/// source DEM is, so §9's rule finds nothing there and only the cells around the seam can move.
fn seam_cone_archive(root: &Path) {
    let (base_lat, base_lon) = base_sample_udeg();
    let peak_lat = f64::from(base_lat) + 8.0 * 512.0;
    let peak_lon = f64::from(base_lon) + SPAN as f64 * 512.0;
    let rect = cell_rect(fixture_bbox(), POSTING_LOG2, CELL_LOG2).unwrap();
    let cells: Vec<(u32, u32)> = rect.cells().collect();
    write_reference(root, &cells, |lat, lon| {
        let d = ((lat - peak_lat).powi(2) + (lon - peak_lon).powi(2)).sqrt() / 512.0;
        Some(plane_metres(lat, lon) + (120.0 - 120.0 * d).max(0.0))
    });
}

/// A reference is composed **per cell**, so all three ways to bake a cell must still produce one
/// raster: inside a wide shard, as a file of its own, and through `bake_cell` over a lift map the
/// caller built itself. The cone stands on a seam, which is where the two cells have to agree about
/// a node neither of them owns alone.
///
/// It also pins the two size claims §9.2 makes: a reference costs no bytes, and a cell with
/// coverage is the same length as one without.
#[test]
fn a_reference_composes_the_same_cell_bytes_in_all_three_bakes() {
    let scratch = Scratch::new("reference-composition");
    let path = plane_source(PIXEL_IS_POINT).write(scratch.path(), "plane");
    let mut mosaic = DemMosaic::default();
    mosaic.push(DemTile::open(&path).unwrap());
    let archive_root = Scratch::new("reference-archive");
    seam_cone_archive(archive_root.path());
    let archive = ReferenceArchive::open(archive_root.path()).unwrap();
    let reference = Some(&archive);

    let mut shard = std::io::Cursor::new(Vec::new());
    let report = bake_shard(&mosaic, fixture_params(), reference, &mut shard, |_| {}).unwrap();
    assert_eq!(
        report.sources.iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["ch"],
        "the archive's attribution must reach the report"
    );
    let shard = shard.into_inner();
    // The digest pin: the rule is one pass over archive pixels, and a change to the pass, the gate,
    // the dilation or the iteration order has to arrive as a deliberate edit to this number.
    assert_eq!(
        common::sha256_hex(&shard),
        "677d91af6c137de0dce25baf16a255d667999622605b19fb106db8ad88618982",
        "the lifted plane fixture's bytes changed — if that was intended, state why in the PR"
    );
    let plain = bake_plane_shard(PIXEL_IS_POINT);
    assert_eq!(shard.len(), plain.len(), "a reference costs no bytes");
    assert_ne!(shard, plain, "a cone on the seam has to move some sample");

    let published = Scratch::new("reference-cells");
    bake_cells(&mosaic, fixture_params(), reference, published.path(), |_| {}).unwrap();

    // The same native sampler the baker composes the lift map over, rebuilt here from the public
    // pieces — which is the whole claim of `bake_cell`'s `Option<&LiftMap>` argument.
    let native = |lat: i32, lon: i32| match mosaic.height(f64::from(lat) / 1e6, f64::from(lon) / 1e6) {
        Some(metres) => quantise(metres),
        None => NODATA,
    };

    let rect = cell_rect(fixture_bbox(), POSTING_LOG2, CELL_LOG2).unwrap();
    let mut moved = 0;
    for (slot, (ci, cj)) in rect.cells().enumerate() {
        let lift = LiftMap::bake(ci, cj, POSTING_LOG2, CELL_LOG2, native, &archive).unwrap();
        let alone = bake_cell(&mosaic, ci, cj, POSTING_LOG2, CELL_LOG2, lift.as_ref())
            .expect("the plane covers every fixture cell");
        assert_eq!(alone.len(), 512, "a lifted cell is the same length as a plain one");

        let in_shard = u32::from_le_bytes(shard[32 + slot * 4..36 + slot * 4].try_into().unwrap()) as usize;
        assert_ne!(in_shard, 0, "cell {ci}/{cj} should be present in the shard");
        let baked = &shard[in_shard..in_shard + 512];
        assert_eq!(baked, &alone[..], "cell {ci}/{cj} differs between the shard and the lone bake");

        let file = std::fs::read(published.path().join(cell_file_name(CELL_LOG2, ci, cj))).unwrap();
        let in_file = u32::from_le_bytes(file[32..36].try_into().unwrap()) as usize;
        assert_eq!(&file[in_file..in_file + 512], &alone[..], "cell {ci}/{cj} differs between its file and the bake");

        // The plain shard has the same layout, so the same offset names the same square in it.
        moved += i32::from(baked != &plain[in_shard..in_shard + 512]);
    }
    assert!(moved >= 2, "the cone sits on a seam, so both cells either side of it must carry a lift");
}

/// The rule itself, over an archive: the tower is lifted to its own height, the steep plane it
/// stands on is not, and the lift reaches the tip's neighbours but no further.
#[test]
fn a_tower_is_lifted_and_the_plane_it_stands_on_is_not() {
    let rect = cell_rect(fixture_bbox(), POSTING_LOG2, CELL_LOG2).unwrap();
    let (ci, cj) = (rect.min_i, rect.min_j);
    let cone = Cone::on_node(ci, cj, 8, 8, 120.0);
    let scratch = Scratch::new("tower");
    write_reference(scratch.path(), &[(ci, cj)], |lat, lon| Some(cone.reference(lat, lon)));
    let archive = ReferenceArchive::open(scratch.path()).unwrap();

    let map =
        LiftMap::bake(ci, cj, POSTING_LOG2, CELL_LOG2, cone.native(), &archive).unwrap().expect("the tower is a crest");
    let at = |y, x| lift_at(&map, ci, cj, y, x);

    assert!((100..=130).contains(&at(8, 8)), "the tip is lifted to the tower's own height, got {}", at(8, 8));
    assert_eq!(at(1, 1), 0, "the planar slope seven nodes away is left alone");
    assert_eq!(at(14, 14), 0, "and so is the far side of it");
    assert!(at(8, 9) > 0, "the one-node extension reaches the tip's neighbour");

    // The tally is what an operator sees: how much, how many, and where the worst of it is.
    let tally = map.tally();
    assert!(tally.nodes < 40, "the rule stays local to the tower, lifted {}", tally.nodes);
    assert_eq!(tally.max_m, at(8, 8), "the largest lift is the tip's");
    assert_eq!(tally.max_at, node_udeg(ci, cj, 8, 8), "and it is reported at the tip");
    assert_eq!(tally.over_report, 0, "a 120 m tower is not worth an operator's attention");

    // `apply` adds the lift, leaves a hole a hole, and leaves a coordinate the map does not
    // describe alone.
    let (tip_lat, tip_lon) = node_udeg(ci, cj, 8, 8);
    let native = cone.native();
    let mut lifted = map.apply(cone.native());
    assert_eq!(lifted(tip_lat, tip_lon), native(tip_lat, tip_lon) + at(8, 8));
    let mut over_hole =
        map.apply(|lat: i32, lon: i32| if (lat, lon) == (tip_lat, tip_lon) { NODATA } else { native(lat, lon) });
    assert_eq!(over_hole(tip_lat, tip_lon), NODATA, "a lift never fills a hole");
    assert_eq!(map.at(tip_lat + 256, tip_lon), 0, "half a posting off the lattice is not a node");
}

/// A reference that stands a uniform **8 m** above our surface lifts nothing, however steep and
/// however convex the mountain under it — and this is the one thing measuring the gap against the
/// *roof* of a pooled pixel's footprint buys.
///
/// The native lattice is a pyramid of 40 m per posting, so the bilinear surface through it is the
/// pyramid exactly (each interval is affine) and `node_max` is sharply convex at the apex: the gate
/// this test isolates is the 10 m one. An archive pixel is the maximum of the source inside a
/// 7.1 × 4.9 m square, so on that slope the pooled value sits about 3 m above the surface at the
/// pixel's own centre. Measured from the centre the 8 m offset reads as an 11 m gap and the apex is
/// lifted; measured from the roof it reads as the 8 m it is, and §9 refuses it.
///
/// A DTM reading a metre or two above a surface model over bare rock is the ordinary case, not a
/// contrived one, which is why this must not become a lift.
#[test]
fn a_reference_below_the_gate_lifts_nothing_however_steep_the_mountain() {
    let rect = cell_rect(fixture_bbox(), POSTING_LOG2, CELL_LOG2).unwrap();
    let (ci, cj) = (rect.min_i, rect.min_j);
    let (apex_lat, apex_lon) = node_udeg(ci, cj, 8, 8);
    // Per posting: 40 m down each axis from the apex. Integral at every node, and affine inside
    // every lattice interval, so the native bilinear is this pyramid to the bit.
    let pyramid = |lat: f64, lon: f64| {
        let dy = (lat - f64::from(apex_lat)).abs() / 512.0;
        let dx = (lon - f64::from(apex_lon)).abs() / 512.0;
        2000.0 - 40.0 * dy - 40.0 * dx
    };
    let native = |lat: i32, lon: i32| quantise(pyramid(f64::from(lat), f64::from(lon)));

    let scratch = Scratch::new("under-the-gate");
    write_reference(scratch.path(), &[(ci, cj)], |lat, lon| Some(pyramid(lat, lon) + 8.0));
    let archive = ReferenceArchive::open(scratch.path()).unwrap();
    assert!(
        LiftMap::bake(ci, cj, POSTING_LOG2, CELL_LOG2, native, &archive).unwrap().is_none(),
        "8 m is not 10 m, and the slope under it must not make up the difference"
    );

    // The same mountain under a reference that *is* 12 m above it is a crest, so the test above is
    // about the gate and not about a fixture that could never lift anything.
    let over = Scratch::new("over-the-gate");
    write_reference(over.path(), &[(ci, cj)], |lat, lon| Some(pyramid(lat, lon) + 12.0));
    let archive = ReferenceArchive::open(over.path()).unwrap();
    let map = LiftMap::bake(ci, cj, POSTING_LOG2, CELL_LOG2, native, &archive).unwrap().expect("12 m clears the gate");
    assert_eq!(lift_at(&map, ci, cj, 8, 8), 12, "and the apex rises by exactly what the reference says");
}

/// The whole point of the halo: a node two or four cells share is lifted by the same amount
/// whichever of them computes it. The tower stands on the node all four cells of a 2 × 2 block
/// share, so one fixture exercises both seams, the corner, the convexity ring across a seam and the
/// dilation across it — and, because the reference is an archive, the tiles the halo reads over the
/// cell edge as well.
#[test]
fn cells_agree_on_every_node_they_share() {
    let rect = cell_rect(fixture_bbox(), POSTING_LOG2, CELL_LOG2).unwrap();
    let (ci, cj) = (rect.min_i, rect.min_j);
    let block: Vec<(u32, u32)> = vec![(ci, cj), (ci, cj + 1), (ci + 1, cj), (ci + 1, cj + 1)];
    let cone = Cone::on_node(ci, cj, SPAN, SPAN, 120.0);
    let scratch = Scratch::new("shared-nodes");
    write_reference(scratch.path(), &block, |lat, lon| Some(cone.reference(lat, lon)));
    let archive = ReferenceArchive::open(scratch.path()).unwrap();
    let map = |ci, cj| {
        LiftMap::bake(ci, cj, POSTING_LOG2, CELL_LOG2, cone.native(), &archive)
            .unwrap()
            .expect("the corner tower is a crest in every cell of the block")
    };
    let (nw, ne, sw, se) = (map(ci, cj), map(ci, cj + 1), map(ci + 1, cj), map(ci + 1, cj + 1));

    let mut shared = 0;
    // Each row's eastern seam: cell (i, cj)'s inclusive high edge is cell (i, cj + 1)'s column 0.
    for (west, east, row) in [(&nw, &ne, ci), (&sw, &se, ci + 1)] {
        for y in 0..=SPAN {
            let (lat, lon) = node_udeg(row, cj, y, SPAN);
            assert_eq!(west.at(lat, lon), east.at(lat, lon), "row {row} seam node {y} disagrees");
            shared += i32::from(west.at(lat, lon) > 0);
        }
    }
    // Each column's northern seam: cell (ci, j)'s high edge is cell (ci + 1, j)'s row 0.
    for (south, north, col) in [(&nw, &sw, cj), (&ne, &se, cj + 1)] {
        for x in 0..=SPAN {
            let (lat, lon) = node_udeg(ci, col, SPAN, x);
            assert_eq!(south.at(lat, lon), north.at(lat, lon), "column {col} seam node {x} disagrees");
            shared += i32::from(south.at(lat, lon) > 0);
        }
    }
    assert!(shared > 0, "the test would pass on four empty edges");

    // The node all four describe, which is the one the tower stands on.
    let (lat, lon) = node_udeg(ci, cj, SPAN, SPAN);
    let corner: Vec<i16> = [&nw, &ne, &sw, &se].map(|m| m.at(lat, lon)).into();
    assert!(corner.iter().all(|&lift| lift == corner[0]), "the shared corner disagrees: {corner:?}");
    assert!(corner[0] > 0, "the tower stands on that corner");
}

/// Coverage that stops inside a cell: the archive's pixels end on the cell's own seam, and a ridge
/// one posting into the neighbour lifts the seam node by dilation alone. The western cell owns no
/// lifted node at all, so its map has to exist for that one node or the two cells disagree about a
/// node neither owns alone.
///
/// It is also the coverage-edge case in the pass itself — absent pixels, not absent tiles.
#[test]
fn a_cell_lifted_only_on_its_seam_still_has_a_map() {
    let rect = cell_rect(fixture_bbox(), POSTING_LOG2, CELL_LOG2).unwrap();
    let (ci, cj) = (rect.min_i, rect.min_j);
    let (_, seam_lon) = node_udeg(ci, cj, 0, SPAN);
    let ridge_lon = f64::from(seam_lon) + 512.0;
    let scratch = Scratch::new("seam-only");
    write_reference(scratch.path(), &[(ci, cj), (ci, cj + 1)], |lat, lon| {
        if lon < f64::from(seam_lon) {
            return None;
        }
        let d = (lon - ridge_lon).abs() / 512.0;
        Some(Cone::slope(lat, lon) + (120.0 - 120.0 * d).max(0.0))
    });
    let archive = ReferenceArchive::open(scratch.path()).unwrap();
    let native = |lat: i32, lon: i32| quantise(Cone::slope(f64::from(lat), f64::from(lon)));

    let west =
        LiftMap::bake(ci, cj, POSTING_LOG2, CELL_LOG2, native, &archive).unwrap().expect("the seam node is lifted");
    let east =
        LiftMap::bake(ci, cj + 1, POSTING_LOG2, CELL_LOG2, native, &archive).unwrap().expect("and so is its column 0");
    assert_eq!(west.tally().nodes, 0, "the west cell owns no lifted node, which is the trap");

    let mut lifted = 0;
    for y in 0..=SPAN {
        let (lat, lon) = node_udeg(ci, cj, y, SPAN);
        assert_eq!(west.at(lat, lon), east.at(lat, lon), "seam node {y} disagrees");
        lifted += i32::from(west.at(lat, lon) > 0);
    }
    assert!(lifted > 0, "the ridge must reach the seam by dilation, or the test proves nothing");
}

/// A tile whose transform is not the lattice its id names is refused **by name**, rather than
/// sliding every height in it by the error. Half a pixel — 32 µdeg, 3.5 m — is the size of mistake
/// a hand-cut tile makes, and it is 32 000 times the tolerance the contract states.
#[test]
fn a_tile_whose_transform_is_not_the_lattice_is_refused_by_name() {
    let rect = cell_rect(fixture_bbox(), POSTING_LOG2, CELL_LOG2).unwrap();
    let (ci, cj) = (rect.min_i, rect.min_j);
    let scratch = Scratch::new("skewed-tile");
    let window = cell_window(ci, cj, POSTING_LOG2, CELL_LOG2).unwrap();
    let (ti, tj) = window.tiles().next().expect("the cell reads at least one tile");
    let mut tile = ArchiveTile::filled(ti, tj, |lat, lon| Some(quantise(Cone::slope(lat as f64, lon as f64))));
    tile.skew_deg = 32e-6;
    write_archive(scratch.path(), "ch", &[tile]);
    let archive = ReferenceArchive::open(scratch.path()).unwrap();

    let error = archive.tile(ti, tj).expect_err("a tile off the lattice must be refused");
    assert!(error.contains(&format!("{tj:04}.tif")), "the refusal must name the file: {error}");
    assert!(error.contains("origin lat"), "and say what is wrong with it: {error}");
    // And the refusal reaches the bake rather than turning into a silent absence of lifts.
    let native = |lat: i32, lon: i32| quantise(Cone::slope(f64::from(lat), f64::from(lon)));
    assert!(LiftMap::bake(ci, cj, POSTING_LOG2, CELL_LOG2, native, &archive).is_err());
}

/// The seam rule from both sides: two adjacent cells baked **independently** hand the reader a
/// continuous surface. The cells share no sample — ownership is half-open (§3.1) — so continuity is
/// a claim about the cross-cell fetch of §5.3 step 2, and the oracle is the only thing that can
/// judge it.
#[test]
fn adjacent_cells_baked_independently_meet_without_a_step() {
    let bytes = bake_plane_shard(PIXEL_IS_POINT);
    let (base_lat, base_lon) = base_sample_udeg();
    // The latitude boundary between the fixture's first and second cell rows.
    let seam_lat = (i64::from(base_lat) + CELL_UDEG) as i32;
    let seam_lon = (i64::from(base_lon) + CELL_UDEG) as i32;

    for step in [-512i64, -256, -1, 0, 1, 256, 512] {
        let lat = (i64::from(seam_lat) + step) as i32;
        assert_eq!(
            sample(&bytes, lat, base_lon + 700),
            Some(oracle(lat, base_lon + 700)),
            "across the latitude seam at {step:+} µdeg"
        );
        let lon = (i64::from(seam_lon) + step) as i32;
        assert_eq!(
            sample(&bytes, base_lat + 700, lon),
            Some(oracle(base_lat + 700, lon)),
            "across the longitude seam at {step:+} µdeg"
        );
    }

    // And the corner where four cells meet.
    for (dlat, dlon) in [(-1i64, -1i64), (-1, 0), (0, -1), (0, 0)] {
        let lat = (i64::from(seam_lat) + dlat) as i32;
        let lon = (i64::from(seam_lon) + dlon) as i32;
        assert_eq!(sample(&bytes, lat, lon), Some(oracle(lat, lon)), "the four-cell corner at ({dlat:+}, {dlon:+})");
    }
}

/// A void in the source becomes a void in the raster and stays one: `NaN` in, `NODATA` out, `None`
/// at every query whose stencil touches it. Nothing is inpainted at any stage.
#[test]
fn a_source_void_propagates_all_the_way_to_none() {
    let scratch = Scratch::new("void");
    let mut dem = plane_source(PIXEL_IS_POINT);
    // Punch a hole near the middle of the raster and remember where it is on the ground.
    let (row, col) = (60usize, 60usize);
    let half = 0.0; // PixelIsPoint: the tie point is post (0, 0)
    let hole_lat_udeg = (dem.tie_lat_deg - half - dem.step_deg * row as f64) * 1e6;
    let hole_lon_udeg = (dem.tie_lon_deg + half + dem.step_deg * col as f64) * 1e6;
    dem.set_north_up(row, col, f32::NAN);
    let path = dem.write(scratch.path(), "holed");
    let mut mosaic = DemMosaic::default();
    mosaic.push(DemTile::open(&path).unwrap());

    let mut out = std::io::Cursor::new(Vec::new());
    bake_shard(&mosaic, fixture_params(), None, &mut out, |_| {}).unwrap();
    let bytes = out.into_inner();

    // The nearest lattice point to the hole is voided — the stencil that produced it had a NaN
    // corner, and §5.4's rule is the same on the write side as on the read side.
    let at = locate(hole_lat_udeg as i32, hole_lon_udeg as i32, POSTING_LOG2).unwrap();
    let (lat, lon) = (lattice_coord(at.i, POSTING_LOG2), lattice_coord(at.j, POSTING_LOG2));
    assert_eq!(sample(&bytes, lat, lon), None, "the lattice point over the hole must be silent");

    // The void is bounded: a lattice point four postings away is untouched and still on the plane.
    let far_lat = (i64::from(lat) + 4 * 512) as i32;
    let far_lon = (i64::from(lon) + 4 * 512) as i32;
    assert_eq!(sample(&bytes, far_lat, far_lon), Some(oracle(far_lat, far_lon)), "one hole must not void a cell");

    // And the sentinel really is in the bytes, not merely refused by the reader.
    assert!(
        bytes.as_chunks::<2>().0.iter().any(|s| i16::from_le_bytes([s[0], s[1]]) == NODATA),
        "a voided sample must be written as the NODATA sentinel"
    );
}

/// A declared `GDAL_NODATA` is honoured as well as `NaN` — GLO-30 does not set the tag, but a
/// re-cut or reprojected source will, and a `-32767` read as a height would be a 32 km hole in the
/// terrain rather than a void.
#[test]
fn a_declared_nodata_value_is_a_void_too() {
    let scratch = Scratch::new("gdal-nodata");
    let mut dem = plane_source(PIXEL_IS_POINT);
    dem.nodata = Some(-32767.0);
    dem.set_north_up(60, 60, -32767.0);
    let path = dem.write(scratch.path(), "declared");
    let mut mosaic = DemMosaic::default();
    mosaic.push(DemTile::open(&path).unwrap());

    let hole_lat = ((dem.tie_lat_deg - dem.step_deg * 60.0) * 1e6) as i32;
    let hole_lon = ((dem.tie_lon_deg + dem.step_deg * 60.0) * 1e6) as i32;
    assert_eq!(mosaic.height(f64::from(hole_lat) / 1e6, f64::from(hole_lon) / 1e6), None);
    // …and a post well away from it is still the plane.
    let clear = mosaic.height(f64::from(hole_lat) / 1e6 + 0.01, f64::from(hole_lon) / 1e6 + 0.01).unwrap();
    assert!((clear - plane_metres(f64::from(hole_lat) + 10_000.0, f64::from(hole_lon) + 10_000.0)).abs() < 0.01);
}

/// A box that overhangs the source costs four directory bytes per uncovered cell, not a cell block —
/// and the reader answers `None` there rather than a clamped neighbour, because §5.1 step 3 refuses
/// to answer a query from a cell that is not in the file.
#[test]
fn cells_with_no_data_at_all_are_absent_rather_than_written() {
    let scratch = Scratch::new("overhang");
    let path = plane_source(PIXEL_IS_POINT).write(scratch.path(), "plane");
    let mut mosaic = DemMosaic::default();
    mosaic.push(DemTile::open(&path).unwrap());

    // Reach three cells west and south of the source's coverage.
    let (base_lat, base_lon) = base_sample_udeg();
    let bbox = BboxUdeg {
        min_lat: (i64::from(base_lat) - 3 * CELL_UDEG) as i32,
        min_lon: (i64::from(base_lon) - 3 * CELL_UDEG) as i32,
        max_lat: base_lat,
        max_lon: base_lon,
    };
    let mut out = std::io::Cursor::new(Vec::new());
    let report = bake_shard(&mosaic, BakeParams { bbox, ..fixture_params() }, None, &mut out, |_| {}).unwrap();
    let bytes = out.into_inner();

    assert_eq!(report.cells_total, 16, "a 4 × 4 rectangle");
    assert!(report.cells_written < report.cells_total, "the south-west cells are outside the source");
    assert!(report.cells_written > 0, "the north-east corner overlaps it");
    assert_eq!(bytes.len(), 32 + 16 * 4 + report.cells_written as usize * 512);

    // Deep inside the uncovered corner, the answer is silence — never a clamped edge value.
    let outside_lat = (i64::from(base_lat) - 3 * CELL_UDEG + 100) as i32;
    let outside_lon = (i64::from(base_lon) - 3 * CELL_UDEG + 100) as i32;
    assert_eq!(sample(&bytes, outside_lat, outside_lon), None);
}

/// `--out <dir>` publishes one 1 × 1 container per cell, named by its catalog id, and each is the
/// same bytes the shard carries. A consumer never branches on which artifact it holds (§4.1).
#[test]
fn per_cell_files_are_one_by_one_containers_of_the_same_bytes() {
    let scratch = Scratch::new("cells");
    let source = Scratch::new("cells-source");
    let path = plane_source(PIXEL_IS_POINT).write(source.path(), "plane");
    let mut mosaic = DemMosaic::default();
    mosaic.push(DemTile::open(&path).unwrap());

    let report = bake_cells(&mosaic, fixture_params(), None, scratch.path(), |_| {}).unwrap();
    assert_eq!((report.cells_total, report.cells_written), (9, 9));

    let rect = cell_rect(fixture_bbox(), POSTING_LOG2, CELL_LOG2).unwrap();
    let shard = bake_plane_shard(PIXEL_IS_POINT);
    for (slot, (ci, cj)) in rect.cells().enumerate() {
        let file = scratch.join(&cell_file_name(CELL_LOG2, ci, cj));
        let bytes = std::fs::read(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        assert_eq!(bytes.len(), 32 + 4 + 512, "a published cell is a 1 × 1 container");

        // Its raster equals the shard's, and the reader gets the same answer from either.
        let offset = u32::from_le_bytes(shard[32 + slot * 4..36 + slot * 4].try_into().unwrap()) as usize;
        assert_eq!(&bytes[36..], &shard[offset..offset + 512]);

        let lat = lattice_coord(cell_base_sample(ci, POSTING_LOG2, CELL_LOG2) + 5, POSTING_LOG2);
        let lon = lattice_coord(cell_base_sample(cj, POSTING_LOG2, CELL_LOG2) + 5, POSTING_LOG2);
        assert_eq!(sample(&bytes, lat, lon), Some(oracle(lat, lon)));
    }
}

/// The coverage-edge clamp, from the producer's side: a single published cell answers a query one
/// microdegree past its last sample with that sample, rather than with an extrapolation or a `None`
/// (§5.3 step 3). The baker has to have written the edge sample for that to be true.
#[test]
fn a_lone_cell_clamps_at_its_own_coverage_edge() {
    let scratch = Scratch::new("clamp");
    let source = Scratch::new("clamp-source");
    let path = plane_source(PIXEL_IS_POINT).write(source.path(), "plane");
    let mut mosaic = DemMosaic::default();
    mosaic.push(DemTile::open(&path).unwrap());
    bake_cells(&mosaic, fixture_params(), None, scratch.path(), |_| {}).unwrap();

    let rect = cell_rect(fixture_bbox(), POSTING_LOG2, CELL_LOG2).unwrap();
    let (ci, cj) = (rect.min_i, rect.min_j);
    let bytes = std::fs::read(scratch.join(&cell_file_name(CELL_LOG2, ci, cj))).unwrap();

    let last_i = cell_base_sample(ci, POSTING_LOG2, CELL_LOG2) + SPAN as u32 - 1;
    let last_j = cell_base_sample(cj, POSTING_LOG2, CELL_LOG2) + SPAN as u32 - 1;
    let (edge_lat, edge_lon) = (lattice_coord(last_i, POSTING_LOG2), lattice_coord(last_j, POSTING_LOG2));
    let edge = oracle(edge_lat, edge_lon);
    assert_eq!(sample(&bytes, edge_lat, edge_lon), Some(edge));
    // Half a posting past the last sample on both axes: clamped to it, not extrapolated to +2 m.
    assert_eq!(sample(&bytes, edge_lat + 256, edge_lon + 256), Some(edge));
    assert_ne!(oracle(edge_lat + 256, edge_lon + 256), edge, "…and an extrapolation would have differed");
}
