//! Viewport overlays from the same immutable OSM snapshot as routing.
use route_engine::{
    osm::{Id, Node, Relation, Tags, Way},
    package::{Package, Source},
    Error, Result,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

struct Feature {
    coordinates: Vec<[f64; 2]>,
    properties: Value,
    bounds: [f64; 4],
    kind: &'static str,
    minzoom: f64,
}

#[derive(Default)]
pub struct Overlays {
    features: Vec<Feature>,
    cells: HashMap<(i32, i32), Vec<usize>>,
    coverage: [f64; 4],
}

fn tag<'a>(tags: &'a Tags, key: &str) -> &'a str {
    tags.get(key).map(String::as_str).unwrap_or("")
}

fn active(tags: &Tags) -> bool {
    !matches!(tag(tags, "state"), "proposed" | "planned" | "construction" | "disused" | "abandoned")
}

fn kind(tags: &Tags) -> Option<&'static str> {
    match tag(tags, "route") {
        "bicycle" => Some("cycling"),
        "hiking" | "foot" => Some("hiking"),
        _ => None,
    }
}

fn rank(network: &str) -> u8 {
    match network {
        "icn" | "iwn" => 4,
        "ncn" | "nwn" => 3,
        "rcn" | "rwn" => 2,
        "lcn" | "lwn" => 1,
        _ => 0,
    }
}

fn memberships(relations: &BTreeMap<i64, Relation>) -> BTreeMap<i64, Vec<Value>> {
    let mut members = BTreeMap::<i64, Vec<Value>>::new();
    for relation in relations.values().filter(|r| active(&r.tags)) {
        let Some(activity) = kind(&relation.tags) else { continue };
        if !matches!(tag(&relation.tags, "type"), "route" | "superroute") {
            continue;
        }
        let route = json!({ "id": relation.id, "kind": activity, "network": tag(&relation.tags, "network"),
            "rank": rank(tag(&relation.tags, "network")), "name": tag(&relation.tags, "name"), "ref": tag(&relation.tags, "ref"),
            "website": relation.tags.get("website").or_else(|| relation.tags.get("contact:website")),
            "symbol": tag(&relation.tags, "osmc:symbol"), "symbol_text": tag(&relation.tags, "symbol") });
        let mut pending = vec![relation.id];
        let mut visited = HashSet::new();
        let mut ways = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            let Some(child) = relations.get(&id) else { continue };
            if !active(&child.tags) || kind(&child.tags).is_some_and(|k| k != activity) {
                continue;
            }
            for (member, _) in &child.members {
                match member {
                    Id::Way(id) => {
                        ways.insert(*id);
                    }
                    Id::Relation(id) => pending.push(*id),
                    _ => {}
                }
            }
        }
        for id in ways {
            members.entry(id).or_default().push(route.clone());
        }
    }
    members
}

impl Overlays {
    pub fn load<S: Source>(package: &Package<S>) -> Result<Self> {
        let mut relations = BTreeMap::new();
        for key in &package.manifest().osm.relations {
            for relation in package.read::<Vec<Relation>>(key)? {
                relations.insert(relation.id, relation);
            }
        }
        let members = memberships(&relations);
        drop(relations);
        let mut ways = Vec::new();
        let mut needed = HashSet::new();
        for key in &package.manifest().osm.ways {
            for way in package.read::<Vec<Way>>(key)? {
                let mut properties = Vec::new();
                if let Some(access) = crate::access::feature(&way) {
                    properties.push(("access", access));
                }
                if let Some(routes) = members.get(&way.id) {
                    for activity in ["cycling", "hiking"] {
                        let mut routes: Vec<_> = routes.iter().filter(|r| r["kind"] == activity).cloned().collect();
                        routes.sort_by_key(|r| std::cmp::Reverse(r["rank"].as_u64().unwrap_or(0)));
                        if let Some(first) = routes.first() {
                            properties.push((
                                activity,
                                json!({ "way": way.id, "kind": activity,
                                "rank": first["rank"], "ref": first["ref"],
                                "routes": routes }),
                            ));
                        }
                    }
                }
                if !properties.is_empty() {
                    needed.extend(way.nodes.iter().copied());
                    ways.push((way, properties));
                }
            }
        }
        let mut nodes = HashMap::new();
        for key in &package.manifest().osm.nodes {
            for node in package.read::<Vec<Node>>(key)? {
                if needed.contains(&node.id) {
                    nodes.insert(node.id, [node.point.lon as f64 * 1e-6, node.point.lat as f64 * 1e-6]);
                }
            }
        }
        let mut result = Self { coverage: package.manifest().bounds, ..Self::default() };
        for (way, properties) in ways {
            let mut run = Vec::new();
            for point in way.nodes.iter().map(|id| nodes.get(id)).chain(std::iter::once(None)) {
                if let Some(point) = point {
                    run.push(*point);
                    continue;
                }
                if run.len() >= 2 {
                    for (activity, properties) in &properties {
                        result.insert(run.clone(), activity, properties.clone());
                    }
                }
                run.clear();
            }
        }
        Ok(result)
    }

    fn insert(&mut self, coordinates: Vec<[f64; 2]>, kind: &'static str, properties: Value) {
        let bounds = coordinates.iter().fold([180.0f64, 90.0f64, -180.0f64, -90.0f64], |b, p| {
            [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
        });
        let minzoom = if kind == "access" {
            10.0
        } else {
            match properties["rank"].as_u64().unwrap_or(0) {
                3..=4 => 6.0,
                2 => 8.0,
                _ => 10.0,
            }
        };
        for cell in cells(bounds) {
            self.cells.entry(cell).or_default().push(self.features.len());
        }
        self.features.push(Feature { coordinates, properties, bounds, kind, minzoom });
    }

    pub fn query(&self, params: &HashMap<String, String>) -> Result<Value> {
        let invalid = || {
            Error::InvalidRequest(
                "Provide bbox=west,south,east,north, zoom=6..22 and layers=cycling,hiking,access".into(),
            )
        };
        let bounds: [f64; 4] = params
            .get("bbox")
            .ok_or_else(invalid)?
            .split(',')
            .map(str::parse)
            .collect::<std::result::Result<Vec<f64>, _>>()
            .map_err(|_| invalid())?
            .try_into()
            .map_err(|_| invalid())?;
        let zoom: f64 = params.get("zoom").ok_or_else(invalid)?.parse().map_err(|_| invalid())?;
        let layers: Vec<_> = params.get("layers").ok_or_else(invalid)?.split(',').collect();
        if !zoom.is_finite()
            || !(6.0..=22.0).contains(&zoom)
            || bounds.iter().any(|v| !v.is_finite())
            || bounds[0] < -180.0
            || bounds[2] > 180.0
            || bounds[1] < -90.0
            || bounds[3] > 90.0
            || bounds[0] >= bounds[2]
            || bounds[1] >= bounds[3]
            || layers.iter().any(|l| !matches!(*l, "cycling" | "hiking" | "access"))
        {
            return Err(invalid());
        }
        if bounds[2] - bounds[0] > 30.0 || bounds[3] - bounds[1] > 30.0 {
            return Err(Error::InvalidRequest("Zoom in to see route networks and access restrictions.".into()));
        }
        let mode = params.get("mode").map(String::as_str).unwrap_or("cycling");
        if !matches!(mode, "cycling" | "walking") {
            return Err(invalid());
        }
        let mut ids = BTreeSet::<usize>::new();
        if intersects(bounds, self.coverage) {
            let [w, s, e, n] = self.coverage;
            for cell in cells([bounds[0].max(w), bounds[1].max(s), bounds[2].min(e), bounds[3].min(n)]) {
                if let Some(found) = self.cells.get(&cell) {
                    ids.extend(found.iter().copied());
                }
            }
        }
        let mut features = Vec::new();
        let mut points = 0;
        for id in ids {
            let f = &self.features[id];
            if zoom < f.minzoom || !layers.contains(&f.kind) || !intersects(bounds, f.bounds) {
                continue;
            }
            let mut properties = f.properties.clone();
            if f.kind == "access" {
                let status = f.properties[format!("{mode}_status")].as_str().unwrap_or("");
                let minimum = match status {
                    "push" => 15.0,
                    "directional" => 14.0,
                    "construction" | "conditional" => 10.0,
                    _ => 13.0,
                };
                if status.is_empty() || zoom < minimum {
                    continue;
                }
                properties["status"] = json!(status);
            }
            points += f.coordinates.len();
            // Bound serialization and browser work independently of the route workers.
            if points > 200_000 {
                return Err(Error::InvalidRequest("Zoom in to show route networks and access restrictions.".into()));
            }
            features.push(json!({ "type": "Feature", "id": id, "properties": properties,
                "geometry": { "type": "LineString", "coordinates": f.coordinates } }));
        }
        Ok(json!({ "type": "FeatureCollection", "features": features, "coverage": self.coverage }))
    }
}

fn intersects(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && a[2] >= b[0] && a[1] <= b[3] && a[3] >= b[1]
}

fn cells(b: [f64; 4]) -> impl Iterator<Item = (i32, i32)> {
    let cell = |v: f64| (v * 20.0).floor() as i32;
    (cell(b[0])..=cell(b[2])).flat_map(move |x| (cell(b[1])..=cell(b[3])).map(move |y| (x, y)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> Tags {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn nested_routes_keep_overlaps_but_not_proposed_sections_or_cycles() {
        let relation = |id, network, members: Vec<Id>, state| Relation {
            id,
            tags: tags(&[("type", "route"), ("route", "bicycle"), ("network", network), ("state", state)]),
            members: members.into_iter().map(|id| (id, String::new())).collect(),
        };
        let relations = [
            relation(1, "icn", vec![Id::Relation(2), Id::Relation(3)], ""),
            relation(2, "rcn", vec![Id::Way(10), Id::Relation(1)], ""),
            relation(3, "lcn", vec![Id::Way(20)], "proposed"),
        ]
        .into_iter()
        .map(|r| (r.id, r))
        .collect();
        let members = memberships(&relations);
        assert_eq!(members[&10].iter().map(|r| r["network"].as_str().unwrap()).collect::<Vec<_>>(), ["icn", "rcn"]);
        assert!(!members.contains_key(&20));
    }
}
