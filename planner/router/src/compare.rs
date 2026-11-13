//! The request set and the check that compare two engines on the two formats of one bake, or two
//! layouts of one bake on the new engine, and the table for the choice of the download level.
use crate::{
    cells::{self, Directory, Geometry, Topology},
    data::RoutingData,
    install,
    model::{Point, Road, NO_ELEVATION},
    router::Response,
    snap::{self, Candidates, Policy, Position, REACH_M},
    Control, Error, Request, Router,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    time::{Duration, Instant},
};

/// The kinds of request, by `index % 8`, or by `index % 5` for a request set with a least
/// distance: the kinds 5 to 7 stay on one road or within 40 km.
pub const KINDS: [&str; 8] =
    ["points", "junctions", "via-junction", "turnaround", "pinned", "one-road", "one-road-back", "alternatives"];

/// The index in `KINDS` of request `index` of a set with the least distance `min_m`.
pub fn kind(index: usize, min_m: f64) -> usize {
    index % if min_m > 0.0 { 5 } else { KINDS.len() }
}

/// The most distance of a request with alternatives.
const ALTERNATIVES_M: f64 = 40_000.0;

fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn coordinate(point: Point) -> [f64; 2] {
    [f64::from(point.lon) * 1e-6, f64::from(point.lat) * 1e-6]
}

/// The point at `fraction` of the length of `shape`.
fn along(shape: &[Point], fraction: f64) -> Point {
    let mut rest = shape.windows(2).map(|pair| pair[0].distance(pair[1])).sum::<f64>() * fraction;
    for pair in shape.windows(2) {
        let length = pair[0].distance(pair[1]);
        if rest <= length && length > 0.0 {
            let t = rest / length;
            let at = |a: i32, b: i32| a + (f64::from(b - a) * t).round() as i32;
            return Point { lat: at(pair[0].lat, pair[1].lat), lon: at(pair[0].lon, pair[1].lon), ..pair[0] };
        }
        rest -= length;
    }
    shape[shape.len() - 1]
}

/// The candidates of `point` as the router snaps it: within 250 m, else within `REACH_M`. `None`
/// when it has none or the data has no roads there.
fn snap<P: RoutingData>(data: &P, point: Point, profile: &str) -> Option<Candidates> {
    let found = data.snap(point, profile, Policy::default()).ok()?;
    let found = if found.retained.is_empty() {
        data.snap(point, profile, Policy { radius_m: REACH_M, ..Policy::default() }).ok()?
    } else {
        found
    };
    (!found.retained.is_empty()).then_some(found)
}

/// `count` requests of `profile`. A fixed xorshift from `seed` draws the points in the bounds; a
/// point counts when it snaps. Request `i` has the kind `KINDS[kind(i, min_m)]`, and its later
/// spots lie from `min_m` to `max_m` from its first. The list is shorter when 100 tries for each
/// request do not give `count`, as for a profile that snaps nowhere in the bounds.
pub fn requests<P: RoutingData>(
    router: &Router<P>,
    profile: &str,
    count: usize,
    seed: u64,
    min_m: f64,
    max_m: f64,
) -> Vec<Request> {
    let data = router.package();
    let [west, south, east, north] = data.bounds();
    let mut state = seed.max(1);
    let mut uniform = move || (xorshift(&mut state) >> 11) as f64 / (1u64 << 53) as f64;
    let mut requests = Vec::with_capacity(count);
    for _ in 0..count.saturating_mul(100) {
        if requests.len() == count {
            break;
        }
        let kind = kind(requests.len(), min_m);
        let max_m = if kind == 7 { max_m.min(ALTERNATIVES_M) } else { max_m };
        // A spot: a drawn point that snaps, and its nearest road.
        let mut spot = |around: Option<Point>| {
            for _ in 0..1_000 {
                let point = match around {
                    None => [west + uniform() * (east - west), south + uniform() * (north - south)],
                    Some(a) => {
                        let lat = f64::from(a.lat) * 1e-6;
                        let dlat = max_m / 111_320.0;
                        let dlon = dlat / lat.to_radians().cos().max(0.01);
                        [f64::from(a.lon) * 1e-6 + (2.0 * uniform() - 1.0) * dlon, lat + (2.0 * uniform() - 1.0) * dlat]
                    }
                };
                if !(west..=east).contains(&point[0]) || !(south..=north).contains(&point[1]) {
                    continue;
                }
                let point = Point {
                    lat: (point[1] * 1e6).round() as i32,
                    lon: (point[0] * 1e6).round() as i32,
                    elevation: NO_ELEVATION,
                };
                if around.is_some_and(|a| !(min_m..=max_m).contains(&a.distance(point))) {
                    continue;
                }
                if let Some(found) = snap(data, point, profile) {
                    return Some((point, found));
                }
            }
            None
        };
        let Some((a, found_a)) = spot(None) else { continue };
        let Some((_, found_b)) = spot(Some(a)) else { continue };
        let Some((_, found_m)) = spot(Some(a)) else { continue };
        let road = |found: &Candidates| found.retained[0].position.road;
        let shape = |found: &Candidates| data.with_road(road(found), |road| road.shape.clone()).ok();
        let (Some(shape_a), Some(shape_b), Some(shape_m)) = (shape(&found_a), shape(&found_b), shape(&found_m)) else {
            continue;
        };
        let projected = |found: &Candidates| found.retained[0].projected;
        let end = |shape: &Vec<Point>| shape[shape.len() - 1];
        let (pa, pb, pm) = (projected(&found_a), projected(&found_b), projected(&found_m));
        let points = match kind {
            1 => vec![end(&shape_a), end(&shape_b)],
            2 => vec![pa, end(&shape_m), pb],
            3 => {
                let Some((_, found_n)) = spot(Some(a)) else { continue };
                vec![pa, pm, projected(&found_n), pb]
            }
            5 => vec![along(&shape_a, 0.25), along(&shape_a, 0.75)],
            6 => vec![along(&shape_a, 0.75), along(&shape_a, 0.25)],
            _ => vec![pa, pb],
        };
        let mut request =
            json!({ "points": points.iter().map(|&p| coordinate(p)).collect::<Vec<_>>(), "profile": profile });
        match kind {
            3 => request["turnarounds"] = json!([1]),
            4 => {
                // The last candidate of each point, as the router snaps the point of the request.
                let (Some(first), Some(last)) = (snap(data, points[0], profile), snap(data, points[1], profile)) else {
                    continue;
                };
                let pin = |found: &Candidates| found.retained[found.retained.len() - 1].position.id();
                request["start_position"] = json!(pin(&first));
                request["end_position"] = json!(pin(&last));
            }
            7 => request["alternatives"] = json!(true),
            _ => {}
        }
        requests.push(serde_json::from_value(request).expect("a request of known fields"));
    }
    requests
}

/// How the two engines answered a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Same,
    /// A route has other roads at the same cost, and the old data costs the new roads exactly.
    Tie,
    /// Two layouts, and a side cut the candidates of a point at 8, which breaks ties by road ID.
    Skipped,
}

/// The answer of a route without the name of its package.
fn answers(response: &Response) -> Vec<Value> {
    let answer = crate::answer::answer(response);
    let mut routes = answer["routes"].as_array().cloned().unwrap_or_default();
    for route in &mut routes {
        if let Some(route) = route.as_object_mut() {
            route.remove("package");
        }
    }
    routes
}

/// One side of a request: the result, its answers and the time of `routes` and `answer::answer`.
pub struct Routed {
    pub result: crate::Result<Response>,
    pub answers: Vec<Value>,
    pub time: Duration,
}

pub fn route<P: RoutingData>(router: &mut Router<P>, request: &Request) -> Routed {
    let started = Instant::now();
    let result = router.routes(request, &Control::default());
    let answers = result.as_ref().map(answers).unwrap_or_default();
    Routed { result, answers, time: started.elapsed() }
}

/// The name of the error kind, as `NoPath`.
pub fn error_kind(error: &Error) -> String {
    let name = format!("{error:?}");
    name.split('(').next().unwrap_or_default().to_string()
}

/// The coordinates of the start and the end of each leg of `route`, which two layouts of one bake
/// give alike.
fn leg_ends<P: RoutingData>(data: &P, route: &crate::Route) -> Result<Vec<[(i32, i32); 2]>, String> {
    let at = |p: &Position| {
        let point = data.with_road(p.road, |road| along(&road.shape, p.fraction)).map_err(|e| e.to_string())?;
        Ok::<_, String>((point.lat, point.lon))
    };
    route.legs.iter().map(|leg| Ok([at(&leg.start)?, at(&leg.end)?])).collect()
}

/// Compares the two sides of a request. A difference is the `Err`. On two layouts (`layouts`), a
/// route gives its reason, its cost and the coordinates of its leg ends, not its roads.
pub fn verdict<A: RoutingData, B: RoutingData>(
    old: &Router<A>,
    new: &Router<B>,
    a: &Routed,
    b: &Routed,
    layouts: bool,
) -> Result<Outcome, String> {
    let truncated = |routed: &Routed| routed.result.as_ref().is_ok_and(|r| r.routes.iter().any(|r| r.snap_truncated));
    if layouts && (truncated(a) || truncated(b)) {
        return Ok(Outcome::Skipped);
    }
    let (ra, rb) = match (&a.result, &b.result) {
        (Ok(ra), Ok(rb)) => (ra, rb),
        (Err(ea), Err(eb)) if std::mem::discriminant(ea) == std::mem::discriminant(eb) => return Ok(Outcome::Same),
        (ra, rb) => {
            return Err(format!(
                "old {:?}, new {:?}",
                ra.as_ref().map(|r| r.routes.len()),
                rb.as_ref().map(|r| r.routes.len())
            ))
        }
    };
    for (k, (x, y)) in ra.routes.iter().zip(&rb.routes).enumerate() {
        if layouts {
            let (ex, ey) = (leg_ends(old.package(), x)?, leg_ends(new.package(), y)?);
            if (x.reason, x.cost, &ex) != (y.reason, y.cost, &ey) {
                return Err(format!(
                    "route {k}: old {} {} {ex:?}, new {} {} {ey:?}",
                    x.reason, x.cost, y.reason, y.cost
                ));
            }
            continue;
        }
        let position = |p: &Position| (p.road, p.fraction.to_bits());
        let ends = |route: &crate::Route| -> Vec<_> {
            route.legs.iter().map(|leg| (position(&leg.start), position(&leg.end))).collect()
        };
        if (x.reason, x.cost, x.snap_truncated, ends(x)) != (y.reason, y.cost, y.snap_truncated, ends(y)) {
            return Err(format!(
                "route {k}: old {} {} {:?}, new {} {} {:?}",
                x.reason,
                x.cost,
                ends(x),
                y.reason,
                y.cost,
                ends(y)
            ));
        }
        let pieces = |route: &crate::Route| -> Vec<_> {
            route.pieces().map(|piece| (piece.road, piece.from.to_bits(), piece.to.to_bits())).collect()
        };
        if pieces(x) != pieces(y) {
            return match old.cost(y, &y.profile) {
                Ok(Some(cost)) if cost == y.cost => Ok(Outcome::Tie),
                cost => Err(format!("route {k}: other roads at cost {}, which the old data costs {cost:?}", y.cost)),
            };
        }
        if a.answers.get(k) != b.answers.get(k) {
            return Err(format!("route {k}: the answers differ"));
        }
    }
    if ra.routes.len() != rb.routes.len() {
        return Err(format!("old {} routes, new {}", ra.routes.len(), rb.routes.len()));
    }
    Ok(Outcome::Same)
}

/// Routes `request` on both routers of the two formats of one bake and compares the answers; also
/// gives the time of each side. A difference is the `Err`.
pub fn check<A: RoutingData, B: RoutingData>(
    old: &mut Router<A>,
    new: &mut Router<B>,
    request: &Request,
) -> Result<(Outcome, [Duration; 2]), String> {
    let (a, b) = (route(old, request), route(new, request));
    Ok((verdict(old, new, &a, &b, false)?, [a.time, b.time]))
}

/// The value at the share `q` of `sorted`, as 0.5 for the median.
pub fn quantile<T: Copy + Default>(sorted: &[T], q: f64) -> T {
    sorted.get(((sorted.len().max(1) - 1) as f64 * q).round() as usize).copied().unwrap_or_default()
}

/// One row of the table for the download level: the package of an install as if `level` were its
/// download level.
#[derive(Debug)]
pub struct Level {
    pub level: u8,
    pub cells: usize,
    /// The median and the largest road count of a cell.
    pub roads: [u32; 2],
    /// The objects of the whole package: 2 + groups for each cell of `level`, and 1 + groups for
    /// each cell above it, which has no geometry.
    pub objects: u64,
    /// The median and the largest bytes of an object of a cell of `level`, from the object bytes
    /// for each road of each part of the download cells of the install.
    pub object_bytes: [u64; 2],
    /// The files and the bytes of `directory.bin` of a world of `WORLD_ROADS` roads.
    pub world_files: f64,
    pub world_directory: f64,
    /// For the zoom-9 tiles and for the 3 × 3 blocks of them: the number of areas, and the median
    /// and the P90 of the routing bytes of an area over the bytes of its own roads.
    pub tiles: (usize, f64, f64),
    pub blocks: (usize, f64, f64),
}

const WORLD_ROADS: f64 = 1e9;
const ZOOM: u32 = 9;

fn tiles() -> f64 {
    f64::from(1u32 << ZOOM)
}

/// The tile coordinate of a longitude and of a latitude, in degrees, as fractions.
fn tile_x(lon: f64) -> f64 {
    (lon + 180.0) / 360.0 * tiles()
}
fn tile_y(lat: f64) -> f64 {
    (1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0 * tiles()
}

/// The box of tile `(x, y)` in microdegrees.
fn tile_box(x: i64, y: i64) -> [i32; 4] {
    let lon = |x: i64| x as f64 / tiles() * 360.0 - 180.0;
    let lat = |y: i64| (std::f64::consts::PI * (1.0 - 2.0 * y as f64 / tiles())).sinh().atan().to_degrees();
    [lon(x), lat(y + 1), lon(x + 1), lat(y)].map(|degrees| (degrees * 1e6).round() as i32)
}

/// `area` grown by the snap window of `REACH_M` at its side nearer to a pole.
// TODO(#3309): call `install::area_cells` here once #3309 has merged; it grows an area by the same rule.
fn grown(area: [i32; 4]) -> Result<[i32; 4], String> {
    let pole = if area[1].unsigned_abs() > area[3].unsigned_abs() { area[1] } else { area[3] };
    let window = snap::window(Point { lat: pole, lon: 0, elevation: NO_ELEVATION }, REACH_M)?;
    let (lon, lat) = (-window[0], pole - window[1]);
    Ok([area[0] - lon, area[1] - lat, area[2] + lon, area[3] + lat])
}

/// The median and the P90 of the area overhead at each level, and the number of areas.
fn overheads(mut values: Vec<f64>) -> (usize, f64, f64) {
    values.sort_by(f64::total_cmp);
    (values.len(), quantile(&values, 0.5), quantile(&values, 0.9))
}

/// The table for the download level, from an install of all cells in `folder`: one row for each
/// level from 2 to the top. An area takes the cells whose box meets it, grown by the snap window,
/// and its own roads are those whose last point lies in it. The areas are the zoom-9 tiles and the
/// 3 × 3 blocks of them that lie in the bounds with a tile of the bounds between them and the edge.
/// Each road has the same installed bytes, so the overhead is a ratio of road counts.
pub fn areas(folder: &Path) -> Result<Vec<Level>, String> {
    let [west, south, east, north] = crate::open::cells(folder, true).map_err(|e| e.to_string())?.bounds();
    let (bytes, plan) = install::installed(folder).map_err(|e| e.to_string())?;
    let directory = Directory::new(&bytes).map_err(|e| e.to_string())?;
    if plan.records.len() != directory.len() {
        return Err("The table needs an install of all cells".into());
    }
    let parts = directory.parts();
    // The object bytes of each part, the level-1 cells (ID, roads, box) and the own roads of each tile.
    let (mut part_bytes, mut roads) = (vec![0u64; parts], 0u64);
    let mut level1: Vec<(u32, u32, [i32; 4])> = Vec::new();
    let mut own: HashMap<(i64, i64), u64> = HashMap::new();
    let mut road = Road::default();
    for index in 0..directory.downloads() {
        let cell = directory.cell(index).map_err(|e| e.to_string())?;
        roads += u64::from(cell.roads);
        for (sum, part) in part_bytes.iter_mut().zip(&cell.parts) {
            *sum += u64::from(part.object_bytes);
        }
        let read = |part| {
            let path = folder.join(install::CELLS).join(install::name(cell.level, cell.id, part));
            std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
        };
        let (topology, geometry) = (read(cells::TOPOLOGY)?, read(cells::GEOMETRY)?);
        let (topology, geometry) = (
            Topology::new(&topology).map_err(|e| e.to_string())?,
            Geometry::new(&geometry).map_err(|e| e.to_string())?,
        );
        for k in 0..topology.cells {
            let (id, cell) =
                (topology.level1(k).map_err(|e| e.to_string())?.id, geometry.level1(k).map_err(|e| e.to_string())?);
            level1.push((id, cell.roads, cell.bounds));
            for r in 0..cell.roads {
                geometry.road(&cell, r, &mut road).map_err(|e| e.to_string())?;
                let end = road.shape[road.shape.len() - 1];
                let (x, y) = (tile_x(f64::from(end.lon) * 1e-6), tile_y(f64::from(end.lat) * 1e-6));
                *own.entry((x.floor() as i64, y.floor() as i64)).or_default() += 1;
            }
        }
    }
    // The tiles that lie in the bounds and are not at their edge; a block is named by its first tile.
    let (x0, x1) = (tile_x(west).ceil() as i64 + 1, tile_x(east).floor() as i64 - 2);
    let (y0, y1) = (tile_y(north).ceil() as i64 + 1, tile_y(south).floor() as i64 - 2);
    let inner: Vec<(i64, i64)> = (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| (x, y))).collect();
    let mut areas = Vec::new();
    for &(x, y) in &inner {
        areas.push((1, x, y));
        if x + 2 <= x1 && y + 2 <= y1 {
            areas.push((3, x, y));
        }
    }
    let rate: Vec<f64> = part_bytes.iter().map(|&bytes| bytes as f64 / roads.max(1) as f64).collect();
    let header = directory.header;
    let mut counts = BTreeMap::new();
    let mut table = Vec::new();
    for level in 2..=header.levels {
        let mut cells: BTreeMap<u32, (u32, [i32; 4])> = BTreeMap::new();
        for &(id, roads, bounds) in &level1 {
            let entry = cells.entry(id & !directory.below(level)).or_insert((0, bounds));
            entry.0 += roads;
            entry.1 = [
                entry.1[0].min(bounds[0]),
                entry.1[1].min(bounds[1]),
                entry.1[2].max(bounds[2]),
                entry.1[3].max(bounds[3]),
            ];
        }
        counts.insert(level, cells.len() as u64);
        let mut sizes: Vec<u32> = cells.values().map(|&(roads, _)| roads).collect();
        sizes.sort_unstable();
        let mut objects: Vec<u64> = sizes
            .iter()
            .flat_map(|&roads| rate.iter().map(move |rate| (rate * f64::from(roads)).round() as u64))
            .collect();
        objects.sort_unstable();
        let (mut tiles, mut blocks) = (Vec::new(), Vec::new());
        for &(side, x, y) in &areas {
            let area = [
                tile_box(x, y + side - 1)[0],
                tile_box(x, y + side - 1)[1],
                tile_box(x + side - 1, y)[2],
                tile_box(x + side - 1, y)[3],
            ];
            let window = grown(area)?;
            let taken: u64 = cells
                .values()
                .filter(|(_, bounds)| cells::meets(*bounds, window))
                .map(|&(roads, _)| u64::from(roads))
                .sum();
            let mine: u64 = (0..side)
                .flat_map(|dy| (0..side).map(move |dx| (x + dx, y + dy)))
                .map(|tile| own.get(&tile).copied().unwrap_or(0))
                .sum();
            if mine > 0 {
                (if side == 1 { &mut tiles } else { &mut blocks }).push(taken as f64 / mine as f64);
            }
        }
        table.push(Level {
            level,
            cells: cells.len(),
            roads: [quantile(&sizes, 0.5), quantile(&sizes, 1.0)],
            objects: 0,
            object_bytes: [quantile(&objects, 0.5), quantile(&objects, 1.0)],
            world_files: 0.0,
            world_directory: 0.0,
            tiles: overheads(tiles),
            blocks: overheads(blocks),
        });
    }
    // The cells above a level have no geometry part; the directory has a record for each cell
    // from the level up.
    let scale = WORLD_ROADS / roads.max(1) as f64;
    for row in &mut table {
        let above: u64 = counts.range(row.level + 1..).map(|(_, &count)| count).sum();
        row.objects = row.cells as u64 * parts as u64 + above * (parts as u64 - 1);
        row.world_files = row.objects as f64 * scale;
        let records = (row.cells as u64 + above) as f64 * scale;
        row.world_directory = cells::HEADER as f64 + records * cells::record_bytes(parts - 2) as f64;
    }
    Ok(table)
}
