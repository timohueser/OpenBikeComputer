//! The overlay index: route networks and access restrictions of the source OSM objects, in the
//! SQLite layout of `specs/planner-release.md`.
mod access;

use crate::source::{Data, Id, Relation, Tags};
use rusqlite::{params, Connection, Transaction};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::Path,
};

pub const FILE: &str = "overlays.sqlite";

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
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
        "mtb" => Some("mtb"),
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

fn access_zoom(status: &str) -> Option<f64> {
    Some(match status {
        "" => return None,
        "push" => 15.0,
        "directional" => 14.0,
        "construction" | "conditional" => 10.0,
        _ => 13.0,
    })
}

fn layer_id(kind: &str) -> i64 {
    match kind {
        "cycling" => 0,
        "hiking" => 1,
        "mtb" => 3,
        _ => 2,
    }
}

type Memberships = BTreeMap<i64, Vec<i64>>;

fn memberships(relations: &BTreeMap<i64, Relation>) -> (Memberships, BTreeMap<i64, Value>) {
    let mut members = Memberships::new();
    let mut routes = BTreeMap::new();
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
            members.entry(id).or_default().push(relation.id);
        }
        routes.insert(relation.id, route);
    }
    (members, routes)
}

/// Writes a new index for the routing package whose manifest identity is `package`.
pub fn write(path: &Path, package: &str, bounds: [f64; 4], osm: &Data) -> Result<(), String> {
    let mut database = Connection::open(path).map_err(text)?;
    database
        .execute_batch("PRAGMA journal_mode=OFF; PRAGMA cache_size=-65536; PRAGMA user_version=2;")
        .map_err(text)?;
    let transaction = database.transaction().map_err(text)?;
    transaction
        .execute_batch(
            "CREATE TABLE metadata(package TEXT NOT NULL, coverage TEXT NOT NULL);
        CREATE TABLE routes(id INTEGER PRIMARY KEY, properties TEXT NOT NULL);
        CREATE TABLE geometries(id INTEGER PRIMARY KEY, way INTEGER NOT NULL, points INTEGER NOT NULL, coordinates BLOB NOT NULL);
        CREATE TABLE attributes(id INTEGER PRIMARY KEY, properties TEXT NOT NULL);
        CREATE TABLE features(id INTEGER PRIMARY KEY, kind TEXT NOT NULL,
            cycling_minzoom REAL, walking_minzoom REAL,
            geometry INTEGER NOT NULL, attributes INTEGER NOT NULL);
        CREATE VIRTUAL TABLE bounds USING rtree(id, west, east, south, north,
            facet_min, facet_max);",
        )
        .map_err(text)?;
    transaction
        .execute("INSERT INTO metadata VALUES (?,?)", params![package, serde_json::to_string(&bounds).map_err(text)?])
        .map_err(text)?;
    let (members, routes) = memberships(&osm.relations);
    for (id, properties) in &routes {
        transaction
            .execute("INSERT INTO routes VALUES (?,?)", params![id, serde_json::to_string(properties).map_err(text)?])
            .map_err(text)?;
    }
    let mut attributes = HashMap::new();
    for way in osm.ways.values() {
        let access = access::feature(way);
        let memberships = members.get(&way.id).map_or(&[][..], Vec::as_slice);
        if access.is_none() && memberships.is_empty() {
            continue;
        }
        let mut properties = Vec::new();
        if let Some(access) = access {
            properties.push(("access", access));
        }
        for activity in ["cycling", "hiking", "mtb"] {
            let mut selected: Vec<_> =
                memberships.iter().map(|id| &routes[id]).filter(|route| route["kind"] == activity).collect();
            selected.sort_by_key(|route| std::cmp::Reverse(route["rank"].as_u64().unwrap_or(0)));
            if let Some(first) = selected.first() {
                properties.push((
                    activity,
                    json!({ "way": way.id, "kind": activity,
                    "rank": first["rank"], "ref": first["ref"],
                    "routes": selected.iter().map(|route| &route["id"]).collect::<Vec<_>>() }),
                ));
            }
        }
        // A node outside the import bounds splits the way into separate lines.
        let mut run = Vec::new();
        let points = way.nodes.iter().map(|id| osm.nodes.get(id).map(|node| [node.point.lon, node.point.lat]));
        for point in points.chain(std::iter::once(None)) {
            if let Some(point) = point {
                run.push(point);
                continue;
            }
            if run.len() >= 2 {
                transaction
                    .execute(
                        "INSERT INTO geometries(way,points,coordinates) VALUES (?,?,?)",
                        params![way.id, run.len() as i64, encode_coordinates(&run)?],
                    )
                    .map_err(text)?;
                let geometry = transaction.last_insert_rowid();
                let coordinates: Vec<_> = run.iter().map(|p| p.map(|v| v as f64 * 1e-6)).collect();
                for (activity, properties) in &properties {
                    insert(&transaction, &mut attributes, geometry, &coordinates, activity, properties)?;
                }
            }
            run.clear();
        }
    }
    transaction.commit().map_err(text)?;
    database.close().map_err(|(_, error)| text(error))
}

fn encode_coordinates(coordinates: &[[i32; 2]]) -> Result<Vec<u8>, String> {
    let mut previous = [0i32; 2];
    let deltas: Vec<_> = coordinates
        .iter()
        .map(|point| {
            let delta = [
                point[0].checked_sub(previous[0]).ok_or("Invalid overlay coordinate")?,
                point[1].checked_sub(previous[1]).ok_or("Invalid overlay coordinate")?,
            ];
            previous = *point;
            Ok(delta)
        })
        .collect::<Result<_, String>>()?;
    postcard::to_allocvec(&deltas).map_err(text)
}

fn insert(
    database: &Transaction<'_>,
    attributes: &mut HashMap<String, i64>,
    geometry: i64,
    coordinates: &[[f64; 2]],
    kind: &str,
    properties: &Value,
) -> Result<(), String> {
    let bounds = coordinates.iter().fold([180.0f64, 90.0f64, -180.0f64, -90.0f64], |b, p| {
        [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
    });
    let (cycling_minzoom, walking_minzoom) = if kind == "access" {
        (
            access_zoom(properties["cycling_status"].as_str().unwrap_or("")),
            access_zoom(properties["walking_status"].as_str().unwrap_or("")),
        )
    } else {
        let minimum = match properties["rank"].as_u64().unwrap_or(0) {
            3..=4 => 6.0,
            2 => 8.0,
            _ => 11.0,
        };
        (Some(minimum), Some(minimum))
    };
    let mut shared = properties.as_object().ok_or("Invalid overlay properties")?.clone();
    shared.remove("way");
    shared.remove("kind");
    let encoded = serde_json::to_string(&shared).map_err(text)?;
    let attribute = if let Some(id) = attributes.get(&encoded) {
        *id
    } else {
        database.execute("INSERT INTO attributes(properties) VALUES (?)", [&encoded]).map_err(text)?;
        let id = database.last_insert_rowid();
        attributes.insert(encoded, id);
        id
    };
    database
        .execute(
            "INSERT INTO features(kind,cycling_minzoom,walking_minzoom,geometry,attributes) VALUES (?,?,?,?,?)",
            params![kind, cycling_minzoom, walking_minzoom, geometry, attribute],
        )
        .map_err(text)?;
    let facet = layer_id(kind) as f64 * 32.0
        + cycling_minzoom.into_iter().chain(walking_minzoom).reduce(f64::min).unwrap_or(23.0);
    database
        .execute(
            "INSERT INTO bounds VALUES (?,?,?,?,?,?,?)",
            params![database.last_insert_rowid(), bounds[0], bounds[2], bounds[1], bounds[3], facet, facet],
        )
        .map_err(text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> Tags {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn packed_geometry_starts_absolute_continues_in_deltas_and_rejects_overflow() {
        let points = [[180_000_000, 90_000_000], [-180_000_000, -90_000_000], [7_812_349, 48_004_567]];
        let deltas: Vec<[i32; 2]> = postcard::from_bytes(&encode_coordinates(&points).unwrap()).unwrap();
        assert_eq!(deltas, [[180_000_000, 90_000_000], [-360_000_000, -180_000_000], [187_812_349, 138_004_567]]);
        assert!(encode_coordinates(&[[i32::MIN, 0], [i32::MAX, 0]]).is_err());
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
        let (members, routes) = memberships(&relations);
        assert_eq!(
            members[&10].iter().map(|id| routes[id]["network"].as_str().unwrap()).collect::<Vec<_>>(),
            ["icn", "rcn"]
        );
        assert!(!members.contains_key(&20));
    }
}
