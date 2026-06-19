//! `ingest.rs` — port of `packer/obcm/ingest.py` for **Stage 3**: lines + closed
//! ways, skipping multipolygon *relations* (those are Stage 4). Reads an
//! `.osm.pbf` with `osmpbf` in two passes — pass 1 builds a `node_id → coord`
//! store, pass 2 resolves ways — and classifies each styled way as a polygon or a
//! line per the plan's Amendment 2 (the closed-line-way fix): a closed
//! `highway=residential` loop becomes a line **only**, never also a filled blob.
//!
//! Coordinates are read *osmium's* way — `decimicro / 1e7` (see the `node_probe`
//! parity gate) — so the f64 lon/lat are bit-identical to the oracle's, and
//! everything downstream (bbox, simplify, serialize) lines up.

use std::collections::HashMap;

use osmpbf::{Element, ElementReader};

use crate::config::Config;
use crate::geom::{polygon_is_valid, Geom};

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

/// Two-pass ingest of a single `.osm.pbf`. Relations are skipped (Stage 4).
pub fn ingest_osm(pbf_path: &str, config: &Config) -> Result<Ingested, String> {
    // --- Pass 1: node-location store (decimicrodegree lon/lat). ---
    // The PBF is node-sorted so a single pass would do, but a full store is the
    // safe, oracle-faithful choice; Stage 6 can shrink it. ~8 B/node + overhead.
    let mut nodes: HashMap<i64, (i32, i32)> = HashMap::new();
    ElementReader::from_path(pbf_path)
        .map_err(|e| format!("open {pbf_path}: {e}"))?
        .for_each(|el| match el {
            Element::Node(n) => {
                nodes.insert(n.id(), (n.decimicro_lon(), n.decimicro_lat()));
            }
            Element::DenseNode(n) => {
                nodes.insert(n.id(), (n.decimicro_lon(), n.decimicro_lat()));
            }
            _ => {}
        })
        .map_err(|e| format!("pass 1 {pbf_path}: {e}"))?;

    // --- Pass 2: ways → features + coastlines. ---
    let mut features = Vec::new();
    let mut coastlines = Vec::new();
    ElementReader::from_path(pbf_path)
        .map_err(|e| format!("open {pbf_path}: {e}"))?
        .for_each(|el| {
            if let Element::Way(w) = el {
                process_way(&w, config, &nodes, &mut features, &mut coastlines);
            }
        })
        .map_err(|e| format!("pass 2 {pbf_path}: {e}"))?;

    Ok(Ingested { features, coastlines })
}

/// One way (mirrors `ingest.py::way`): capture coastline always, then style +
/// classify into a single polygon-or-line emission.
fn process_way(
    w: &osmpbf::Way,
    config: &Config,
    nodes: &HashMap<i64, (i32, i32)>,
    features: &mut Vec<IngestFeature>,
    coastlines: &mut Vec<Vec<(f64, f64)>>,
) {
    // Resolve coords from the store. A missing node aborts the whole way — this
    // mirrors osmium raising `InvalidLocationError` for the list comprehension,
    // caught by `ingest.py`'s try/except (the way is dropped).
    let refs: Vec<i64> = w.refs().collect();
    let mut coords: Vec<(f64, f64)> = Vec::with_capacity(refs.len());
    for r in &refs {
        match nodes.get(r) {
            Some(&(dx, dy)) => coords.push((to_deg(dx), to_deg(dy))),
            None => return,
        }
    }

    // Tags collected once, used for both styling and classification.
    let tags: HashMap<&str, &str> = w.tags().collect();

    // Coastlines are captured ALWAYS — even if the way is also closed/styled —
    // and as lines, never areas (matches `ingest.py::way`'s leading block).
    if tags.get("natural") == Some(&"coastline") && coords.len() >= 2 {
        coastlines.push(coords.clone());
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
        // relations, Stage 4). Mirror area()'s `len(ext_coords) >= 3` guard, and
        // skip rings osmium's assembler would reject as invalid (e.g. the
        // self-intersecting "Red House" building in malta) — it emits no polygon
        // for those, and crucially no line either (way() already returned).
        if coords.len() >= 3 && polygon_is_valid(&coords, &[]) {
            features.push(IngestFeature {
                style_id: style.id,
                min_lod: style.min_lod,
                geom: Geom::Polygon { exterior: coords, interiors: Vec::new() },
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
            geom: Geom::Line(coords),
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

    /// The `tiny.osm` truth table (its header comment), as **Stage-3 Rust** sees
    /// it: relations are skipped and the closed-line-way double-emit is fixed, so
    /// the 11-feature/1-coastline oracle result becomes 7 features + 1 coastline.
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
        assert_eq!(n(39, true), 1, "W5 closed landuse=forest ⇒ 1 polygon");
        assert_eq!(n(15, true), 1, "W11 highway=pedestrian area=yes ⇒ 1 polygon");
        assert_eq!(n(12, false), 1, "W6 closed highway=residential ⇒ 1 line");
        assert_eq!(n(5, false), 1, "W7 highway=primary ⇒ 1 line");
        assert_eq!(n(3, false), 1, "W7b highway=trunk ⇒ 1 line");
        assert_eq!(n(42, false), 1, "W9 admin_level=2 ⇒ 1 line");
        assert_eq!(n(32, false), 1, "W12 natural=water area=no ⇒ 1 line");

        // The fixes/omissions we MUST honor:
        assert_eq!(n(12, true), 0, "no residential blob (closed-line-way fix)");
        assert_eq!(n(32, true), 0, "no water polygon (R1 relation skipped in Stage 3)");
        // forest polygons come only from W5, not R2's two outer rings (relations skipped).
        assert_eq!(ing.features.len(), 7, "7 features total (2 polygons + 5 lines)");
    }
}
