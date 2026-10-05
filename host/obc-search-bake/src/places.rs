use super::{enrich::Index, geometry, input::Feature};
use geo::Geometry;
use osmpbfreader::OsmId;
use serde_json::{json, Value};

fn kind(f: &Feature) -> Option<(&str, &str)> {
    if let Some(kind) = obc_places::classify(f.tags.iter().map(|(k, v)| (k.as_str(), v.as_str()))) {
        return Some((kind.key, kind.value));
    }
    [
        "place",
        "amenity",
        "shop",
        "tourism",
        "leisure",
        "office",
        "craft",
        "emergency",
        "healthcare",
        "historic",
        "railway",
        "aeroway",
        "aerialway",
        "natural",
        "man_made",
        "waterway",
        "club",
        "military",
        "highway",
        "mountain_pass",
        "boundary",
        "landuse",
        "building",
    ]
    .into_iter()
    .find_map(|key| {
        let value = f.tag(key);
        (!value.is_empty() && value != "no").then_some((key, value))
    })
}

pub fn record(f: &Feature, i: usize, index: &Index<'_>) -> Option<Value> {
    if f.road() || f.tags.contains_key("_interpolation_range") {
        return None;
    }
    let (key, value) = kind(f)?;
    // Keep the OSM place node as the searchable identity of a linked administrative area.
    if key == "boundary" && index.linked_place(i).is_some() {
        return None;
    }
    if !f.tags.contains_key("_poi") && f.name().is_empty() {
        return None;
    }
    let shared = obc_places::classify(f.tags.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    if shared.is_some_and(|k| k.value == "peak") && !matches!(f.source, OsmId::Node(_)) {
        return None;
    }
    if (key == "place" || value == "peak") && f.name().trim().is_empty() {
        return None;
    }
    let point = match (&f.source, &f.geometry) {
        (OsmId::Way(_), Geometry::Polygon(p)) if shared.is_some() => {
            let coords: Vec<_> = p.exterior().0.iter().map(|c| (c.x, c.y)).collect();
            let (x, y) = obc_places::ring_centroid(&coords);
            geo::Point::new(x, y)
        }
        (OsmId::Way(_), _) if shared.is_some() => return None,
        (OsmId::Relation(_), Geometry::MultiPolygon(polygons)) if shared.is_some() => {
            let rings: Vec<Vec<_>> =
                polygons.0.iter().map(|p| p.exterior().0.iter().map(|c| (c.x, c.y)).collect()).collect();
            let (x, y) = obc_places::area_center(rings.iter().map(Vec::as_slice))?;
            geo::Point::new(x, y)
        }
        _ => index.center(i),
    };
    let mut name: std::collections::BTreeMap<_, _> = f
        .tags
        .iter()
        .filter(|(k, _)| {
            k.as_str() == "name"
                || k.starts_with("name:")
                || matches!(k.as_str(), "alt_name" | "short_name" | "official_name" | "loc_name" | "int_name")
        })
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    if let Some(boundary) = index.place_boundary(i) {
        for (key, value) in boundary.tags.iter() {
            if key.as_str() == "name" || key.starts_with("name:") || key.ends_with("_name") {
                name.entry(key.to_string()).or_insert_with(|| value.to_string());
            }
        }
    }
    let extra: std::collections::BTreeMap<_, _> = f
        .tags
        .iter()
        .filter(|(k, _)| {
            k.starts_with("contact:")
                || k.starts_with("description")
                || matches!(k.as_str(), "opening_hours" | "website" | "phone" | "cuisine" | "population" | "ele")
        })
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let (object_type, object_id) = match f.source {
        OsmId::Node(n) => ("N", n.0),
        OsmId::Way(w) => ("W", w.0),
        OsmId::Relation(r) => ("R", r.0),
    };
    let (address, country) = index.address(i);
    let extent = geometry::envelope(&index.place_boundary(i).unwrap_or(f).geometry);
    let address_type = if key == "place" {
        value
    } else if key == "boundary" {
        index.address_type(i)
    } else {
        key
    };
    Some(json!({"object_type":object_type,"object_id":object_id,"osm_key":key,"osm_value":value,
        "address_type":address_type,"country_code":country,
        "centroid":[f64::from(obc_places::to_udeg(point.x())) / 1e6, f64::from(obc_places::to_udeg(point.y())) / 1e6],
        "bbox":[extent.lower()[0],extent.lower()[1],extent.upper()[0],extent.upper()[1]],
        "name":name,"extra":extra,"address":address,"postcode":address.get("postcode").map(String::as_str).unwrap_or(""),
        "importance":(0.75 - f64::from(index.search_rank(i)) / 40.).max(0.05)}))
}
