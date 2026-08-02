//! **The restatement pins.** Two places in this crate deliberately write bytes `obc-pack` already
//! knows how to write, because the engine may not depend on the packer at runtime (libGEOS follows
//! it, and the engine compiles for `wasm32-unknown-unknown`):
//!
//! - `shard.rs` restates the OBCM header, style table and LOD table (`obc-pack`'s `header_bytes`,
//!   `pack_style_dict`, `push_lod_entry`);
//! - `qtree.rs` restates the quadtree's recursion floor and its bin-packing policy.
//!
//! A restatement is only acceptable if a divergence *fails a test* rather than mis-writing a map, so
//! these compare the two implementations **byte for byte over identical inputs** — the same
//! discipline `tests/oracle.rs::the_engine_and_the_packer_agree_on_the_grid` applies to the grid
//! arithmetic. `obc-pack` is a dev-dependency, so none of this enters the engine's build graph.
//!
//! The inputs are read back out of a file the *packer* produced, so neither side gets to state what
//! the answer is: the packer writes a map, this test slices the three tables out of it, and the
//! engine's writers are asked to produce the same bytes.

use obc_elevation::NullElevation;
use obc_formats::obcm::{HEADER_LEN, LOD_ENTRY_LEN, NAV_CHUNK_SIZE, POI_CHUNK_SIZE, STYLE_RECORD_LEN};
use obc_pack::geom::Geom;
use obc_pack::nav::NavGraph;
use obc_pack::progress::Progress;
use obc_pack::quadtree::build_lod_with;
use obc_pack::serialize::{pack_style_dict, NavProfile, Style};
use obc_pack::{serialize_lods, LodLayer};
use obcm_assemble::grid::AlignedBox;
use obcm_assemble::schema::StyleRecord;
use obcm_assemble::shard;

/// A grid-aligned power-of-two box, because the engine's header writer takes one (§2.1) — the
/// worked example's `2^19` square, so the values are the ones OBCA §7 already prints.
const BOX: AlignedBox = AlignedBox { min_lat: 47_185_920, min_lon: 7_340_032, span_log2: 19 };

/// One style, in the shape both sides construct from: `(id, z_index, color, weight, priority,
/// dashed, color2)`.
type Row = (u8, i8, u16, u8, u8, bool, Option<u16>);

/// The style set both sides are handed. Every flag bit is exercised: four priorities, a dashed
/// record, a `color2` record (including `Some(0x0000)`, which is a real colour and not a sentinel),
/// both extremes of the signed `z_index`, and one plain record.
fn styles() -> (Vec<Style>, Vec<StyleRecord>) {
    let rows: [Row; 5] = [
        (1, 0, 0x001F, 1, 1, false, None),
        (2, -3, 0xF800, 4, 2, true, None),
        (3, 7, 0x07E0, 2, 3, false, Some(0xBEEF)),
        (4, -128, 0xFFFF, 255, 4, true, Some(0x0000)),
        (9, 127, 0x8410, 3, 1, false, None),
    ];
    let pack = rows
        .iter()
        .map(|&(id, z_index, color, weight, priority, dashed, color2)| Style {
            id,
            z_index,
            color,
            weight,
            priority,
            dashed,
            color2,
        })
        .collect();
    let engine = rows
        .iter()
        .map(|&(id, z_index, color, weight, priority, dashed, color2)| StyleRecord {
            id,
            z_index,
            color,
            weight,
            priority,
            dashed,
            color2,
        })
        .collect();
    (pack, engine)
}

/// One map from the real packer, with a three-level ladder whose LODs carry different amounts of
/// geometry — so the LOD table has three distinct `(offset, node count, chunk count)` triples rather
/// than three copies of one.
fn packed() -> (Vec<u8>, Vec<Style>, Vec<StyleRecord>, u16) {
    let (pack_styles, engine_styles) = styles();
    let marker_color = 0xF81Fu16;
    let bbox = BOX.ubox();
    let deg = |v: i64| v as f64 / 1e6;
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    // Enough short lines, spread over the box, that the finest level's quadtree really subdivides.
    let mut features: Vec<(u8, Geom)> = Vec::new();
    for k in 0..64i64 {
        let lat = min_lat + (max_lat - min_lat) * k / 64 + 17;
        let lon = min_lon + (max_lon - min_lon) * (k * 7 % 64) / 64 + 23;
        features.push((
            pack_styles[k as usize % pack_styles.len()].id,
            Geom::Line(vec![(deg(lon), deg(lat)), (deg(lon + 4_000), deg(lat + 4_000))]),
        ));
    }
    let lods: Vec<LodLayer> = [(None, 64usize), (Some(20.0), 32), (Some(4.0), 8)]
        .into_iter()
        .map(|(max_mpp, take)| LodLayer {
            max_mpp,
            chunk_size: 1024,
            root: build_lod_with(features[..take].to_vec(), bbox, 1024, &Progress::silent()),
        })
        .collect();
    let profiles = [NavProfile { name: "Road".into(), highway: [16; 32], surface: [16; 8], climb_weight: 10 }];
    let (bytes, dropped) = serialize_lods(
        &lods,
        &pack_styles,
        marker_color,
        bbox,
        &[],
        &NavGraph::default(),
        &profiles,
        &mut NullElevation,
    );
    assert_eq!(dropped, 0, "the fixture must not lose features to the chunk cap");
    (bytes, pack_styles, engine_styles, marker_color)
}

/// The header (`OBCM_Spec.md` §1): 40 bytes of magic, version, bbox in lat/lon/lat/lon order, and
/// five offsets. The engine writes it from a [`AlignedBox`] and the packer from a raw bbox tuple, so
/// this is the one place their *interfaces* differ and their bytes may not.
#[test]
fn the_header_matches_the_packers_byte_for_byte() {
    let (bytes, _, engine_styles, marker_color) = packed();
    let want = &bytes[..HEADER_LEN];
    // The packer's own choices for the three offsets, read back out of what it wrote — so the
    // comparison is over identical inputs rather than over two guesses at a layout.
    let lod_table_offset = u32::from_le_bytes(want[26..30].try_into().unwrap());
    let poi_offset = u32::from_le_bytes(want[32..36].try_into().unwrap());
    let nav_offset = u32::from_le_bytes(want[36..40].try_into().unwrap());
    let got = shard::header_bytes(BOX, 3, marker_color, lod_table_offset, poi_offset, nav_offset);
    assert_eq!(got, want, "the restated OBCM header diverged from obc-pack's");
    // …and the field the offsets were read from is the one the engine writes there.
    assert_eq!(lod_table_offset as usize, HEADER_LEN + 1 + engine_styles.len() * STYLE_RECORD_LEN);
}

/// The style table (§2): the skin's half of a restyle, and the table the §4.1 agreement check keys
/// on. `pack_style_dict` sorts by id; the engine refuses an unsorted table instead (§4.7), so the
/// inputs here are already in order and the *bytes* must agree.
#[test]
fn the_style_table_matches_the_packers_byte_for_byte() {
    let (bytes, pack_styles, engine_styles, _) = packed();
    let len = 1 + pack_styles.len() * STYLE_RECORD_LEN;
    assert_eq!(shard::pack_style_table(&engine_styles), &bytes[HEADER_LEN..HEADER_LEN + len]);
    // The independent spelling, in case the packed file ever stops carrying the table verbatim.
    assert_eq!(shard::pack_style_table(&engine_styles), pack_style_dict(&pack_styles));
}

/// The LOD table (§3): one 18-byte `<fIIHI>` entry per ladder level. `Max Meters/Pixel` is the trap
/// — it is an `f32` with `+inf` at the top, and `null` and `0.0` are different maps.
#[test]
fn the_lod_table_matches_the_packers_byte_for_byte() {
    let (bytes, pack_styles, _, _) = packed();
    let lod_table_offset = HEADER_LEN + 1 + pack_styles.len() * STYLE_RECORD_LEN;
    let want = &bytes[lod_table_offset..lod_table_offset + 3 * LOD_ENTRY_LEN];
    // Re-read each entry's own fields, then ask the engine's writer to reproduce it.
    let mut got = Vec::new();
    for (i, max_mpp) in [None, Some(20.0), Some(4.0)].into_iter().enumerate() {
        let e = &want[i * LOD_ENTRY_LEN..][..LOD_ENTRY_LEN];
        shard::push_lod_entry(
            &mut got,
            max_mpp,
            u32::from_le_bytes(e[4..8].try_into().unwrap()),
            u32::from_le_bytes(e[8..12].try_into().unwrap()),
            u16::from_le_bytes(e[12..14].try_into().unwrap()) as usize,
            u32::from_le_bytes(e[14..18].try_into().unwrap()),
        );
    }
    assert_eq!(got, want, "the restated LOD table diverged from obc-pack's");
    // The `+inf` top level is written as `+inf`, not as a large finite number or a zero.
    assert!(f32::from_le_bytes(want[0..4].try_into().unwrap()).is_infinite());
}

/// The quadtree's recursion floor. `qtree.rs` restates the packer's literal `10` in
/// `build_poi_tree` / `build_nav_tree`; the reader resolves exactly one subdivision rule, so a
/// divergence would put records outside the leaf that indexes them — silently, and only for the
/// dense clusters where it matters most.
#[test]
fn the_split_floor_matches_the_packers() {
    assert_eq!(
        obcm_assemble::qtree::SPLIT_FLOOR,
        10,
        "obc-pack's build_poi_tree/build_nav_tree stop at `max_lon - min_lon < 10`"
    );
    // The two chunk sizes the floor is applied against are format constants on both sides.
    assert_eq!((POI_CHUNK_SIZE, NAV_CHUNK_SIZE), (512, 512));
}
