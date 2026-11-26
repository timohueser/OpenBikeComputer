//! The stretches of the ways of the routing bake, with their steepness and trail grades:
//! `ways.sqlite` of `specs/planner-release.md`.

use crate::column::Column;
use crate::{grid, overlays, sort, Graph};
use planner_router::model::{Point, NO_ELEVATION};
use rusqlite::{params, Connection};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

pub const FILE: &str = "ways.sqlite";

/// `gradeThresholds` of `builder/web/src/lib/planner/grade-data.ts`, in %.
const THRESHOLDS: [f64; 5] = [3.0, 6.0, 10.0, 15.0, 20.0];
/// The buffer of a tile of zoom 12, 1/64 of the tile, in tiles of the block zoom 9.
const BUFFER: f64 = 1.0 / 64.0 / 8.0;
const FERRY: u8 = 6;

#[derive(Debug, Default)]
pub struct Report {
    pub ways: u64,
    pub stretches: u64,
    pub length_m: f64,
    /// The length of the pieces without a steepness class.
    pub unclassified_m: f64,
}

/// The steepness in % of each piece between two points, as `profileGrades` of
/// `builder/web/src/lib/planner/grade-data.ts` computes it, with the same arithmetic.
fn grades(distance: &[f64], height: &[Option<f64>]) -> Vec<Option<f64>> {
    let mut grades = vec![None; distance.len().saturating_sub(1)];
    let mut start = 0;
    while start < distance.len() {
        if height[start].is_none() {
            start += 1;
            continue;
        }
        let mut end = start;
        while end + 1 < distance.len() && height[end + 1].is_some() {
            end += 1;
        }
        let h = |i: usize| height[i].expect("a height of the run");
        let at = |index: usize, at: f64| {
            let next = (index + 1).min(end);
            let length = distance[next] - distance[index];
            h(index) + if length != 0.0 { (at - distance[index]) / length } else { 0.0 } * (h(next) - h(index))
        };
        let (mut left, mut right) = (start, start);
        for i in start..end {
            if distance[i + 1] <= distance[i] {
                continue;
            }
            let middle = (distance[i] + distance[i + 1]) / 2.0;
            let (from, to) = (distance[start].max(middle - 50.0), distance[end].min(middle + 50.0));
            while left < end - 1 && distance[left + 1] <= from {
                left += 1;
            }
            while right < end - 1 && distance[right + 1] <= to {
                right += 1;
            }
            if to - from >= 20.0 {
                grades[i] = Some((at(right, to) - at(left, from)) / (to - from) * 100.0);
            }
        }
        start = end + 1;
    }
    grades
}

/// The steepness class of each piece: the count of `THRESHOLDS` that its steepness, rounded to
/// 0.1 %, reaches, in either direction.
fn slopes(distance: &[f64], height: &[Option<f64>]) -> Vec<Option<u8>> {
    let class = |grade: f64| {
        let magnitude = (grade.abs() * 10.0).round() / 10.0;
        THRESHOLDS.iter().filter(|&&limit| magnitude >= limit).count() as u8
    };
    grades(distance, height).into_iter().map(|grade| grade.filter(|g| g.is_finite()).map(class)).collect()
}

/// A road in the direction of the nodes of its way.
#[derive(Clone, Copy)]
struct Edge {
    from: u32,
    to: u32,
    road: usize,
    reversed: bool,
}

/// The roads of one way as chains along the way: one chain when they form one, else one chain for
/// each road. A road and the road of its other direction, with the same points in reverse order,
/// count once.
fn chains(graph: &Graph, roads: impl Iterator<Item = usize>) -> Vec<Vec<Edge>> {
    let mut seen = HashSet::new();
    let edges: Vec<Edge> = roads
        .filter_map(|road| {
            let (from, to, reversed) = (graph.roads.from(road), graph.roads.to(road), graph.roads.reversed(road));
            let (from, to) = if reversed { (to, from) } else { (from, to) };
            let mut points: Vec<[i32; 2]> = graph.roads.shape(road).iter().map(|p| [p.lon, p.lat]).collect();
            if reversed {
                points.reverse();
            }
            seen.insert(points).then_some(Edge { from, to, road, reversed })
        })
        .collect();
    let next: HashMap<u32, usize> = edges.iter().enumerate().map(|(i, edge)| (edge.from, i)).collect();
    let ends: HashSet<u32> = edges.iter().map(|edge| edge.to).collect();
    let mut starts = edges.iter().filter(|edge| !ends.contains(&edge.from));
    if let (Some(&first), None) = (starts.next(), starts.next()) {
        if next.len() == edges.len() && ends.len() == edges.len() {
            let mut chain = vec![first];
            while let Some(&i) = next.get(&chain[chain.len() - 1].to) {
                chain.push(edges[i]);
            }
            if chain.len() == edges.len() {
                return vec![chain];
            }
        }
    }
    edges.into_iter().map(|edge| vec![edge]).collect()
}

/// The blocks `x * 512 + y` of zoom 9 that each segment of `line` touches or comes nearer to than
/// `BUFFER`, by the box of the segment.
fn blocks(line: &[Point]) -> BTreeSet<i64> {
    let last = (1i64 << grid::ZOOM) - 1;
    let tile = |p: &Point| grid::mercator(f64::from(p.lon) * 1e-6, f64::from(p.lat) * 1e-6, grid::ZOOM);
    let range = |a: f64, b: f64| {
        ((a.min(b) - BUFFER).floor() as i64).clamp(0, last)..=((a.max(b) + BUFFER).floor() as i64).clamp(0, last)
    };
    let mut blocks = BTreeSet::new();
    for pair in line.windows(2) {
        let ((ax, ay), (bx, by)) = (tile(&pair[0]), tile(&pair[1]));
        for x in range(ax, bx) {
            blocks.extend(range(ay, by).map(|y| x * 512 + y));
        }
    }
    blocks
}

/// Writes a new `ways.sqlite` of the roads of `graph` at `path`.
pub fn write(path: &Path, graph: &Graph) -> Result<Report, String> {
    let text = |e: rusqlite::Error| e.to_string();
    let mut database = Connection::open(path).map_err(text)?;
    database.execute_batch("PRAGMA journal_mode=OFF; PRAGMA user_version=1;").map_err(text)?;
    let transaction = database.transaction().map_err(text)?;
    transaction
        .execute_batch(
            "CREATE TABLE stretches(block INTEGER NOT NULL, way INTEGER NOT NULL, minzoom INTEGER NOT NULL,
            slope INTEGER, mtb INTEGER, sac INTEGER, trail INTEGER NOT NULL, coordinates BLOB NOT NULL);",
        )
        .map_err(text)?;
    let mut insert = transaction.prepare("INSERT INTO stretches VALUES (?,?,?,?,?,?,?,?)").map_err(text)?;
    let mut report = Report::default();
    // The roads are in a spatial order: this groups them by way.
    let rows: Column<(u32, u32)> = (0..graph.roads.len()).map(|r| (graph.roads.way_row(r), r as u32)).collect();
    let mut rows = sort::sorted(rows).peekable();
    while let Some((row, first)) = rows.next() {
        let mut roads = vec![first as usize];
        while let Some((_, road)) = rows.next_if(|&(next, _)| next == row) {
            roads.push(road as usize);
        }
        let road = graph.roads.attributes(first as usize);
        if road.class == FERRY {
            continue;
        }
        let way = graph.osm.ways.row(row as usize);
        let highway = way.tags.get("highway").unwrap_or("");
        let minzoom = if highway == "service" { 13 } else { 12 };
        let trail = matches!(highway, "path" | "footway" | "bridleway" | "steps" | "track");
        let sac = road.hiking_difficulty.filter(|&grade| grade != 255);
        let before = report.stretches;
        for chain in chains(graph, roads.into_iter()) {
            let mut line: Vec<Point> = Vec::new();
            for edge in chain {
                let mut shape = graph.roads.shape(edge.road).to_vec();
                if edge.reversed {
                    shape.reverse();
                }
                line.extend(&shape[usize::from(!line.is_empty())..]);
            }
            let mut distance = vec![0.0];
            for pair in line.windows(2) {
                distance.push(distance[distance.len() - 1] + pair[0].distance(pair[1]));
            }
            let heights: Vec<_> =
                line.iter().map(|p| (p.elevation != NO_ELEVATION).then_some(f64::from(p.elevation))).collect();
            let slopes = slopes(&distance, &heights);
            report.length_m += distance[distance.len() - 1];
            for (i, slope) in slopes.iter().enumerate() {
                if slope.is_none() {
                    report.unclassified_m += distance[i + 1] - distance[i];
                }
            }
            let mut start = 0;
            for end in 1..=slopes.len() {
                if end < slopes.len() && slopes[end] == slopes[start] {
                    continue;
                }
                let stretch = &line[start..=end];
                let points: Vec<[i32; 2]> = stretch.iter().map(|p| [p.lon, p.lat]).collect();
                let coordinates = overlays::encode_coordinates(&points)?;
                for block in blocks(stretch) {
                    insert
                        .execute(params![
                            block,
                            road.way,
                            minzoom,
                            slopes[start],
                            road.mtb_scale(),
                            sac,
                            trail,
                            coordinates
                        ])
                        .map_err(text)?;
                }
                report.stretches += 1;
                start = end;
            }
        }
        report.ways += u64::from(report.stretches > before);
    }
    drop(insert);
    transaction.execute_batch("CREATE INDEX stretches_block ON stretches(block);").map_err(text)?;
    transaction.commit().map_err(text)?;
    database.close().map_err(|(_, error)| text(error))?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{Builder, Tags, Ways};
    use planner_router::model::{Road, BIKE, FOOT, PUSH};
    use serde_json::Value;

    #[test]
    fn the_classes_are_those_of_the_shared_cases_of_the_elevation_profile() {
        let cases: Value =
            serde_json::from_str(include_str!("../../../builder/web/src/lib/planner/grade-cases.json")).unwrap();
        for case in cases.as_array().unwrap() {
            let numbers = |key: &str| case[key].as_array().unwrap().iter().map(Value::as_f64).collect::<Vec<_>>();
            let distance: Vec<f64> = numbers("distance_m").into_iter().map(Option::unwrap).collect();
            let expected: Vec<Option<u8>> = numbers("slope").into_iter().map(|s| s.map(|s| s as u8)).collect();
            assert_eq!(slopes(&distance, &numbers("height_m")), expected, "{}", case["case"]);
        }
    }

    /// Points along 1° N from `lon` microdegrees, about 20 m apart, with the heights of `height` at
    /// the distance of each point from `lon`.
    fn line(lon: i32, count: usize, height: impl Fn(f64) -> f32) -> Vec<Point> {
        let step = (20.0 / planner_router::geometry::METRES_PER_UDEG).round() as i32;
        let start = Point { lat: 1_000_000, lon, elevation: 0.0 };
        let point = |lon| Point { lon, elevation: height(start.distance(Point { lon, ..start })), ..start };
        (0..count as i32).map(|i| point(lon + i * step)).collect()
    }

    fn road(way: i64, (from, to): (u32, u32), shape: Vec<Point>) -> Road {
        Road { from, to, way, class: 1, access: BIKE | FOOT | PUSH, difficulty: 255, shape, ..Road::default() }
    }

    /// The road and the road of its other direction.
    fn both(road: Road) -> [Road; 2] {
        let mut back = Road { from: road.to, to: road.from, reversed: true, ..road.clone() };
        back.shape.reverse();
        [road, back]
    }

    /// `block`, `way`, `minzoom`, `slope`, `mtb`, `sac`, `trail` and the points of `coordinates`.
    type Row = (i64, i64, i64, Value, Value, Value, i64, Vec<[i32; 2]>);

    /// The rows of `ways.sqlite` of a graph of `roads`, whose ways have the tags of `ways`.
    fn bake(ways: &[(i64, &[(&str, &str)])], roads: Vec<Road>) -> (Report, Vec<Row>) {
        let mut table = Builder::<Ways>::default();
        for (id, tags) in ways {
            table.push(*id, &[1, 2], &Tags::from_pairs(tags.iter().copied()));
        }
        let junctions = roads.iter().map(|road| road.from.max(road.to) as usize + 1).max().unwrap_or(0);
        let mut graph = Graph {
            points: Column::filled(junctions, Point::default()),
            roads: roads.into_iter().collect(),
            ..Graph::default()
        };
        graph.osm.ways = table.finish().0;
        graph.link_ways();
        let path = std::env::temp_dir().join(format!(
            "route-build-ways-{}-{:?}.sqlite",
            std::process::id(),
            std::thread::current().id()
        ));
        let report = write(&path, &graph).unwrap();
        let database = Connection::open(&path).unwrap();
        let null = |value: Option<i64>| value.map_or(Value::Null, Value::from);
        let rows = database
            .prepare("SELECT block, way, minzoom, slope, mtb, sac, trail, coordinates FROM stretches ORDER BY rowid")
            .unwrap()
            .query_map([], |row| {
                let deltas: Vec<[i32; 2]> = postcard::from_bytes(&row.get::<_, Vec<u8>>(7)?).unwrap();
                let points = deltas.iter().scan([0, 0], |at, d| {
                    *at = [at[0] + d[0], at[1] + d[1]];
                    Some(*at)
                });
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    null(row.get(3)?),
                    null(row.get(4)?),
                    null(row.get(5)?),
                    row.get(6)?,
                    points.collect(),
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let version: i64 = database.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap();
        assert_eq!(version, 1);
        drop(database);
        std::fs::remove_file(&path).unwrap();
        (report, rows)
    }

    #[test]
    fn the_window_crosses_the_junctions_of_a_way_and_each_direction_counts_once() {
        let climb = |d: f64| (0.11 * (d - 40.0).max(0.0)) as f32;
        let points = line(7_500_000, 7, climb);
        let shape = |range: std::ops::RangeInclusive<usize>| points[range].to_vec();
        let mut roads = Vec::new();
        for (ends, range) in [((2, 3), 4..=6), ((0, 1), 0..=2), ((1, 2), 2..=4)] {
            let [forward, back] = both(road(7, ends, shape(range)));
            // The reversed road comes first for the first road of the way.
            roads.extend(if ends == (0, 1) { [back, forward] } else { [forward, back] });
        }
        let (report, rows) = bake(&[(7, &[("highway", "residential")])], roads);
        let slopes: Vec<_> = rows.iter().map(|row| (row.3.clone(), row.7.len())).collect();
        // Each road alone would give the classes 0, 3 and 3.
        assert_eq!(slopes, [(1.into(), 3), (2.into(), 3), (3.into(), 3)]);
        assert_eq!(rows[0].7[0], [7_500_000, 1_000_000]);
        assert_eq!((report.ways, report.stretches, report.unclassified_m), (1, 3, 0.0));
    }

    #[test]
    fn the_roads_of_a_ring_stand_alone_and_a_ferry_has_no_stretch() {
        let flat = line(7_500_000, 3, |_| 0.0);
        // A second road from junction 0 to junction 1 on other points.
        let other = line(7_600_000, 3, |_| 0.0);
        let rise = line(flat[2].lon, 3, |d| (0.11 * d) as f32);
        let fall = line(rise[2].lon, 3, |d| (0.11 * (40.0 - d)) as f32);
        let ferry = Road { class: FERRY, ..road(8, (3, 4), line(7_500_000, 3, |d| (0.5 * d) as f32)) };
        let roads =
            vec![road(7, (0, 1), flat), road(7, (0, 1), other), road(7, (1, 2), rise), road(7, (2, 0), fall), ferry];
        let (report, rows) = bake(&[(7, &[("highway", "track")]), (8, &[("route", "ferry")])], roads);
        let slopes: Vec<_> = rows.iter().map(|row| (row.1, row.3.clone())).collect();
        assert_eq!(slopes, [(7, 0.into()), (7, 0.into()), (7, 3.into()), (7, 3.into())]);
        assert_eq!(report.ways, 1);
    }

    #[test]
    fn a_bridge_takes_the_steepness_of_its_own_ends() {
        // Steep approaches meet a bridge whose heights the roads layer interpolates from its ends.
        let up = line(7_500_000, 3, |d| (0.3 * d) as f32);
        let bridge = line(up[2].lon, 4, |d| (12.0 + 0.04 * d) as f32);
        let down = line(bridge[3].lon, 3, |d| (14.4 - 0.3 * d) as f32);
        let bridge = Road { structure: true, ..road(8, (1, 2), bridge) };
        let roads = vec![road(7, (0, 1), up), bridge, road(9, (2, 3), down)];
        let tags: &[(&str, &str)] = &[("highway", "secondary")];
        let (_, rows) = bake(&[(7, tags), (8, &[("highway", "secondary"), ("bridge", "yes")]), (9, tags)], roads);
        let slopes: Vec<_> = rows.iter().map(|row| (row.1, row.3.clone())).collect();
        assert_eq!(slopes, [(7, 5.into()), (8, 1.into()), (9, 5.into())]);
    }

    #[test]
    fn a_stretch_has_the_grades_of_its_way_and_a_row_for_each_block_near_it() {
        // The edge of two blocks of zoom 9 at 7.734375° E; the buffer is about 1,370 microdegrees.
        let edge = 7_734_375;
        let at = |lon: i32| Point { lat: 1_000_000, lon, elevation: 0.0 };
        let (across, near, far) = (
            vec![at(edge - 100), at(edge + 100)],
            vec![at(edge - 600), at(edge - 400)],
            vec![at(edge - 9_000), at(edge - 8_000)],
        );
        let unknown = |shape: Vec<Point>| shape.into_iter().map(|p| Point { elevation: NO_ELEVATION, ..p }).collect();
        let roads = vec![
            Road { difficulty: 3, hiking_difficulty: Some(4), ..road(1, (0, 1), across) },
            Road { hiking_difficulty: Some(255), ..road(2, (2, 3), near) },
            road(3, (4, 5), unknown(far)),
        ];
        let ways: &[(i64, &[(&str, &str)])] =
            &[(1, &[("highway", "path")]), (2, &[("highway", "track")]), (3, &[("highway", "service")])];
        let (report, rows) = bake(ways, roads);
        let block = |x: i64| x * 512 + 254;
        let found: Vec<_> = rows.iter().map(|r| (r.0, r.1, r.2, r.3.clone(), r.4.clone(), r.5.clone(), r.6)).collect();
        let null = Value::Null;
        assert_eq!(
            found,
            [
                (block(266), 1, 12, 0.into(), 3.into(), 4.into(), 1),
                (block(267), 1, 12, 0.into(), 3.into(), 4.into(), 1),
                (block(266), 2, 12, 0.into(), null.clone(), null.clone(), 1),
                (block(267), 2, 12, 0.into(), null.clone(), null.clone(), 1),
                (block(266), 3, 13, null.clone(), null.clone(), null, 0),
            ]
        );
        assert_eq!(rows[0].7, [[edge - 100, 1_000_000], [edge + 100, 1_000_000]]);
        // 111 m of 156 m have no known height.
        assert!((report.unclassified_m / report.length_m - 0.714).abs() < 0.001, "{report:?}");
    }
}
