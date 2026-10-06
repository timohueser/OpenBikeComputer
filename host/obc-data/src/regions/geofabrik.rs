//! The cached Geofabrik area index. Public PBF URLs supply unambiguous full area paths.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::Bbox;

#[derive(Debug, Clone)]
pub struct Area {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    /// Polygons, each with an exterior followed by its holes, longitude first.
    pub polygons: Vec<Vec<Vec<[f64; 2]>>>,
    pub bounds: Bbox,
}

#[derive(Deserialize)]
struct Index {
    #[serde(rename = "type")]
    kind: String,
    features: Vec<Feature>,
}

#[derive(Deserialize)]
struct Feature {
    #[serde(rename = "type")]
    kind: String,
    properties: Properties,
    geometry: Geometry,
}

#[derive(Deserialize)]
struct Properties {
    id: String,
    name: String,
    parent: Option<String>,
    urls: Urls,
}

#[derive(Deserialize)]
struct Urls {
    pbf: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", content = "coordinates")]
enum Geometry {
    Polygon(Vec<Vec<[f64; 2]>>),
    MultiPolygon(Vec<Vec<Vec<[f64; 2]>>>),
}

pub fn parse(body: &[u8]) -> Result<BTreeMap<String, Area>, String> {
    let index: Index = serde_json::from_slice(body).map_err(|e| format!("Geofabrik index: {e}"))?;
    if index.kind != "FeatureCollection" || index.features.is_empty() {
        return Err("Geofabrik index is a non-empty FeatureCollection".into());
    }
    let mut ids = BTreeMap::new();
    let mut areas = BTreeMap::new();
    for feature in index.features {
        let properties = feature.properties;
        let id = properties
            .urls
            .pbf
            .strip_prefix("https://download.geofabrik.de/")
            .and_then(|path| path.strip_suffix("-latest.osm.pbf"))
            .filter(|path| !path.is_empty() && path.split('/').all(crate::is_kebab))
            .ok_or_else(|| format!("Geofabrik area `{}` has no valid public PBF URL", properties.id))?
            .to_string();
        if feature.kind != "Feature" || properties.name.trim().is_empty() {
            return Err(format!("Geofabrik area `{id}` has no name or feature"));
        }
        let polygons = match feature.geometry {
            Geometry::Polygon(rings) => vec![rings],
            Geometry::MultiPolygon(polygons) => polygons,
        };
        let mut bounds = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        if polygons.is_empty() || polygons.iter().any(Vec::is_empty) {
            return Err(format!("Geofabrik area `{id}` has no polygon"));
        }
        for ring in polygons.iter().flatten() {
            if ring.len() < 4 || ring.first() != ring.last() {
                return Err(format!("Geofabrik area `{id}` has an open or short ring"));
            }
            for &[lon, lat] in ring {
                if !(-180.0..=180.0).contains(&lon) || !(-90.0..=90.0).contains(&lat) {
                    return Err(format!("Geofabrik area `{id}` has invalid coordinates"));
                }
                bounds = [bounds[0].min(lon), bounds[1].min(lat), bounds[2].max(lon), bounds[3].max(lat)];
            }
        }
        let bounds = Bbox::new(bounds).map_err(|e| format!("Geofabrik area `{id}`: {e}"))?;
        if ids.insert(properties.id, id.clone()).is_some()
            || areas
                .insert(id.clone(), Area { id, name: properties.name, parent: properties.parent, polygons, bounds })
                .is_some()
        {
            return Err("Geofabrik index repeats an area".into());
        }
    }
    for area in areas.values_mut() {
        if let Some(parent) = &area.parent {
            area.parent = Some(
                ids.get(parent)
                    .ok_or_else(|| format!("Geofabrik area `{}` has unknown parent `{parent}`", area.id))?
                    .clone(),
            );
        }
    }
    Ok(areas)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_resolve_full_paths_and_parent_ids_and_geometry_is_checked() {
        let feature = |id: &str, path: &str, parent: Option<&str>| {
            serde_json::json!({
                "type": "Feature", "properties": {"id": id, "name": id, "parent": parent,
                    "urls": {"pbf": format!("https://download.geofabrik.de/{path}-latest.osm.pbf")}},
                "geometry": {"type": "MultiPolygon", "coordinates": [[[[7.0,47.0],[8.0,47.0],[8.0,48.0],[7.0,47.0]]]]}
            })
        };
        let body = serde_json::json!({"type":"FeatureCollection", "features":[
            feature("parent", "europe", None), feature("child", "europe/test", Some("parent"))]});
        let bytes = serde_json::to_vec(&body).unwrap();
        let areas = parse(&bytes).unwrap();
        assert_eq!(areas["europe/test"].parent.as_deref(), Some("europe"));
        assert_eq!(areas["europe/test"].bounds.west, 7.0);
        let mut bad = body.clone();
        bad["features"][1]["properties"]["urls"]["pbf"] = "https://download.geofabrik.de/../test-latest.osm.pbf".into();
        assert!(parse(&serde_json::to_vec(&bad).unwrap()).unwrap_err().contains("PBF URL"));
        bad = body;
        bad["features"][1]["geometry"]["coordinates"][0][0][3][0] = 9.into();
        assert!(parse(&serde_json::to_vec(&bad).unwrap()).unwrap_err().contains("ring"));
    }
}
