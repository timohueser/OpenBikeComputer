//! Viewport queries of the overlay index that `route-build` writes with the routing package.
use route_engine::{Error, Result};
use rusqlite::{params, Connection, OpenFlags};
use serde_json::{json, Value};
use std::{collections::HashMap, path::Path, sync::Mutex};

pub struct Overlays {
    package: String,
    database: Mutex<Connection>,
    coverage: [f64; 4],
}

fn invalid_data(error: impl std::fmt::Display) -> Error {
    Error::InvalidData(error.to_string())
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

pub(crate) fn query_window(params: &HashMap<String, String>) -> Result<([f64; 4], f64, Vec<&str>, &str)> {
    let invalid = || {
        Error::InvalidRequest(
            "Provide bbox=west,south,east,north, zoom=6..22 and layers=cycling,hiking,mtb,access".into(),
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
        || layers.iter().any(|l| !matches!(*l, "cycling" | "hiking" | "mtb" | "access"))
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
    Ok((bounds, zoom, layers, mode))
}

impl Overlays {
    pub(crate) fn coverage(&self) -> [f64; 4] {
        self.coverage
    }
    pub fn open(directory: &Path, identity: &str) -> Result<Self> {
        Self::open_file(&directory.join("overlays.sqlite"), identity)
    }

    pub fn open_file(path: &Path, identity: &str) -> Result<Self> {
        let database = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(invalid_data)?;
        let version: i64 = database.pragma_query_value(None, "user_version", |row| row.get(0)).map_err(invalid_data)?;
        if version != 2 {
            return Err(Error::InvalidData("Rebuild the overlay index with route-build".into()));
        }
        database
            .prepare(
                "SELECT b.facet_min,g.way,a.properties FROM bounds b
            JOIN features f ON f.id=b.id JOIN geometries g ON g.id=f.geometry
            JOIN attributes a ON a.id=f.attributes LIMIT 0",
            )
            .map_err(invalid_data)?;
        database.execute_batch("PRAGMA cache_size=-4096; PRAGMA mmap_size=0;").map_err(invalid_data)?;
        let (package, coverage): (String, String) = database
            .query_row("SELECT package,coverage FROM metadata", [], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(invalid_data)?;
        if package != identity {
            return Err(Error::InvalidData("Overlay index uses another routing package".into()));
        }
        Ok(Self {
            package,
            database: Mutex::new(database),
            coverage: serde_json::from_str(&coverage).map_err(invalid_data)?,
        })
    }

    pub fn query(&self, params: &HashMap<String, String>, cancelled: &dyn Fn() -> bool) -> Result<Vec<u8>> {
        let (bounds, zoom, layers, mode) = query_window(params)?;
        if bounds[0] > self.coverage[2]
            || bounds[2] < self.coverage[0]
            || bounds[1] > self.coverage[3]
            || bounds[3] < self.coverage[1]
        {
            return serde_json::to_vec(
                &json!({ "type": "FeatureCollection", "features": [], "coverage": self.coverage, "package": self.package }),
            )
            .map_err(invalid_data);
        }
        let database = self.database.lock().map_err(|_| Error::Limit)?;
        if cancelled() {
            return Err(Error::Cancelled);
        }
        // Filter each mode in SQLite before reading and decoding its coordinates.
        let mut statement = database
            .prepare_cached(
                "WITH selected AS MATERIALIZED (
                SELECT id FROM bounds
                WHERE west<=?1 AND east>=?2 AND south<=?3 AND north>=?4
                    AND facet_min>=?11 AND facet_max<=?12
                ORDER BY id)
            SELECT f.id,f.kind,g.coordinates,a.properties,g.way
            FROM selected b CROSS JOIN features f ON f.id=b.id
                JOIN geometries g ON g.id=f.geometry JOIN attributes a ON a.id=f.attributes
            WHERE (CASE ?10 WHEN 'cycling' THEN f.cycling_minzoom ELSE f.walking_minzoom END)<=?5
                AND f.kind IN (?6,?7,?8,?9)",
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
                selected("mtb"),
                selected("access"),
                mode,
                layers.iter().map(|layer| layer_id(layer) * 32).min(),
                layers.iter().map(|layer| layer_id(layer) * 32).max().map(|id| id as f64 + zoom)
            ])
            .map_err(invalid_data)?;
        // Retain encoded output and one feature; large viewports must not retain a JSON tree.
        let mut encoded = br#"{"coverage":"#.to_vec();
        serde_json::to_writer(&mut encoded, &self.coverage).map_err(invalid_data)?;
        encoded.extend_from_slice(br#","features":["#);
        let mut first = true;
        let mut routes = serde_json::Map::new();
        let mut route = database.prepare_cached("SELECT properties FROM routes WHERE id=?").map_err(invalid_data)?;
        let mut points = 0;
        while let Some(row) = rows.next().map_err(invalid_data)? {
            if cancelled() {
                return Err(Error::Cancelled);
            }
            let id: i64 = row.get(0).map_err(invalid_data)?;
            let kind: String = row.get(1).map_err(invalid_data)?;
            let coordinates: Vec<u8> = row.get(2).map_err(invalid_data)?;
            let mut properties: Value =
                serde_json::from_str(&row.get::<_, String>(3).map_err(invalid_data)?).map_err(invalid_data)?;
            properties["kind"] = json!(kind);
            properties["way"] = json!(row.get::<_, i64>(4).map_err(invalid_data)?);
            if kind == "access" {
                let status = properties[format!("{mode}_status")].as_str().unwrap_or("");
                if !access_zoom(status).is_some_and(|minimum| zoom >= minimum) {
                    continue;
                }
                properties["status"] = json!(status);
            } else if let Some(memberships) = properties["routes"].as_array() {
                for id in memberships {
                    if let serde_json::map::Entry::Vacant(entry) = routes.entry(id.to_string()) {
                        let encoded: String = route
                            .query_row([id.as_i64().ok_or_else(|| invalid_data("Invalid route ID"))?], |row| row.get(0))
                            .map_err(invalid_data)?;
                        entry.insert(serde_json::from_str(&encoded).map_err(invalid_data)?);
                    }
                }
            }
            let coordinates = decode_coordinates(&coordinates)?;
            let coordinates = simplify(&coordinates, zoom);
            points += coordinates.len();
            // Bound serialization and browser work independently of the route workers.
            if points > 200_000 {
                return Err(Error::InvalidRequest("Zoom in to show route networks and access restrictions.".into()));
            }
            if !first {
                encoded.push(b',');
            }
            first = false;
            serde_json::to_writer(
                &mut encoded,
                &json!({ "type": "Feature", "id": id, "properties": properties,
                "geometry": { "type": "LineString", "coordinates": coordinates } }),
            )
            .map_err(invalid_data)?;
        }
        encoded.extend_from_slice(br#"],"package":"#);
        serde_json::to_writer(&mut encoded, &self.package).map_err(invalid_data)?;
        encoded.extend_from_slice(br#","routes":"#);
        serde_json::to_writer(&mut encoded, &routes).map_err(invalid_data)?;
        encoded.extend_from_slice(br#","type":"FeatureCollection"}"#);
        Ok(encoded)
    }
}

// Half a map pixel at the requested zoom; keep way endpoints and sharp bends.
fn simplify(coordinates: &[[f64; 2]], zoom: f64) -> Vec<[f64; 2]> {
    if coordinates.len() < 2 {
        return coordinates.to_vec();
    }
    let scale = coordinates[0][1].to_radians().cos();
    let tolerance = (360.0 / (512.0 * 2.0_f64.powf(zoom)) * 0.5 * scale).powi(2);
    let mut keep = vec![false; coordinates.len()];
    keep[0] = true;
    keep[coordinates.len() - 1] = true;
    let mut pending = vec![(0, coordinates.len() - 1)];
    while let Some((first, last)) = pending.pop() {
        let a = coordinates[first];
        let b = coordinates[last];
        let dx = (b[0] - a[0]) * scale;
        let dy = b[1] - a[1];
        let mut furthest = None;
        let mut distance = tolerance;
        for (i, p) in coordinates.iter().enumerate().take(last).skip(first + 1) {
            let x = (p[0] - a[0]) * scale;
            let y = p[1] - a[1];
            let t = ((x * dx + y * dy) / (dx * dx + dy * dy).max(f64::MIN_POSITIVE)).clamp(0.0, 1.0);
            let d = (x - dx * t).powi(2) + (y - dy * t).powi(2);
            if d > distance {
                distance = d;
                furthest = Some(i);
            }
        }
        if let Some(i) = furthest {
            keep[i] = true;
            pending.push((first, i));
            pending.push((i, last));
        }
    }
    coordinates.iter().zip(keep).filter(|(_, keep)| *keep).map(|(p, _)| p.map(|v| (v * 1e6).round() / 1e6)).collect()
}

fn decode_coordinates(bytes: &[u8]) -> Result<Vec<[f64; 2]>> {
    let (deltas, trailing): (Vec<[i32; 2]>, _) = postcard::take_from_bytes(bytes).map_err(invalid_data)?;
    if !trailing.is_empty() {
        return Err(invalid_data("Trailing overlay coordinate data"));
    }
    let mut previous = [0i32; 2];
    deltas
        .into_iter()
        .map(|delta| {
            for axis in 0..2 {
                previous[axis] = previous[axis]
                    .checked_add(delta[axis])
                    .ok_or_else(|| invalid_data("Invalid overlay coordinate"))?;
            }
            if !(-180_000_000..=180_000_000).contains(&previous[0])
                || !(-90_000_000..=90_000_000).contains(&previous[1])
            {
                return Err(invalid_data("Overlay coordinate outside world bounds"));
            }
            Ok(previous.map(|v| v as f64 * 1e-6))
        })
        .collect()
}

fn layer_id(kind: &str) -> i64 {
    match kind {
        "cycling" => 0,
        "hiking" => 1,
        "mtb" => 3,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_detail_tracks_zoom_and_keeps_endpoints_and_bends() {
        let coordinates = [[8.0, 48.0], [8.0005, 48.00001], [8.001, 48.0], [8.001, 48.001], [8.002, 48.001]];
        assert_eq!(simplify(&coordinates, 12.0), [coordinates[0], coordinates[2], coordinates[3], coordinates[4]]);
        assert_eq!(simplify(&coordinates, 22.0), coordinates);
        assert_eq!(simplify(&coordinates, 6.0), [coordinates[0], coordinates[4]]);
    }

    #[test]
    fn packed_geometry_preserves_source_precision_and_rejects_invalid_deltas() {
        let points = [[180_000_000, 90_000_000], [-180_000_000, -90_000_000], [7_812_349, 48_004_567], [0, 0]];
        let deltas = [
            [180_000_000, 90_000_000],
            [-360_000_000, -180_000_000],
            [187_812_349, 138_004_567],
            [-7_812_349, -48_004_567],
        ];
        let encoded = postcard::to_allocvec(&deltas[..]).unwrap();
        let expected: Vec<_> = points.iter().map(|p| p.map(|v| (v as f64 * 1e-6).to_bits())).collect();
        assert_eq!(
            decode_coordinates(&encoded).unwrap().iter().map(|p| p.map(f64::to_bits)).collect::<Vec<_>>(),
            expected
        );
        let invalid = postcard::to_allocvec(&vec![[i32::MAX, 0]]).unwrap();
        assert!(decode_coordinates(&invalid).is_err());
        assert!(decode_coordinates(&[encoded, vec![0]].concat()).is_err());
    }
}
