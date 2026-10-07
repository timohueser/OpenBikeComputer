//! Full-map composition over independently encoded drawing and network sections.
use obc_draw::serialize::{serialize_tree, LodLayer, Node};
use obc_elevation::ElevationSource;
use obc_formats::obcm::{LOD_ENTRY_LEN, POI_HOURS_REF_NONE};
use obc_map_core::serialize::{
    align_up, check_scale_covers, header_bytes, lay_out, pack_style_dict, prefix_offsets, push_lod_entry, scaled,
    LodBytes, MapWriter, NavProfile, Style, STYLE_OFFSET,
};
use obc_network::nav::NavGraph;
use obc_network::serialize::{has_hours, serialize_nav_section, serialize_poi_pool, serialize_poi_section};
use obc_places::metadata::Poi;
use std::io::{self, Seek, Write};

/// Not an entry point — the in-memory parity oracle for [`serialize_lods_streaming`].
///
/// It lays out the same complete `.obcm` byte stream the obvious way: build every LOD's bytes, then
/// concatenate. Every production caller writes through the streaming twin instead, which holds one
/// tree at a time; this one exists so `streaming_matches_in_memory` can assert the two are
/// byte-identical, and because a corpus-building test outside this crate wants a map in a `Vec<u8>`.
///
/// The second return value is the total chunk-overflow feature drops (see [`obc_draw::serialize::pack_chunk`]).
///
// Eight positional arguments, one past clippy's default. A struct would move the same eight names
// one indirection away and force every caller to name a type to say "no styles, no POIs, an empty
// graph"; the streaming twin carries the same list, and the two must stay in lockstep.
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn serialize_lods(
    lods: &[LodLayer],
    styles: &[Style],
    marker_color: u16,
    global_bbox: (i64, i64, i64, i64),
    pois: &[Poi],
    nav: &NavGraph,
    profiles: &[NavProfile],
    terrain: &mut dyn ElevationSource,
) -> (Vec<u8>, usize) {
    let style_data = pack_style_dict(styles);
    let lod_count = lods.len();
    let (lod_table_offset, payload_start) = prefix_offsets(style_data.len(), lod_count);

    struct Block {
        ib: Vec<u8>,
        nc: u32,
        cb: Vec<u8>,
        cc: u32,
        cs: usize,
        mpp: Option<f64>,
    }
    let mut blocks = Vec::with_capacity(lod_count);
    let mut dropped = 0usize;
    for lod in lods {
        let (ib, nc, cb, cc, lod_dropped) = serialize_tree(&lod.root, lod.chunk_size);
        dropped += lod_dropped;
        blocks.push(Block { ib, nc, cb, cc, cs: lod.chunk_size, mpp: lod.max_mpp });
    }

    // Each LOD's index is named by a scaled `Index Offset`, so it starts on a unit boundary. The
    // LOD table's own end is rounded up once and every region behind it ends aligned by
    // construction, so no further alignment is needed here.
    let mut cursor = payload_start;
    let mut table = Vec::with_capacity(lod_count * LOD_ENTRY_LEN);
    for b in &blocks {
        push_lod_entry(&mut table, b.mpp, scaled(cursor), b.nc, b.cs, b.cc);
        cursor += b.ib.len() + b.cb.len();
    }

    // The POI section starts right after the last LOD's chunks; the nav section follows it.
    let poi_section_offset = cursor;
    let poi_section = serialize_poi_section(pois, global_bbox, poi_section_offset)
        .expect("POI section must fit before emitting the in-memory map");
    let nav_section_offset = poi_section_offset + poi_section.len();
    let nav_section = serialize_nav_section(nav, profiles, global_bbox, nav_section_offset, terrain);
    let dark_style_offset = align_up(nav_section_offset + nav_section.len());

    let (out, ()) = lay_out(0, |w| {
        w.put(&header_bytes(
            lod_count,
            marker_color,
            global_bbox,
            lod_table_offset,
            poi_section_offset,
            nav_section_offset,
            dark_style_offset,
        ))?;
        let at = w.begin_section()?; // → the style table
        debug_assert_eq!(at, STYLE_OFFSET as u64);
        w.put(&style_data)?;
        let at = w.begin_section()?; // → the LOD table
        debug_assert_eq!(at, lod_table_offset as u64, "the header names the table the cursor reached");
        w.put(&table)?;
        let at = w.begin_section()?; // → the first LOD's index
        debug_assert_eq!(at, payload_start as u64, "the first LOD entry names the index the cursor reached");
        for b in &blocks {
            w.put(&b.ib)?;
            w.put(&b.cb)?;
        }
        w.put(&poi_section)?;
        w.put(&nav_section)?;
        let at = w.begin_section()?;
        debug_assert_eq!(at, dark_style_offset as u64);
        w.put(&style_data)
    });
    check_scale_covers(out.len() as u64);
    (out, dropped)
}

/// The production writer, and the streaming counterpart to [`serialize_lods`]: the same byte stream,
/// but it builds, serializes and drops one LOD tree at a time, so peak memory is about one tree plus
/// one LOD's chunk bytes.
///
/// The POI and nav section offsets are not known until every LOD is sized, so the header and a
/// zeroed LOD table go out with placeholders, and step 5 seeks back and patches them. Returns
/// `(bytes_written, dropped_features)`, the latter counting chunk-overflow drops so the CLI can
/// warn.
///
/// `build(i)` yields LOD `i`'s `(root, chunk_size, max_mpp)`, called once per level in order; each
/// tree is dropped before the next call. A `None` root writes an empty region: no index, no chunk,
/// and the single-`0` offset table a chunkless LOD needs. A cell artifact depends on that, because
/// it writes the complete ladder with its out-of-band levels empty so that band membership never
/// appears in the bytes, and `Index Node Count == 0` is the predicate a reader caches at mount to
/// skip a level with no I/O at all.
#[allow(clippy::too_many_arguments)]
pub fn serialize_lods_streaming<W, F>(
    w: &mut W,
    lod_count: usize,
    styles: &[Style],
    marker_color: u16,
    global_bbox: (i64, i64, i64, i64),
    pois: &[Poi],
    landmarks: &[obc_pack::landmark_map::Landmark],
    peaks: &obc_pack::peak_map::Peaks,
    nav: &NavGraph,
    profiles: &[NavProfile],
    terrain: &mut dyn ElevationSource,
    mut build: F,
) -> io::Result<(u64, usize)>
where
    W: Write + Seek,
    F: FnMut(usize) -> (Option<Node>, usize, Option<f64>),
{
    let mut dropped = 0usize;
    let schedules: Vec<_> = pois
        .iter()
        .map(|p| has_hours(p.subtype).then_some(p.hours.as_ref()).flatten())
        .chain(landmarks.iter().map(|p| p.hours.as_ref()))
        .collect();
    let (pool, refs) = obc_places::hours::build_hours_pool(&schedules, |schedule| *schedule);
    if pool.len() >= POI_HOURS_REF_NONE as usize {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "hours pool exceeds its record limit"));
    }
    let landmark_refs: Vec<_> = refs[pois.len()..].iter().map(|index| index.unwrap_or(POI_HOURS_REF_NONE)).collect();
    let landmark_bytes = obc_pack::landmark_map::serialize(landmarks, &landmark_refs)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let peak_bytes = obc_pack::peak_map::serialize(peaks).map_err(io::Error::other)?;
    let mut writer = MapWriter::new(w, lod_count, styles, marker_color, global_bbox)?;
    for i in 0..lod_count {
        let (root, chunk_size, max_mpp) = build(i);
        let region = root.map(|root| {
            let out = serialize_tree(&root, chunk_size);
            drop(root);
            out
        });
        let bytes = region.as_ref().map(|(index, nodes, chunks, count, _)| LodBytes {
            index,
            nodes: *nodes,
            chunks,
            chunk_count: *count,
        });
        dropped += region.as_ref().map_or(0, |region| region.4);
        writer.lod(chunk_size, max_mpp, bytes)?;
    }
    let poi_offset = writer.position();
    let poi = serialize_poi_pool(pois, global_bbox, poi_offset, &pool, &refs[..pois.len()])?;
    let nav = serialize_nav_section(nav, profiles, global_bbox, poi_offset + poi.len(), terrain);
    Ok((writer.finish(&poi, &nav, &landmark_bytes, &peak_bytes)?, dropped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_draw::serialize::{Feature, Kind};
    use obc_elevation::NullElevation;
    use obc_map_core::config::LineStyle;
    #[test]
    fn streaming_matches_in_memory() {
        // Streaming output must be byte-identical to `serialize_lods` for the same 2-LOD pyramid
        // and POI + nav sections, so the fixture carries POIs of a few categories (two sharing one
        // pooled schedule) and a small nav graph.
        use obc_network::nav::{Edge, Node as NavNode};
        use obc_places::metadata::Poi;
        use std::io::Cursor;

        let bbox = (0, 0, 1_000_000, 1_000_000);
        let styles = vec![Style {
            id: 1,
            z_index: 0,
            color: 0x1234,
            weight: 2,
            priority: 1,
            line_style: LineStyle::Solid,
            color2: None,
            fixed_width: false,
            terrain_layer: false,
        }];
        let lods = vec![
            LodLayer {
                max_mpp: Some(100.0),
                chunk_size: 256,
                root: Node::Leaf {
                    bbox,
                    features: vec![Feature {
                        style_id: 1,
                        kind: Kind::Line,
                        rings: vec![vec![(0.1, 0.1), (0.9, 0.9)]],
                    }],
                },
            },
            LodLayer {
                max_mpp: None,
                chunk_size: 256,
                root: Node::Leaf {
                    bbox,
                    features: vec![Feature {
                        style_id: 1,
                        kind: Kind::Line,
                        rings: vec![vec![(0.2, 0.2), (0.8, 0.8), (0.5, 0.1)]],
                    }],
                },
            },
        ];
        let poi = |subtype, lon, lat, name: Option<&str>, hours: Option<&str>| Poi {
            metadata: obc_formats::obcm::PoiMetadata {
                source: obc_formats::obcm::SourceId::osm(1, subtype as u64),
                approach: None,
            },
            access_nodes: Vec::new(),
            wikidata: None,
            wikipedia: None,
            subtype,
            lon_udeg: lon,
            lat_udeg: lat,
            name: name.map(String::from),
            from_node: true,
            hours: hours.and_then(obc_places::hours::parse),
            elevation_m: None,
            population: None,
        };
        let pois = vec![
            poi(1, 100_000, 100_000, Some("Brunnen"), None),
            poi(5, 200_000, 200_000, None, Some("Mo-Fr 08:00-18:00")),
            poi(17, 300_000, 300_000, Some("Apotheke"), Some("Mo-Fr 08:00-18:00")),
            poi(18, 400_000, 400_000, Some("Velowerkstatt"), Some("24/7")),
        ];
        let nav = NavGraph {
            nodes: vec![NavNode { id: 0, coord: (100_000, 100_000) }, NavNode { id: 1, coord: (200_000, 200_000) }],
            edges: vec![Edge {
                a: 0,
                b: 1,
                polyline: vec![(100_000, 100_000), (150_000, 160_000), (200_000, 200_000)],
                length_m: 15_700,
                kind: 0,
            }],
        };

        // Two profiles so the profile table is non-trivial and both paths must agree on its bytes.
        let profiles = vec![
            NavProfile { name: "Road".into(), highway: [16; 32], surface: [16; 8], climb_weight: 10 },
            NavProfile { name: "Gravel".into(), highway: [24; 32], surface: [32; 8], climb_weight: 8 },
        ];

        for nav in [&nav, &NavGraph::default()] {
            let (reference, ref_dropped) =
                serialize_lods(&lods, &styles, 0xABCD, bbox, &pois, nav, &profiles, &mut NullElevation);
            assert_eq!(ref_dropped, 0, "nothing overflows in this fixture");

            let mut cur = Cursor::new(Vec::new());
            let (total, dropped) = serialize_lods_streaming(
                &mut cur,
                lods.len(),
                &styles,
                0xABCD,
                bbox,
                &pois,
                &[],
                &obc_pack::peak_map::Peaks::default(),
                nav,
                &profiles,
                &mut NullElevation,
                |i| (Some(lods[i].root.clone()), lods[i].chunk_size, lods[i].max_mpp),
            )
            .unwrap();

            assert_eq!(cur.into_inner(), reference, "streaming output must be byte-identical");
            assert_eq!(total as usize, reference.len());
            assert_eq!(dropped, 0, "and reports the same (zero) drop count");
        }
    }
}
