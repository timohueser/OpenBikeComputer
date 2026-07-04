//! `ingest.rs` — read an `.osm.pbf` into styled features (lines, closed-way
//! polygons, and multipolygon/`boundary` relation areas). Two `osmpbf` passes:
//!
//!   - **Pass 1** builds the `node_id → coord` store and collects qualifying area
//!     relations. Relations sit last in a sorted PBF, so one whole-file read sees
//!     them after the nodes — no extra pass.
//!   - **Pass 2** resolves ways into features + coastlines and captures the
//!     geometry of any way that is a relation member.
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

pub struct IngestFeature {
    pub style_id: u8,
    pub min_lod: usize,
    pub geom: Geom,
}

/// Coastlines are captured separately (always) — they feed the bbox and land/sea.
pub struct Ingested {
    pub features: Vec<IngestFeature>,
    pub coastlines: Vec<Vec<(f64, f64)>>,
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
    let mut nodes: HashMap<i64, (i32, i32)> = HashMap::new();
    let mut pending: Vec<PendingRelation> = Vec::new();
    let mut needed_ways: HashSet<i64> = HashSet::new();
    ElementReader::from_path(pbf_path)
        .map_err(|e| format!("open {pbf_path}: {e}"))?
        .for_each(|el| match el {
            Element::Node(n) => {
                nodes.insert(n.id(), (n.decimicro_lon(), n.decimicro_lat()));
            }
            Element::DenseNode(n) => {
                nodes.insert(n.id(), (n.decimicro_lon(), n.decimicro_lat()));
            }
            Element::Relation(r) => collect_relation(&r, config, &mut pending, &mut needed_ways),
            _ => {}
        })
        .map_err(|e| format!("pass 1 {pbf_path}: {e}"))?;

    // --- Pass 2: ways → features + coastlines, plus member-way geometry capture. ---
    let mut features = Vec::new();
    let mut coastlines = Vec::new();
    let mut member_geom: HashMap<i64, Vec<(f64, f64)>> = HashMap::with_capacity(needed_ways.len());
    ElementReader::from_path(pbf_path)
        .map_err(|e| format!("open {pbf_path}: {e}"))?
        .for_each(|el| {
            if let Element::Way(w) = el {
                let refs: Vec<i64> = w.refs().collect();
                // A missing node aborts the whole way — osmium would raise
                // `InvalidLocationError` here, and the way is dropped.
                let Some(coords) = resolve_coords(&refs, &nodes) else { return };
                process_way(&w, &refs, &coords, config, &mut features, &mut coastlines);
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

    Ok(Ingested { features, coastlines })
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
) {
    let tags: HashMap<&str, &str> = w.tags().collect();

    // Coastlines are captured ALWAYS — even if the way is also closed/styled — and
    // as lines, never areas.
    if tags.get("natural") == Some(&"coastline") && coords.len() >= 2 {
        coastlines.push(coords.to_vec());
    }

    let Some(style) = config.get_style(&tags) else { return };

    // A closed area emits a polygon; a closed road loop emits a line, never both.
    let is_closed = refs.len() >= 2 && refs.first() == refs.last();
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
