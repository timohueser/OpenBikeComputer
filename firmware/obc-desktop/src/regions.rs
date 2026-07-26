//! The Geofabrik download index, as the region picker wants it.
//!
//! This is `packer/web_builder/geofabrik.py` in Rust, on purpose and field for
//! field: the raw `index-v1.json` is a GeoJSON FeatureCollection of every
//! downloadable extract with a full-resolution boundary, which is tens of
//! megabytes of coastline the picker has no use for. So the properties are
//! trimmed, the geometries are simplified, and the result is cached.
//!
//! Two things are deliberately shared with the Python implementation rather than
//! reinvented:
//!
//! * **The cache files.** Same directory, same names, same document shape — a
//!   developer running both the dev server and the app fetches the index once.
//! * **The simplification.** `.simplify(0.01)` in shapely is
//!   `GEOSTopologyPreserveSimplify`, which is exactly
//!   [`obc_pack::geom::topology_preserve_simplify`]. The packer already links
//!   GEOS, so the app needs no second geometry stack to produce the same outlines
//!   at the same tolerance.

use std::path::Path;
use std::time::{Duration, SystemTime};

use obc_pack::geom::{topology_preserve_simplify, Geom};
use serde_json::{json, Map, Value};

const INDEX_URL: &str = "https://download.geofabrik.de/index-v1.json";

/// ~0.01° ≈ 1 km. Plenty for a world-scale picker outline, and the tolerance the
/// dev server has always used.
const SIMPLIFY_TOLERANCE: f64 = 0.01;

/// Re-fetch the raw index at most weekly. Regions change rarely and the file is
/// large.
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

/// The trimmed FeatureCollection, building it if the cache is cold or stale.
pub fn regions() -> Result<Value, String> {
    let dir = crate::paths::geofabrik_cache();
    let raw_path = dir.join("index-v1.json");
    let simplified_path = dir.join("regions-simplified.json");

    ensure_raw_index(&raw_path)?;
    if let Some(cached) = fresh_simplified(&simplified_path, &raw_path) {
        return Ok(cached);
    }
    let raw = std::fs::read_to_string(&raw_path).map_err(|e| format!("read {}: {e}", raw_path.display()))?;
    let fc = build_simplified(&raw)?;
    // A failed write is not a failed request: the picker works, it just pays the
    // simplify again next launch.
    if let Ok(text) = serde_json::to_string(&fc) {
        let _ = std::fs::write(&simplified_path, text);
    }
    Ok(fc)
}

/// The `.pbf` URL for each id, in the order asked for and deduplicated — a build
/// selecting a region and its parent must not download the same file twice.
/// Mirrors `geofabrik.region_pbf_urls`.
pub fn pbf_urls(ids: &[String]) -> Result<Vec<(String, String)>, String> {
    let fc = regions()?;
    let features = fc.get("features").and_then(Value::as_array).ok_or("region index has no features")?;
    let mut out: Vec<(String, String)> = Vec::new();
    for id in ids {
        let found = features
            .iter()
            .find(|f| f.pointer("/properties/id").and_then(Value::as_str) == Some(id.as_str()))
            .and_then(|f| f.pointer("/properties/pbf_url").and_then(Value::as_str))
            .ok_or_else(|| format!("unknown region id: {id}"))?;
        if !out.iter().any(|(_, u)| u == found) {
            out.push((id.clone(), found.to_string()));
        }
    }
    Ok(out)
}

fn ensure_raw_index(raw_path: &Path) -> Result<(), String> {
    if let Ok(meta) = std::fs::metadata(raw_path) {
        let fresh = meta
            .modified()
            .ok()
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .is_some_and(|age| age < MAX_AGE);
        if fresh && meta.len() > 0 {
            return Ok(());
        }
    }
    let dir = raw_path.parent().expect("cache path has a parent");
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let body = crate::http::get_text(INDEX_URL).map_err(|e| format!("fetch the Geofabrik index: {e}"))?;
    // Written whole and moved into place: a half-written index that survives a
    // crash would poison every later launch with a parse error.
    let tmp = raw_path.with_extension("json.part");
    std::fs::write(&tmp, &body).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, raw_path).map_err(|e| format!("install {}: {e}", raw_path.display()))?;
    Ok(())
}

fn fresh_simplified(simplified: &Path, raw: &Path) -> Option<Value> {
    let (s, r) = (std::fs::metadata(simplified).ok()?, std::fs::metadata(raw).ok()?);
    if s.modified().ok()? < r.modified().ok()? {
        return None; // the index was refreshed under it
    }
    serde_json::from_str(&std::fs::read_to_string(simplified).ok()?).ok()
}

fn build_simplified(raw: &str) -> Result<Value, String> {
    let raw: Value = serde_json::from_str(raw).map_err(|e| format!("parse the Geofabrik index: {e}"))?;
    let features = raw.get("features").and_then(Value::as_array).ok_or("the Geofabrik index has no features")?;

    let mut out: Vec<Value> = Vec::with_capacity(features.len());
    for feat in features {
        let props = feat.get("properties").and_then(Value::as_object);
        // Only regions that can actually be downloaded — the index also lists
        // container entries with no `.pbf`.
        let Some(pbf_url) = props.and_then(|p| p.get("urls")).and_then(|u| u.get("pbf")).and_then(Value::as_str) else {
            continue;
        };
        let props = props.expect("checked above");
        let geometry = feat.get("geometry").map(simplify_geometry).unwrap_or(Value::Null);
        out.push(json!({
            "type": "Feature",
            "properties": {
                "id": props.get("id").cloned().unwrap_or(Value::Null),
                "name": clean_name(props.get("name").and_then(Value::as_str).unwrap_or("")),
                "parent": props.get("parent").cloned().unwrap_or(Value::Null),
                "pbf_url": pbf_url,
            },
            "geometry": geometry,
        }));
    }

    // Which regions can be expanded, so the picker can offer subregions.
    let parents: std::collections::HashSet<String> = out
        .iter()
        .filter_map(|f| f.pointer("/properties/parent").and_then(Value::as_str).map(str::to_string))
        .collect();
    for f in &mut out {
        let id = f.pointer("/properties/id").and_then(Value::as_str).unwrap_or("").to_string();
        if let Some(props) = f.get_mut("properties").and_then(Value::as_object_mut) {
            props.insert("has_children".into(), Value::Bool(parents.contains(&id)));
        }
    }

    Ok(json!({ "type": "FeatureCollection", "features": out }))
}

/// A few Geofabrik names embed HTML (`"Nord-Norge<br />(Northern Norway)"`).
fn clean_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut in_tag = false;
    for c in name.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("  ", " ").trim().to_string()
}

/// GeoJSON → [`Geom`] → simplify → GeoJSON. A geometry that doesn't survive the
/// round trip is passed through untouched: a slightly heavy outline is a much
/// better outcome than a region the picker cannot draw.
fn simplify_geometry(geometry: &Value) -> Value {
    let Some(parsed) = geom_from_geojson(geometry) else {
        return geometry.clone();
    };
    let simplified = topology_preserve_simplify(&parsed, SIMPLIFY_TOLERANCE);
    geojson_from_geom(&simplified).unwrap_or_else(|| geometry.clone())
}

fn ring(value: &Value) -> Option<Vec<(f64, f64)>> {
    let pts: Vec<(f64, f64)> = value
        .as_array()?
        .iter()
        .filter_map(|p| {
            let a = p.as_array()?;
            Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))
        })
        .collect();
    (pts.len() >= 4).then_some(pts)
}

fn polygon(value: &Value) -> Option<Geom> {
    let rings = value.as_array()?;
    let exterior = ring(rings.first()?)?;
    let interiors = rings.iter().skip(1).filter_map(ring).collect();
    Some(Geom::Polygon { exterior, interiors })
}

fn geom_from_geojson(geometry: &Value) -> Option<Geom> {
    let coords = geometry.get("coordinates")?;
    match geometry.get("type")?.as_str()? {
        "Polygon" => polygon(coords),
        "MultiPolygon" => {
            let parts: Vec<Geom> = coords.as_array()?.iter().filter_map(polygon).collect();
            (!parts.is_empty()).then_some(Geom::Multi(parts))
        }
        _ => None,
    }
}

fn ring_json(r: &[(f64, f64)]) -> Value {
    Value::Array(r.iter().map(|&(x, y)| json!([x, y])).collect())
}

fn polygon_json(g: &Geom) -> Option<Value> {
    match g {
        Geom::Polygon { exterior, interiors } => {
            let mut rings = vec![ring_json(exterior)];
            rings.extend(interiors.iter().map(|r| ring_json(r)));
            Some(Value::Array(rings))
        }
        _ => None,
    }
}

fn geojson_from_geom(g: &Geom) -> Option<Value> {
    match g {
        Geom::Polygon { .. } => Some(json!({ "type": "Polygon", "coordinates": polygon_json(g)? })),
        // A simplified multipolygon can collapse to a single polygon; keep the
        // GeoJSON type honest either way.
        Geom::Multi(parts) => {
            let polys: Vec<Value> = parts.iter().filter_map(polygon_json).collect();
            match polys.len() {
                0 => None,
                1 => Some(json!({ "type": "Polygon", "coordinates": polys.into_iter().next()? })),
                _ => Some(json!({ "type": "MultiPolygon", "coordinates": polys })),
            }
        }
        _ => None,
    }
}

/// The five properties the picker reads, so a trimmed feature that lost one fails
/// here rather than in the webview.
#[allow(dead_code)]
fn assert_shape(props: &Map<String, Value>) -> bool {
    ["id", "name", "parent", "pbf_url", "has_children"].iter().all(|k| props.contains_key(*k))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW: &str = r#"{
        "type": "FeatureCollection",
        "features": [
            {"properties": {"id": "europe", "name": "Europe", "parent": null,
                            "urls": {"pbf": "https://example.invalid/europe.osm.pbf"}},
             "geometry": {"type": "Polygon", "coordinates":
                [[[0,0],[10,0],[10,0.0001],[10,10],[0,10],[0,0]]]}},
            {"properties": {"id": "monaco", "name": "Monaco<br />(MC)", "parent": "europe",
                            "urls": {"pbf": "https://example.invalid/monaco.osm.pbf"}},
             "geometry": {"type": "Polygon", "coordinates": [[[1,1],[2,1],[2,2],[1,2],[1,1]]]}},
            {"properties": {"id": "no-download", "name": "Container", "parent": null, "urls": {}},
             "geometry": {"type": "Polygon", "coordinates": [[[0,0],[1,0],[1,1],[0,0]]]}}
        ]
    }"#;

    #[test]
    fn trims_to_the_five_properties_the_picker_reads() {
        let fc = build_simplified(RAW).expect("build");
        let feats = fc["features"].as_array().expect("features");
        // The entry with no `.pbf` URL is not a region anyone can build.
        assert_eq!(feats.len(), 2);
        for f in feats {
            assert!(assert_shape(f["properties"].as_object().expect("properties")));
        }
        assert_eq!(feats[0]["properties"]["has_children"], Value::Bool(true));
        assert_eq!(feats[1]["properties"]["has_children"], Value::Bool(false));
    }

    #[test]
    fn strips_the_html_a_few_geofabrik_names_carry() {
        let fc = build_simplified(RAW).expect("build");
        assert_eq!(fc["features"][1]["properties"]["name"], "Monaco (MC)");
    }

    #[test]
    fn simplification_drops_the_vertex_no_one_can_see() {
        let fc = build_simplified(RAW).expect("build");
        // Europe's ring carries a 0.0001° detour — under the 0.01° tolerance, so
        // GEOS removes it while the corners stay.
        let ring = fc["features"][0]["geometry"]["coordinates"][0].as_array().expect("ring");
        assert_eq!(ring.len(), 5, "expected the sub-tolerance vertex to be gone: {ring:?}");
    }

    #[test]
    fn a_geometry_type_the_picker_cannot_use_is_passed_through_untouched() {
        let point = json!({"type": "Point", "coordinates": [1.0, 2.0]});
        assert_eq!(simplify_geometry(&point), point);
    }
}
