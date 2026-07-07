//! `ingest.rs` — read an `.osm.pbf` into styled features (lines, closed-way
//! polygons, and multipolygon/`boundary` relation areas). Two `osmpbf` passes:
//!
//!   - **Pass 1** builds the `node_id → coord` store and collects qualifying area
//!     relations. Relations sit last in a sorted PBF, so one whole-file read sees
//!     them after the nodes — no extra pass. Tagged nodes are also matched
//!     against the POI table here ([`crate::poi`]).
//!   - **Pass 2** resolves ways into features + coastlines and captures the
//!     geometry of any way that is a relation member. Closed ways matching the
//!     POI table yield centroid POIs.
//!
//! Each relation's member ways are then assembled into polygons-with-holes via
//! [`assemble_multipolygon`]. Assembly is additive: a tagged closed way that is
//! also a relation member yields its own polygon *and* contributes to the relation.
//! A closed `highway=residential` loop is a line only, never a filled blob.
//!
//! Coordinates use `decimicro / 1e7`, never `* 1e-7`, so the f64 lon/lat match
//! osmium's exactly and everything downstream lines up.

use std::collections::{HashMap, HashSet};

use osmpbf::{Element, ElementReader, RelMemberType};

use crate::config::Config;
use crate::geom::{assemble_multipolygon, polygon_is_valid, Geom};
use crate::hours;
use crate::nav::{self, NavGraph, RoutableWay};
use crate::poi::{self, Poi};

pub struct IngestFeature {
    pub style_id: u8,
    pub min_lod: usize,
    pub geom: Geom,
}

/// Coastlines are captured separately (always) — they feed the bbox and land/sea.
/// POIs are the classified + deduped point-of-interest set ([`crate::poi`]),
/// serialized into the OBCM POI section (§7). `nav_graph` is the in-memory
/// routable graph ([`crate::nav`]), serialized into the v8 nav-graph section (§8).
pub struct Ingested {
    pub features: Vec<IngestFeature>,
    pub coastlines: Vec<Vec<(f64, f64)>>,
    pub pois: Vec<Poi>,
    pub nav_graph: NavGraph,
}

/// A pass-1 area relation awaiting member geometry (pass 2) and assembly.
struct PendingRelation {
    style_id: u8,
    min_lod: usize,
    /// Member **way** ids in member order. Roles are dropped — `build_area`
    /// classifies outer/inner by geometry.
    member_ways: Vec<i64>,
}

/// The tags whose presence (with `area != no`) classifies a *closed* way as a
/// polygon.
const AREA_TAGS: [&str; 6] = ["building", "landuse", "amenity", "leisure", "natural", "waterway"];

/// `decimicro / 1e7`, never `* 1e-7`, so coords match osmium exactly.
#[inline]
fn to_deg(decimicro: i32) -> f64 {
    decimicro as f64 / 1e7
}

/// Two-pass ingest of a single `.osm.pbf` (lines + closed-way polygons +
/// relation-assembled area polygons).
pub fn ingest_osm(pbf_path: &str, config: &Config) -> Result<Ingested, String> {
    // --- Pass 1: node-location store + relation collection. ---
    // The PBF is node-sorted, so the store is filled before any relation is read.
    // The stage strings are matched by the web builder's progress UI — print each
    // when its pass actually starts, not both up front.
    println!("Pass 1: reading nodes...");
    let mut nodes: HashMap<i64, (i32, i32)> = HashMap::new();
    let mut pending: Vec<PendingRelation> = Vec::new();
    let mut needed_ways: HashSet<i64> = HashSet::new();
    // POI candidates from both passes, deduped after assembly. Classification is
    // config-free (hardcoded table — locked decision on #115).
    let mut poi_cands: Vec<Poi> = Vec::new();
    ElementReader::from_path(pbf_path)
        .map_err(|e| format!("open {pbf_path}: {e}"))?
        .for_each(|el| match el {
            Element::Node(n) => {
                nodes.insert(n.id(), (n.decimicro_lon(), n.decimicro_lat()));
                push_node_poi(n.tags(), n.decimicro_lon(), n.decimicro_lat(), &mut poi_cands);
            }
            Element::DenseNode(n) => {
                nodes.insert(n.id(), (n.decimicro_lon(), n.decimicro_lat()));
                push_node_poi(n.tags(), n.decimicro_lon(), n.decimicro_lat(), &mut poi_cands);
            }
            Element::Relation(r) => collect_relation(&r, config, &mut pending, &mut needed_ways),
            _ => {}
        })
        .map_err(|e| format!("pass 1 {pbf_path}: {e}"))?;

    // --- Pass 2: ways → features + coastlines, plus member-way geometry capture. ---
    println!("Pass 2: processing ways...");
    let mut features = Vec::new();
    let mut coastlines = Vec::new();
    let mut member_geom: HashMap<i64, Vec<(f64, f64)>> = HashMap::with_capacity(needed_ways.len());
    // Routable-way topology for the nav graph ([`crate::nav`]). We keep the OSM node
    // ids here (which the render path drops) so shared nodes can be recovered as
    // junctions after the pass; the graph is built from these once all ways are seen.
    let mut routable_ways: Vec<RoutableWay> = Vec::new();
    ElementReader::from_path(pbf_path)
        .map_err(|e| format!("open {pbf_path}: {e}"))?
        .for_each(|el| {
            if let Element::Way(w) = el {
                let refs: Vec<i64> = w.refs().collect();
                // A missing node aborts the whole way — osmium would raise
                // `InvalidLocationError` here, and the way is dropped.
                let Some(coords) = resolve_coords(&refs, &nodes) else { return };
                push_routable_way(&w, &refs, &coords, &mut routable_ways);
                process_way(&w, &refs, &coords, config, &mut features, &mut coastlines, &mut poi_cands);
                if needed_ways.contains(&w.id()) {
                    member_geom.insert(w.id(), coords);
                }
            }
        })
        .map_err(|e| format!("pass 2 {pbf_path}: {e}"))?;

    // --- Assemble relation areas from captured member geometry. ---
    // Each outer ring (+ nested holes) becomes one polygon, styled by the relation.
    // **Completeness:** like osmium, only assemble when ALL member ways are present;
    // an incomplete relation (a member clipped out of the extract) is dropped, not
    // assembled from survivors — that would emit a phantom boundary-crossing polygon.
    for pr in &pending {
        let mut members = Vec::with_capacity(pr.member_ways.len());
        let mut complete = true;
        for wid in &pr.member_ways {
            match member_geom.get(wid) {
                Some(g) => members.push(g.clone()),
                None => {
                    complete = false;
                    break;
                }
            }
        }
        if !complete {
            continue;
        }
        for poly in assemble_multipolygon(&members) {
            features.push(IngestFeature { style_id: pr.style_id, min_lod: pr.min_lod, geom: poly });
        }
    }

    // --- POIs: collapse OSM double-mapping, then log per-category counts. ---
    let (pois, poi_dropped) = poi::dedupe(poi_cands);
    println!("{}", poi::format_counts(&pois, poi_dropped));

    // --- Nav graph: junctions + deduped edges from the routable ways, then
    // island pruning + v9-guarantee edge splits ([`nav::build_graph`]). Serialized
    // into the §8 nav section. Logged (with component + kinds stats) alongside POIs.
    let (nav_graph, nav_stats) = nav::build_graph(&routable_ways);
    println!("{}", nav::format_summary(&nav_graph, &nav_stats));

    Ok(Ingested { features, coastlines, pois, nav_graph })
}

/// Capture a routable way's node-id sequence + µdeg coords for the nav graph.
/// Routability is tag-based ([`nav::is_routable`]) and independent of styling — a
/// way can be routable without a render style and vice-versa. Ways with fewer than
/// two nodes carry no edge and are skipped. `coords` is the way's f64-degree
/// geometry from [`resolve_coords`]; it is snapped to the µdeg grid here (the same
/// grid POIs and the serializer use) so edge lengths and later serialization agree.
fn push_routable_way(w: &osmpbf::Way, refs: &[i64], coords: &[(f64, f64)], out: &mut Vec<RoutableWay>) {
    if refs.len() < 2 {
        return;
    }
    // Classify once (routability + way-kind byte). `None` ⇒ not routable — this is
    // the only place tags exist, so the kind is captured here or never.
    let Some(kind) = nav::classify(w.tags()) else { return };
    let coords_udeg = coords.iter().map(|&(x, y)| (poi::to_udeg(x), poi::to_udeg(y))).collect();
    out.push(RoutableWay { node_ids: refs.to_vec(), coords: coords_udeg, kind });
}

/// Classify one node's tags against the POI table; push a candidate on match.
/// The overwhelmingly common untagged-node case falls straight through.
fn push_node_poi<'a, I>(tags: I, decimicro_lon: i32, decimicro_lat: i32, out: &mut Vec<Poi>)
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    if let Some((subtype, name, raw_hours)) = poi::classify(tags) {
        out.push(Poi {
            subtype,
            lon_udeg: poi::to_udeg(to_deg(decimicro_lon)),
            lat_udeg: poi::to_udeg(to_deg(decimicro_lat)),
            name,
            from_node: true,
            hours: raw_hours.and_then(hours::parse),
        });
    }
}

/// Resolve a way's node refs to degree coordinates. `None` iff any node is missing
/// — the caller drops the way (osmium's `InvalidLocationError`).
fn resolve_coords(refs: &[i64], nodes: &HashMap<i64, (i32, i32)>) -> Option<Vec<(f64, f64)>> {
    let mut coords = Vec::with_capacity(refs.len());
    for r in refs {
        let &(dx, dy) = nodes.get(r)?;
        coords.push((to_deg(dx), to_deg(dy)));
    }
    Some(coords)
}

/// Collect a `type=multipolygon`/`type=boundary` relation (skipping `admin_level`)
/// for area assembly: record its style + member way-ids. Roles are ignored;
/// non-way members are skipped.
fn collect_relation(
    r: &osmpbf::Relation,
    config: &Config,
    pending: &mut Vec<PendingRelation>,
    needed_ways: &mut HashSet<i64>,
) {
    let tags: HashMap<&str, &str> = r.tags().collect();
    match tags.get("type").copied() {
        Some("multipolygon") | Some("boundary") => {}
        _ => return,
    }
    // admin_level relations are line-only → no polygon.
    if tags.contains_key("admin_level") {
        return;
    }
    let Some(style) = config.get_style(&tags) else { return };
    let member_ways: Vec<i64> =
        r.members().filter(|m| m.member_type == RelMemberType::Way).map(|m| m.member_id).collect();
    if member_ways.is_empty() {
        return;
    }
    for &wid in &member_ways {
        needed_ways.insert(wid);
    }
    pending.push(PendingRelation { style_id: style.id, min_lod: style.min_lod, member_ways });
}

/// One way: capture coastline always, then style + classify into a single
/// polygon-or-line emission. `refs`/`coords` are pre-resolved.
fn process_way(
    w: &osmpbf::Way,
    refs: &[i64],
    coords: &[(f64, f64)],
    config: &Config,
    features: &mut Vec<IngestFeature>,
    coastlines: &mut Vec<Vec<(f64, f64)>>,
    pois: &mut Vec<Poi>,
) {
    let tags: HashMap<&str, &str> = w.tags().collect();
    let is_closed = refs.len() >= 2 && refs.first() == refs.last();

    // Coastlines are captured ALWAYS — even if the way is also closed/styled — and
    // as lines, never areas.
    if tags.get("natural") == Some(&"coastline") && coords.len() >= 2 {
        coastlines.push(coords.to_vec());
    }

    // A closed way matching the POI table yields a POI at the ring centroid —
    // independent of styling (a bare `shop=supermarket` outline has no style at
    // all). The building-tagged supermarket way and the area campsite are the
    // motivating cases; relations are out of scope (#115).
    if is_closed {
        if let Some((subtype, name, raw_hours)) = poi::classify(tags.iter().map(|(&k, &v)| (k, v))) {
            let (cx, cy) = poi::ring_centroid(coords);
            pois.push(Poi {
                subtype,
                lon_udeg: poi::to_udeg(cx),
                lat_udeg: poi::to_udeg(cy),
                name,
                from_node: false,
                hours: raw_hours.and_then(hours::parse),
            });
        }
    }

    let Some(style) = config.get_style(&tags) else { return };

    // A closed area emits a polygon; a closed road loop emits a line, never both.
    if is_closed && is_area(&tags) {
        // admin_level + area ⇒ drop entirely (no line, no polygon).
        if tags.contains_key("admin_level") {
            return;
        }
        // Skip rings osmium's assembler would reject as invalid (e.g. a
        // self-intersecting building); no polygon and no line (line branch returned).
        if coords.len() >= 3 && polygon_is_valid(coords, &[]) {
            features.push(IngestFeature {
                style_id: style.id,
                min_lod: style.min_lod,
                geom: Geom::Polygon { exterior: coords.to_vec(), interiors: Vec::new() },
            });
        }
        return;
    }

    // Line: open ways, and closed-but-not-area circular roads.
    if coords.len() >= 2 {
        features.push(IngestFeature { style_id: style.id, min_lod: style.min_lod, geom: Geom::Line(coords.to_vec()) });
    }
}

/// Closed-way area heuristic: `area=yes` ⇒ area; `area=no` ⇒ never; otherwise
/// area iff it carries any [`AREA_TAGS`] key.
fn is_area(tags: &HashMap<&str, &str>) -> bool {
    match tags.get("area") {
        Some(&"yes") => true,
        Some(&"no") => false,
        _ => AREA_TAGS.iter().any(|k| tags.contains_key(k)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TINY_PBF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packer/tests/corpus/data/tiny.osm.pbf");

    fn is_polygon(g: &Geom) -> bool {
        matches!(g, Geom::Polygon { .. })
    }

    /// The `tiny.osm` truth table: relations assembled (R1's lake with a hole, R2's
    /// two forest outers) plus lines and closed-way polygons → 10 features.
    #[test]
    fn tiny_truth_table() {
        // The fixture is committed in-repo (source of truth `tiny/tiny.osm`); a
        // missing fixture is a hard failure, not a skip.
        assert!(
            std::path::Path::new(TINY_PBF).exists(),
            "corpus fixture missing: {TINY_PBF}. It is committed; rebuild from tiny/tiny.osm via \
             packer/tests/corpus/build_corpus.sh"
        );
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packer/presets/default.json")).expect("config");
        let ing = ingest_osm(TINY_PBF, &cfg).expect("ingest");

        // W8 (way 109) is the only coastline; nodes 29,30 ⇒ 2 points.
        assert_eq!(ing.coastlines.len(), 1, "exactly one coastline");
        assert_eq!(ing.coastlines[0].len(), 2);

        // Multiset of (style_id, is_polygon).
        let mut counts: HashMap<(u8, bool), usize> = HashMap::new();
        for f in &ing.features {
            *counts.entry((f.style_id, is_polygon(&f.geom))).or_insert(0) += 1;
        }
        let n = |id: u8, poly: bool| counts.get(&(id, poly)).copied().unwrap_or(0);

        // Style ids: forest=39, pedestrian=15, residential=12, primary=5,
        // trunk=3, admin_level/2=42, water=32 (see config doc order).
        assert_eq!(n(39, true), 3, "W5 closed forest + R2's two outer rings ⇒ 3 polygons");
        assert_eq!(n(32, true), 1, "R1 natural=water ⇒ 1 polygon (lake)");
        assert_eq!(n(15, true), 1, "W11 highway=pedestrian area=yes ⇒ 1 polygon");
        assert_eq!(n(12, false), 1, "W6 closed highway=residential ⇒ 1 line");
        assert_eq!(n(5, false), 1, "W7 highway=primary ⇒ 1 line");
        assert_eq!(n(3, false), 1, "W7b highway=trunk ⇒ 1 line");
        assert_eq!(n(42, false), 1, "W9 admin_level=2 ⇒ 1 line");
        assert_eq!(n(32, false), 1, "W12 natural=water area=no ⇒ 1 line");

        // R1 is a lake WITH an island (one hole).
        let lake = ing.features.iter().find(|f| f.style_id == 32 && is_polygon(&f.geom)).expect("water polygon");
        match &lake.geom {
            Geom::Polygon { interiors, .. } => assert_eq!(interiors.len(), 1, "R1 has one hole"),
            _ => unreachable!(),
        }

        // The fixes/omissions we MUST honor:
        assert_eq!(n(12, true), 0, "no residential blob (closed-line-way fix)");
        // 5 polygons (3 forest, 1 pedestrian, 1 water lake) + 5 lines.
        assert_eq!(ing.features.len(), 10, "10 features total");
    }

    /// End-to-end POI extraction over the hand-authored `poi.osm` fixture (its
    /// header comment is the truth table): node + closed-way classification,
    /// name folding, and both dedup pairs (node-beats-centroid, named-beats-
    /// unnamed). See packer/tests/corpus/poi/poi.osm.
    #[test]
    fn poi_fixture_end_to_end() {
        const POI_PBF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packer/tests/corpus/data/poi.osm.pbf");
        assert!(
            std::path::Path::new(POI_PBF).exists(),
            "corpus fixture missing: {POI_PBF}. It is committed; rebuild from poi/poi.osm via \
             packer/tests/corpus/build_corpus.sh"
        );
        let cfg =
            Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packer/presets/default.json")).expect("config");
        let ing = ingest_osm(POI_PBF, &cfg).expect("ingest");

        // 7 candidates (5 nodes + 2 way-centroids), 2 dedup-dropped ⇒ 5 kept.
        assert_eq!(ing.pois.len(), 5, "expected 5 POIs, got: {:?}", ing.pois);

        let find = |name: Option<&str>, subtype: u8| {
            ing.pois
                .iter()
                .find(|p| p.subtype == subtype && p.name.as_deref() == name)
                .unwrap_or_else(|| panic!("missing poi subtype {subtype} name {name:?}: {:?}", ing.pois))
        };

        // N1: named water node, exact µdeg grid.
        let n1 = find(Some("Marktbrunnen"), 1);
        assert_eq!((n1.lat_udeg, n1.lon_udeg, n1.from_node), (47_995_000, 7_850_000, true));
        // N2 beat W1's centroid: node position (the building corner), way's
        // name folded ü→ue at pack time.
        let n2 = find(Some("Edeka Mueller"), 13);
        assert_eq!((n2.lat_udeg, n2.lon_udeg, n2.from_node), (47_989_900, 7_859_900, true));
        // N3: CJK name folded to empty ⇒ unnamed.
        let n3 = find(None, 1);
        assert_eq!((n3.lat_udeg, n3.lon_udeg), (47_980_000, 7_840_000));
        // N5 beat the unnamed spring N6 40 m away (named > unnamed, same category).
        find(Some("Brunnen A"), 1);
        assert!(!ing.pois.iter().any(|p| p.subtype == 2), "spring N6 must be dedup-dropped");
        // W2: unnamed campsite way ⇒ POI at the ring centroid.
        let w2 = find(None, 5);
        assert_eq!((w2.lat_udeg, w2.lon_udeg, w2.from_node), (48_000_200, 7_870_200, false));
        // N4 (amenity=parking) never classified.
        assert_eq!(crate::poi::format_counts(&ing.pois, 0).matches("water 3").count(), 1);
    }

    fn tags(pairs: &[(&'static str, &'static str)]) -> HashMap<&'static str, &'static str> {
        pairs.iter().copied().collect()
    }

    /// The closed-way polygon/line gate: `area=yes` forces area even with no
    /// AREA_TAGS key; `area=no` forces a line even with one present (the W12
    /// `natural=water area=no` case); absent `area` falls back to any AREA_TAGS key.
    #[test]
    fn is_area_overrides_and_tag_fallback() {
        assert!(is_area(&tags(&[("area", "yes")])), "area=yes ⇒ area regardless of other tags");
        assert!(!is_area(&tags(&[("area", "no"), ("natural", "water")])), "area=no ⇒ never an area");
        for key in AREA_TAGS {
            assert!(is_area(&tags(&[(key, "whatever")])), "AREA_TAGS key {key} ⇒ area");
        }
        assert!(!is_area(&tags(&[("highway", "residential")])), "no area tag, no AREA_TAGS key ⇒ line");
        // An unrecognized `area` value falls through to the tag fallback (not yes/no).
        assert!(!is_area(&tags(&[("area", "maybe")])), "unknown area value, no AREA_TAGS key ⇒ line");
        assert!(is_area(&tags(&[("area", "maybe"), ("building", "yes")])), "unknown area value falls back to tags");
    }
}
