//! Viewport overlays from the same immutable OSM snapshot as routing.
use route_engine::{
    directory::Directory,
    osm::{Id, Node, Relation, Tags, Way},
    Error, Result,
};
use rusqlite::{params, Connection, OpenFlags, Transaction};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::Path,
    sync::Mutex,
};

pub struct Overlays {
    database: Mutex<Connection>,
    coverage: [f64; 4],
}

fn invalid_data(error: impl std::fmt::Display) -> Error {
    Error::InvalidData(error.to_string())
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

impl Overlays {
    pub fn open(directory: &Path, identity: &str) -> Result<Self> {
        let database = Connection::open_with_flags(directory.join("overlays.sqlite"), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(invalid_data)?;
        database.execute_batch("PRAGMA cache_size=-4096; PRAGMA mmap_size=0;").map_err(invalid_data)?;
        let (package, coverage): (String, String) = database
            .query_row("SELECT package,coverage FROM metadata", [], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(invalid_data)?;
        if package != identity {
            return Err(Error::InvalidData("Overlay index uses another routing package".into()));
        }
        Ok(Self { database: Mutex::new(database), coverage: serde_json::from_str(&coverage).map_err(invalid_data)? })
    }

    pub fn build(directory: &Path) -> Result<()> {
        let package = Directory::open(directory)?;
        let output = directory.join("overlays.sqlite");
        if output.exists() {
            Self::open(directory, package.identity())?;
            return Ok(());
        }
        let partial = directory.join(".overlays.sqlite.partial");
        std::fs::OpenOptions::new().write(true).create_new(true).open(&partial).map_err(invalid_data)?;
        let result = (|| {
            let mut database = Connection::open(&partial).map_err(invalid_data)?;
            database.execute_batch("PRAGMA journal_mode=OFF; PRAGMA cache_size=-65536;").map_err(invalid_data)?;
            let transaction = database.transaction().map_err(invalid_data)?;
            transaction
                .execute_batch(
                    "CREATE TABLE metadata(package TEXT NOT NULL, coverage TEXT NOT NULL);
                CREATE TABLE features(id INTEGER PRIMARY KEY, kind TEXT NOT NULL, minzoom REAL NOT NULL,
                    points INTEGER NOT NULL, coordinates TEXT NOT NULL, properties TEXT NOT NULL);
                CREATE VIRTUAL TABLE bounds USING rtree(id, west, east, south, north);",
                )
                .map_err(invalid_data)?;
            transaction
                .execute(
                    "INSERT INTO metadata VALUES (?,?)",
                    params![
                        package.identity(),
                        serde_json::to_string(&package.manifest().bounds).map_err(invalid_data)?
                    ],
                )
                .map_err(invalid_data)?;
            let mut relations = BTreeMap::new();
            for key in package.keys(&package.manifest().osm.relations)? {
                for relation in package.read::<Vec<Relation>>(&key)? {
                    relations.insert(relation.id, relation);
                }
            }
            let (members, routes) = memberships(&relations);
            drop(relations);
            let mut ways = Vec::new();
            let mut needed = HashSet::new();
            for key in package.keys(&package.manifest().osm.ways)? {
                for way in package.read::<Vec<Way>>(&key)? {
                    let access = crate::access::feature(&way);
                    let memberships = members.get(&way.id).cloned().unwrap_or_default();
                    if access.is_some() || !memberships.is_empty() {
                        needed.extend(way.nodes.iter().copied());
                        ways.push((way.id, way.nodes, access, memberships));
                    }
                }
            }
            drop(members);
            let mut nodes = HashMap::new();
            for key in package.keys(&package.manifest().osm.nodes)? {
                for node in package.read::<Vec<Node>>(&key)? {
                    if needed.contains(&node.id) {
                        nodes.insert(node.id, [node.point.lon as f64 * 1e-6, node.point.lat as f64 * 1e-6]);
                    }
                }
            }
            drop(needed);
            for (way, ids, access, memberships) in ways {
                let mut properties = Vec::new();
                if let Some(access) = access {
                    properties.push(("access", access));
                }
                for activity in ["cycling", "hiking"] {
                    let mut selected: Vec<_> =
                        memberships.iter().map(|id| &routes[id]).filter(|route| route["kind"] == activity).collect();
                    selected.sort_by_key(|route| std::cmp::Reverse(route["rank"].as_u64().unwrap_or(0)));
                    if let Some(first) = selected.first() {
                        properties.push((
                            activity,
                            json!({ "way": way, "kind": activity,
                            "rank": first["rank"], "ref": first["ref"], "routes": selected }),
                        ));
                    }
                }
                let mut run = Vec::new();
                for point in ids.iter().map(|id| nodes.get(id)).chain(std::iter::once(None)) {
                    if let Some(point) = point {
                        run.push(*point);
                        continue;
                    }
                    if run.len() >= 2 {
                        for (activity, properties) in &properties {
                            insert(&transaction, &run, activity, properties)?;
                        }
                    }
                    run.clear();
                }
            }
            transaction.commit().map_err(invalid_data)?;
            database.close().map_err(|(_, error)| invalid_data(error))?;
            std::fs::rename(&partial, output).map_err(invalid_data)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(partial);
        }
        result
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
        if bounds[0] > self.coverage[2]
            || bounds[2] < self.coverage[0]
            || bounds[1] > self.coverage[3]
            || bounds[3] < self.coverage[1]
        {
            return Ok(json!({ "type": "FeatureCollection", "features": [], "coverage": self.coverage }));
        }
        let database = self.database.lock().map_err(|_| Error::Limit)?;
        let mut statement = database
            .prepare_cached(
                "SELECT f.id,f.kind,f.coordinates,f.properties,f.points
            FROM bounds b JOIN features f ON f.id=b.id
            WHERE b.west<=?1 AND b.east>=?2 AND b.south<=?3 AND b.north>=?4
                AND f.minzoom<=?5 AND f.kind IN (?6,?7,?8) ORDER BY f.id",
            )
            .map_err(invalid_data)?;
        let selected = |kind| if layers.contains(&kind) { kind } else { "" };
        let mut rows = statement
            .query(params![
                bounds[2],
                bounds[0],
                bounds[3],
                bounds[1],
                zoom,
                selected("cycling"),
                selected("hiking"),
                selected("access")
            ])
            .map_err(invalid_data)?;
        let mut features = Vec::new();
        let mut points = 0;
        while let Some(row) = rows.next().map_err(invalid_data)? {
            let id: i64 = row.get(0).map_err(invalid_data)?;
            let kind: String = row.get(1).map_err(invalid_data)?;
            let coordinates: String = row.get(2).map_err(invalid_data)?;
            let mut properties: Value =
                serde_json::from_str(&row.get::<_, String>(3).map_err(invalid_data)?).map_err(invalid_data)?;
            if kind == "access" {
                let status = properties[format!("{mode}_status")].as_str().unwrap_or("");
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
            points += row.get::<_, u32>(4).map_err(invalid_data)?;
            // Bound serialization and browser work independently of the route workers.
            if points > 200_000 {
                return Err(Error::InvalidRequest("Zoom in to show route networks and access restrictions.".into()));
            }
            features.push(json!({ "type": "Feature", "id": id, "properties": properties,
                "geometry": { "type": "LineString", "coordinates": serde_json::from_str::<Value>(&coordinates).map_err(invalid_data)? } }));
        }
        Ok(json!({ "type": "FeatureCollection", "features": features, "coverage": self.coverage }))
    }
}

fn insert(database: &Transaction<'_>, coordinates: &[[f64; 2]], kind: &str, properties: &Value) -> Result<()> {
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
    database
        .execute(
            "INSERT INTO features(kind,minzoom,points,coordinates,properties) VALUES (?,?,?,?,?)",
            params![
                kind,
                minzoom,
                coordinates.len() as i64,
                serde_json::to_string(coordinates).map_err(invalid_data)?,
                serde_json::to_string(properties).map_err(invalid_data)?
            ],
        )
        .map_err(invalid_data)?;
    database
        .execute(
            "INSERT INTO bounds VALUES (?,?,?,?,?)",
            params![database.last_insert_rowid(), bounds[0], bounds[2], bounds[1], bounds[3]],
        )
        .map_err(invalid_data)?;
    Ok(())
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
        let (members, routes) = memberships(&relations);
        assert_eq!(
            members[&10].iter().map(|id| routes[id]["network"].as_str().unwrap()).collect::<Vec<_>>(),
            ["icn", "rcn"]
        );
        assert!(!members.contains_key(&20));
    }
}
