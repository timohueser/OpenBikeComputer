//! `ingest.rs` — port of `packer/obcm/ingest.py`. **Stage 3** covered lines +
//! closed ways; **Stage 4** adds multipolygon/`boundary` *relation* area assembly
//! (lakes-with-islands, multi-part forests) alongside them. Reads an `.osm.pbf`
//! with `osmpbf` in two passes:
//!
//!   - **Pass 1** builds the `node_id → coord` store **and** collects qualifying
//!     area relations (their style + member way-ids) — relations sit last in a
//!     sorted PBF, so a single whole-file read sees them after the nodes, no extra
//!     pass needed.
//!   - **Pass 2** resolves ways into features + coastlines (Stage 3) and captures
//!     the geometry of any way that is a relation member.
//!
//! Then each relation's member ways are assembled into polygons-with-holes via
//! GEOS `build_area` ([`assemble_multipolygon`]) and emitted styled by the
//! *relation's* tags. Stage 4 is **additive**: a tagged closed way that is also a
//! relation member still yields its own Stage-3 `from_way` polygon *and*
//! contributes to the relation polygon (matching the oracle).
//!
//! Closed-way classification keeps Amendment 2 (the closed-line-way fix): a closed
//! `highway=residential` loop becomes a line **only**, never also a filled blob.
//!
//! Coordinates are read *osmium's* way — `decimicro / 1e7` (see the `node_probe`
//! parity gate) — so the f64 lon/lat are bit-identical to the oracle's, and
//! everything downstream (bbox, simplify, serialize) lines up.

use std::collections::{HashMap, HashSet};

use osmpbf::{Element, ElementReader, RelMemberType};

use crate::config::Config;
use crate::geom::{assemble_multipolygon, polygon_is_valid, Geom};

/// A feature as it leaves ingest: a simple geometry plus its style id and the
/// `min_lod` gate. Mirrors the `{style_id, min_lod, geometry}` dicts `ingest.py`
/// appends to `self.features`.
pub struct IngestFeature {
    pub style_id: u8,
    pub min_lod: usize,
    pub geom: Geom,
}

/// Everything ingest produces: the styled features and the coastline lines
/// (captured separately, always — they feed the bbox and, later, land/sea).
pub struct Ingested {
    pub features: Vec<IngestFeature>,
    pub coastlines: Vec<Vec<(f64, f64)>>,
}

/// A qualifying area relation collected in pass 1, awaiting member geometry (pass
/// 2) and assembly. Holds owned data only (no borrow into the PBF buffer).
struct PendingRelation {
    style_id: u8,
    min_lod: usize,
    /// Member **way** ids, in member order. Roles are deliberately dropped —
    /// `build_area` classifies outer/inner by geometry (handover §3.1).
    member_ways: Vec<i64>,
}

/// The tags whose presence (with `area != no`) classifies a *closed* way as a
/// polygon. Mirrors `ingest.py::way`'s `area_tags`.
const AREA_TAGS: [&str; 6] = ["building", "landuse", "amenity", "leisure", "natural", "waterway"];

/// osmium derives lon/lat as `decimicro / 1e7` — division by the exact integer
/// `1e7`, never `* 1e-7`. The `node_probe` gate proved this is bit-identical to
/// the oracle. Keep it that way.
#[inline]
fn to_deg(decimicro: i32) -> f64 {
    decimicro as f64 / 1e7
}

/// Two-pass ingest of a single `.osm.pbf` (lines + closed-way polygons +
/// relation-assembled area polygons).
pub fn ingest_osm(pbf_path: &str, config: &Config) -> Result<Ingested, String> {
    // --- Pass 1: node-location store + relation collection. ---
    // The PBF is node-sorted so the store is filled before any relation is read
    // (relations come last); a full store is the safe, oracle-faithful choice —
    // Stage 6 can shrink it. ~8 B/node + overhead.
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
                // A missing node aborts the whole way — mirrors osmium raising
                // `InvalidLocationError`, caught by `ingest.py` (the way is dropped).
                let Some(coords) = resolve_coords(&refs, &nodes) else { return };
                process_way(&w, &refs, &coords, config, &mut features, &mut coastlines);
                // Capture geometry for relation assembly (move, no extra clone).
                if needed_ways.contains(&w.id()) {
                    member_geom.insert(w.id(), coords);
                }
            }
        })
        .map_err(|e| format!("pass 2 {pbf_path}: {e}"))?;

    // --- Assemble relation areas from captured member geometry (Stage 4). ---
    // Each outer ring (+ its nested holes) becomes one polygon, styled by the
    // relation. Un-assemblable/invalid relations yield nothing (skip-and-warn),
    // matching osmium silently dropping broken relations.
    //
    // **Completeness:** osmium's MultipolygonManager only assembles a relation when
    // ALL its member ways are present; an incomplete relation (a member way clipped
    // out of the extract) is dropped, not assembled from the surviving members.
    // We mirror that — assembling a partial ring would emit a phantom polygon the
    // oracle never produces (seen as over-production on boundary-crossing relations
    // in freiburg-town/monaco).
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
            continue; // incomplete relation → osmium drops it too
        }
        for poly in assemble_multipolygon(&members) {
            features.push(IngestFeature { style_id: pr.style_id, min_lod: pr.min_lod, geom: poly });
        }
    }

    Ok(Ingested { features, coastlines })
}

/// Resolve a way's node refs to degree coordinates against the node store.
/// `None` iff any node is missing — the caller drops the way, mirroring osmium's
/// `InvalidLocationError`.
fn resolve_coords(refs: &[i64], nodes: &HashMap<i64, (i32, i32)>) -> Option<Vec<(f64, f64)>> {
    let mut coords = Vec::with_capacity(refs.len());
    for r in refs {
        let &(dx, dy) = nodes.get(r)?;
        coords.push((to_deg(dx), to_deg(dy)));
    }
    Some(coords)
}

/// Collect a `type=multipolygon`/`type=boundary` relation (skipping `admin_level`)
/// for Stage-4 assembly: record its style + member way-ids. Mirrors which
/// relations osmium's `AreaManager` turns into areas, and `ingest.py::area()`'s
/// `admin_level` early-return. Member roles are ignored — geometry decides
/// outer/inner (handover §3.1); non-way members (nodes, sub-relations) are skipped.
fn collect_relation(
    r: &osmpbf::Relation,
    config: &Config,
    pending: &mut Vec<PendingRelation>,
    needed_ways: &mut HashSet<i64>,
) {
    let tags: HashMap<&str, &str> = r.tags().collect();
    // Only area relation types build polygons (osmium's AreaManager).
    match tags.get("type").copied() {
        Some("multipolygon") | Some("boundary") => {}
        _ => return,
    }
    // admin_level relations are line-only (handover §3.4) → no polygon.
    if tags.contains_key("admin_level") {
        return;
    }
    let Some(style) = config.get_style(&tags) else { return };
    let member_ways: Vec<i64> = r
        .members()
        .filter(|m| m.member_type == RelMemberType::Way)
        .map(|m| m.member_id)
        .collect();
    if member_ways.is_empty() {
        return;
    }
    for &wid in &member_ways {
        needed_ways.insert(wid);
    }
    pending.push(PendingRelation { style_id: style.id, min_lod: style.min_lod, member_ways });
}

/// One way (mirrors `ingest.py::way`): capture coastline always, then style +
/// classify into a single polygon-or-line emission. `refs`/`coords` are
/// pre-resolved (so the caller can also reuse `coords` for member capture).
fn process_way(
    w: &osmpbf::Way,
    refs: &[i64],
    coords: &[(f64, f64)],
    config: &Config,
    features: &mut Vec<IngestFeature>,
    coastlines: &mut Vec<Vec<(f64, f64)>>,
) {
    // Tags collected once, used for both styling and classification.
    let tags: HashMap<&str, &str> = w.tags().collect();

    // Coastlines are captured ALWAYS — even if the way is also closed/styled —
    // and as lines, never areas (matches `ingest.py::way`'s leading block).
    if tags.get("natural") == Some(&"coastline") && coords.len() >= 2 {
        coastlines.push(coords.to_vec());
    }

    let Some(style) = config.get_style(&tags) else { return };

    // Closed-way classification (plan Amendment 2 / handover §4): emit a polygon
    // iff it's an area, else a line — never both (the oracle's double-emit bug we
    // intentionally do not replicate).
    let is_closed = refs.len() >= 2 && refs.first() == refs.last();
    if is_closed && is_area(&tags) {
        // admin_level + area ⇒ the oracle drops it entirely (way() skips the line
        // because is_area, area() skips the polygon because admin_level). Match.
        if tags.contains_key("admin_level") {
            return;
        }
        // Closed ways are already first==last; no holes (those come from
        // relations). Mirror area()'s `len(ext_coords) >= 3` guard, and skip rings
        // osmium's assembler would reject as invalid (e.g. the self-intersecting
        // "Red House" building in malta) — it emits no polygon for those, and
        // crucially no line either (way() already returned).
        if coords.len() >= 3 && polygon_is_valid(coords, &[]) {
            features.push(IngestFeature {
                style_id: style.id,
                min_lod: style.min_lod,
                geom: Geom::Polygon { exterior: coords.to_vec(), interiors: Vec::new() },
            });
        }
        return;
    }

    // Line: open ways, and closed-but-not-area "circular roads" (W6 residential,
    // W12 natural=water area=no). Mirror way()'s `len(coords) >= 2` guard.
    if coords.len() >= 2 {
        features.push(IngestFeature {
            style_id: style.id,
            min_lod: style.min_lod,
            geom: Geom::Line(coords.to_vec()),
        });
    }
}

/// Closed-way area heuristic, byte-for-byte with `ingest.py::way`:
/// `area=yes` ⇒ area; `area=no` ⇒ never; otherwise area iff it carries any
/// [`AREA_TAGS`] key.
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

    /// The `tiny.osm` truth table (its header comment), as **Stage-4 Rust** sees
    /// it: relations are now assembled, and the closed-line-way double-emit stays
    /// fixed. So the Stage-3 result (7 features + 1 coastline) gains R1's lake
    /// (1 water polygon WITH a hole) and R2's two forest outers → 10 features.
    #[test]
    fn tiny_truth_table() {
        if !std::path::Path::new(TINY_PBF).exists() {
            eprintln!("SKIP tiny_truth_table: {TINY_PBF} missing (run build_corpus.sh)");
            return;
        }
        let cfg = Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packer/config.json"))
            .expect("config");
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
        let lake = ing
            .features
            .iter()
            .find(|f| f.style_id == 32 && is_polygon(&f.geom))
            .expect("water polygon");
        match &lake.geom {
            Geom::Polygon { interiors, .. } => assert_eq!(interiors.len(), 1, "R1 has one hole"),
            _ => unreachable!(),
        }

        // The fixes/omissions we MUST honor:
        assert_eq!(n(12, true), 0, "no residential blob (closed-line-way fix)");
        // 5 polygons (3 forest, 1 pedestrian, 1 water lake) + 5 lines.
        assert_eq!(ing.features.len(), 10, "10 features total");
    }
}
