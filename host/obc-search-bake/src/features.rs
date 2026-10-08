use super::geometry::{coordinates, polygons, rings};
use geo::Intersects;
use geo::{Geometry, Point, Polygon};
use osmpbfreader::{OsmId, OsmObj, Relation, Tags, Way};
use rstar::RTree;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub struct Feature {
    pub source: OsmId,
    pub tags: Tags,
    pub geometry: Geometry,
}

impl Feature {
    pub fn tag(&self, name: &str) -> &str {
        self.tags.get(name).map(|s| s.as_str()).unwrap_or("")
    }
    pub fn name(&self) -> &str {
        self.tag("name")
    }
    pub fn road(&self) -> bool {
        matches!(self.source, OsmId::Way(_))
            && !(self.tag("highway") == "footway" && matches!(self.tag("footway"), "sidewalk" | "crossing" | "link"))
            && (!self.name().is_empty()
                || matches!(
                    self.tag("highway"),
                    "motorway"
                        | "trunk"
                        | "primary"
                        | "secondary"
                        | "tertiary"
                        | "unclassified"
                        | "residential"
                        | "road"
                        | "living_street"
                        | "pedestrian"
                ))
            && matches!(
                self.tag("highway"),
                "service"
                    | "cycleway"
                    | "path"
                    | "footway"
                    | "steps"
                    | "bridleway"
                    | "motorway_link"
                    | "primary_link"
                    | "trunk_link"
                    | "secondary_link"
                    | "tertiary_link"
                    | "residential"
                    | "track"
                    | "unclassified"
                    | "tertiary"
                    | "secondary"
                    | "primary"
                    | "living_street"
                    | "trunk"
                    | "motorway"
                    | "pedestrian"
                    | "road"
            )
    }
    pub fn address_tags(&self) -> bool {
        self.tags.keys().any(|k| k.starts_with("addr:"))
    }
}

pub fn poi(tags: &Tags) -> bool {
    let named = tags.keys().any(|k| k.as_str() == "name" || k.starts_with("name:"));
    obc_places::classify(tags.iter().map(|(k, v)| (k.as_str(), v.as_str()))).is_some()
        || tags.iter().any(|(key, value)| match (key.as_str(), value.as_str()) {
            (_, "no") => false,
            ("amenity", "parking_space" | "parking_entrance" | "waste_disposal" | "hunting_stand") => false,
            (
                "highway",
                "turning_circle" | "mini_roundabout" | "noexit" | "crossing" | "give_way" | "stop" | "turning_loop"
                | "passing_place",
            ) => false,
            ("highway", "street_lamp" | "traffic_signals") => named,
            ("emergency", "yes" | "fire_hydrant")
            | ("healthcare" | "historic" | "military" | "tourism", "yes")
            | ("aerialway", "pylon") => false,
            ("tourism", "information") => !tags.contains_key("information"),
            ("information", "yes" | "route_marker" | "trail_blaze") => false,
            ("information", _) => tags.get("tourism").is_some_and(|v| v == "information"),
            ("leisure", "nature_reserve" | "swimming_pool" | "garden" | "common") => named,
            (
                "railway",
                "rail" | "abandoned" | "disused" | "razed" | "level_crossing" | "switch" | "signal" | "buffer_stop",
            ) => false,
            ("railway" | "building" | "natural" | "waterway", _) => named,
            (
                "amenity" | "shop" | "tourism" | "craft" | "office" | "emergency" | "historic" | "leisure" | "club"
                | "military" | "healthcare" | "aerialway" | "aeroway" | "highway" | "place" | "mountain_pass",
                _,
            ) => true,
            ("man_made", "pier" | "tower" | "bridge" | "water_tower" | "lighthouse" | "watermill" | "tunnel") => true,
            ("man_made", "works" | "dyke" | "adit") => named,
            ("addr:housename", name) => !name.trim().is_empty(),
            _ => false,
        })
}

fn wanted(tags: &Tags) -> bool {
    tags.keys().any(|k| k.starts_with("addr:"))
        || ["postal_code", "postcode", "tiger:zip_left", "tiger:zip_right"].iter().any(|k| tags.contains_key(*k))
        || poi(tags)
        || tags.contains_key("name")
        || tags.contains_key("highway")
        || tags.get("boundary").is_some_and(|v| v == "postal_code")
}

fn address_tags(mut tags: Tags) -> Tags {
    if poi(&tags) {
        tags.insert("_poi".into(), "yes".into());
    }
    if let Some(code) = ["postal_code", "postcode", "addr:postcode", "tiger:zip_left", "tiger:zip_right"]
        .into_iter()
        .find_map(|key| tags.get(key).filter(|value| !value.is_empty()).cloned())
    {
        tags.insert("addr:postcode".into(), code);
    }
    tags.into_inner()
        .into_iter()
        .filter(|(k, _)| {
            k.starts_with("contact:")
                || k.starts_with("description")
                || k.starts_with("addr:")
                || k.starts_with("name:")
                || matches!(
                    k.as_str(),
                    "name"
                        | "alt_name"
                        | "loc_name"
                        | "int_name"
                        | "short_name"
                        | "official_name"
                        | "place"
                        | "boundary"
                        | "admin_level"
                        | "highway"
                        | "area"
                        | "building"
                        | "wikidata"
                        | "postal_code"
                        | "landuse"
                        | "capital"
                        | "ISO3166-1"
                        | "ISO3166-1:alpha2"
                        | "ref"
                        | "footway"
                        | "_poi"
                        | "leisure"
                        | "natural"
                        | "water"
                        | "waterway"
                        | "mountain_pass"
                        | "historic"
                        | "amenity"
                        | "shop"
                        | "tourism"
                        | "office"
                        | "craft"
                        | "railway"
                        | "emergency"
                        | "healthcare"
                        | "club"
                        | "military"
                        | "aerialway"
                        | "aeroway"
                        | "man_made"
                        | "information"
                        | "opening_hours"
                        | "website"
                        | "phone"
                        | "cuisine"
                        | "population"
                        | "ele"
                )
        })
        .collect()
}

fn line(ids: &[i64], nodes: &[(i64, i32, i32)]) -> Option<geo::LineString> {
    let points: Option<Vec<_>> = ids
        .iter()
        .map(|id| {
            nodes
                .binary_search_by_key(id, |n| n.0)
                .ok()
                .map(|i| [f64::from(nodes[i].1) / 1e7, f64::from(nodes[i].2) / 1e7])
        })
        .collect();
    Some(coordinates(points?))
}

fn relation_geometry(r: &Relation, ways: &BTreeMap<i64, Vec<i64>>, nodes: &[(i64, i32, i32)]) -> Option<Geometry> {
    let mut outer = Vec::new();
    let mut inner = Vec::new();
    for member in &r.refs {
        if let OsmId::Way(id) = member.member {
            let parts = match member.role.as_str() {
                "" | "outer" => &mut outer,
                "inner" => &mut inner,
                _ => continue,
            };
            parts.push(ways.get(&id.0)?.clone());
        } else if member.role == "outer" || member.role == "inner" {
            return None;
        }
    }
    let convert = |parts| rings(parts)?.iter().map(|ids| line(ids, nodes)).collect::<Option<Vec<_>>>();
    polygons(convert(outer)?, convert(inner)?)
}

pub struct Input {
    pub features: Vec<Feature>,
    pub associated: BTreeMap<OsmId, Vec<OsmId>>,
    pub labels: BTreeMap<OsmId, Vec<OsmId>>,
    pub incomplete_geometries: usize,
    pub nodes: usize,
}

pub fn read(path: &Path) -> Result<Input, Box<dyn std::error::Error>> {
    let mut input = Input {
        features: Vec::new(),
        associated: BTreeMap::new(),
        labels: BTreeMap::new(),
        incomplete_geometries: 0,
        nodes: 0,
    };
    let mut nodes = Vec::new();
    let mut ways: Vec<Way> = Vec::new();
    let mut relations = Vec::new();
    let mut needed = BTreeSet::new();
    super::input::read(path, |object| match object {
        OsmObj::Node(n) => {
            nodes.push((n.id.0, n.decimicro_lon, n.decimicro_lat));
            if wanted(&n.tags) {
                input.features.push(Feature {
                    source: OsmId::Node(n.id),
                    geometry: Point::new(f64::from(n.decimicro_lon) / 1e7, f64::from(n.decimicro_lat) / 1e7).into(),
                    tags: address_tags(n.tags),
                });
            }
        }
        OsmObj::Way(mut w) if wanted(&w.tags) => {
            w.tags = address_tags(w.tags);
            ways.push(w);
        }
        OsmObj::Relation(r) if r.tags.get("type").is_some_and(|v| v == "associatedStreet") => {
            let streets: Vec<_> = r.refs.iter().filter(|m| m.role == "street").map(|m| m.member).collect();
            for m in r.refs.iter().filter(|m| m.role == "house" || m.role == "address") {
                input.associated.entry(m.member).or_default().extend(&streets);
            }
        }
        OsmObj::Relation(mut r)
            if wanted(&r.tags) && r.tags.get("type").is_some_and(|v| v == "multipolygon" || v == "boundary") =>
        {
            input
                .labels
                .insert(OsmId::Relation(r.id), r.refs.iter().filter(|m| m.role == "label").map(|m| m.member).collect());
            for m in &r.refs {
                if let OsmId::Way(id) = m.member {
                    needed.insert(id.0);
                }
            }
            r.tags = address_tags(r.tags);
            relations.push(r);
        }
        _ => {}
    })?;
    input.nodes = nodes.len();
    nodes.sort_unstable_by_key(|n| n.0);
    let mut members = BTreeMap::new();
    super::input::read(path, |object| {
        if let OsmObj::Way(w) = object {
            if needed.contains(&w.id.0) {
                members.insert(w.id.0, w.nodes.iter().map(|n| n.0).collect());
            }
        }
    })?;
    let endpoints: BTreeMap<_, _> = input
        .features
        .iter()
        .filter_map(|f| match f.source {
            OsmId::Node(id) if !f.tag("addr:housenumber").is_empty() => Some((id.0, &f.tags)),
            _ => None,
        })
        .collect();
    let mut interpolated = Vec::new();
    for w in ways {
        let ids: Vec<_> = w.nodes.iter().map(|n| n.0).collect();
        let geometry = line(&ids, &nodes).filter(|l| l.0.len() >= 2).map(|l| {
            if ids.len() >= 4
                && ids.first() == ids.last()
                && (!w.tags.contains_key("highway") || w.tags.get("area").is_some_and(|v| v == "yes"))
            {
                Geometry::Polygon(Polygon::new(l, vec![]))
            } else {
                Geometry::LineString(l)
            }
        });
        if let Some(geometry) = geometry {
            if let Geometry::LineString(line) = &geometry {
                if w.tags.contains_key("addr:interpolation") {
                    let numbered: Vec<_> =
                        ids.iter().enumerate().filter_map(|(i, id)| endpoints.get(id).map(|tags| (i, *tags))).collect();
                    interpolated.extend(super::interpolation::ranges(OsmId::Way(w.id), line, &w.tags, &numbered));
                }
            }
            interpolated.push(Feature { source: OsmId::Way(w.id), tags: w.tags, geometry });
        } else {
            input.incomplete_geometries += 1;
        }
    }
    input.features.extend(interpolated);
    for r in relations {
        if let Some(geometry) = relation_geometry(&r, &members, &nodes) {
            input.features.push(Feature { source: OsmId::Relation(r.id), tags: r.tags, geometry });
        } else {
            input.incomplete_geometries += 1;
        }
    }
    // A place node may also be a POI that inherits a building address.
    let pois: Vec<_> = input
        .features
        .iter()
        .filter(|f| {
            matches!(f.source, OsmId::Node(_))
                && !f.tag("place").is_empty()
                && ["amenity", "shop", "tourism", "office", "craft"].iter().any(|key| !f.tag(key).is_empty())
        })
        .map(|f| {
            let mut tags = f.tags.clone();
            tags.remove("place");
            Feature { source: f.source, tags, geometry: f.geometry.clone() }
        })
        .collect();
    input.features.extend(pois);
    input.features.sort_by_key(|f| f.source);
    let buildings = RTree::bulk_load(
        input
            .features
            .iter()
            .enumerate()
            .filter(|(_, f)| f.address_tags() && matches!(f.geometry, Geometry::Polygon(_) | Geometry::MultiPolygon(_)))
            .map(|(index, f)| super::geometry::Entry { index, envelope: super::geometry::envelope(&f.geometry) })
            .collect(),
    );
    let keep: Vec<_> = input
        .features
        .iter()
        .map(|f| {
            f.tags.contains_key("_poi")
                || f.address_tags()
                || f.road()
                || !f.tag("place").is_empty()
                || !f.tag("boundary").is_empty()
                || (!f.tag("landuse").is_empty() && !f.name().is_empty())
                || (matches!(f.source, OsmId::Node(_))
                    && buildings
                        .locate_in_envelope_intersecting(&super::geometry::envelope(&f.geometry))
                        .any(|e| input.features[e.index].geometry.intersects(&f.geometry)))
        })
        .collect();
    let mut keep = keep.into_iter();
    input.features.retain(|_| keep.next().unwrap());
    Ok(input)
}
