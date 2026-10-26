//! The bake of a region: the features of the kind table with their heights, cut into the tiles of
//! one archive.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Instant;

use serde_json::{json, Map, Value};

use crate::kinds::{kind_of, Entry};
use crate::osm::{self, Selected, LANDUSE, LIFT_REACH_M};
use crate::pmtiles::{self, Header};
use crate::roads::{self, Classes, River};
use crate::structures::{self, class_of, is_bridge, is_track, road_half};
use crate::tiles::{simplified, Geometry, Tiles, BUFFER_M, EXTENT, FORMAT, MAX_ZOOM, TOLERANCE};
use crate::water::{self, Kind, Level};
use crate::{lifts, rails, world, Part, Tags, Terrain};

const EARTH_RADIUS_KM: f64 = 6371.0088;
/// The waterways and lakes that the water solver sets, and the falls, weirs and dams that keep the
/// smoothing of a level from crossing them.
const RIVERS: [&str; 2] = ["river", "stream"];
const LAKES: [&str; 1] = ["lake"];
const MARKS: [&str; 3] = ["waterfall", "dam", "dam-point"];
/// The ways that the road solver sets the heights of, but a cable route on rock: it has no bench to stamp.
const ROADS: [&str; 6] = ["main-road", "road", "path", "railway", "tunnel", "gallery"];

/// What the step bakes: the kind table, the region, the credit and the road classes.
pub struct Options {
    pub kinds: Vec<Entry>,
    pub bounds: [f64; 4],
    /// The bounds as the options give them, for the metadata.
    pub bounds_json: Vec<Value>,
    pub attribution: String,
    pub classes: Classes,
    pub class_names: Vec<String>,
}

/// A feature as it enters the tiles: its kind, geometry and lowest zoom, its tags and parts.
struct Placed {
    kind: String,
    geometry: Geometry,
    min_zoom: u8,
    tags: Tags,
    parts: Vec<Part>,
}

/// The tiles of the bake, and what the low zooms need of the features that reach them.
struct Placer {
    tiles: Tiles,
    low: Vec<Placed>,
    shared: HashMap<[u64; 2], u32>,
    counts: BTreeMap<String, u64>,
}

impl Placer {
    fn place(&mut self, kind: &str, geometry: Geometry, min_zoom: u8, tags: Tags, parts: Vec<Part>) {
        if self.tiles.add(MAX_ZOOM, kind, geometry, &tags, &parts) {
            *self.counts.entry(kind.to_string()).or_default() += 1;
        }
        if min_zoom < MAX_ZOOM && self.tiles.meets(min_zoom, &parts) {
            for point in parts.iter().flat_map(|p| &p.points) {
                *self.shared.entry(bits(*point)).or_default() += 1;
            }
            self.low.push(Placed { kind: kind.to_string(), geometry, min_zoom, tags, parts });
        }
    }

    fn entry(&mut self, entry: &Entry, tags: Tags, parts: Vec<Part>) {
        self.place(&entry.kind, entry.geometry, entry.min_zoom, tags, parts);
    }
}

fn bits(point: [f64; 2]) -> [u64; 2] {
    [point[0].to_bits(), point[1].to_bits()]
}

fn line(points: Vec<[f64; 2]>, z: Option<Vec<f64>>) -> Vec<Part> {
    vec![Part { outer: false, points, z }]
}

/// Write the archive of the region to `output` from the OSM `pbf` and the `terrain`; the metrics.
/// `work` holds the cut of the OSM, `memory` is what the road jobs may take.
pub fn bake(
    options: &Options,
    pbf: &Path,
    terrain: &dyn Terrain,
    output: &Path,
    work: &Path,
    memory: usize,
) -> Result<Value, String> {
    let kinds = &options.kinds;
    let (binary, version) = osm::probe()?;
    let mut placer =
        Placer { tiles: Tiles::new(options.bounds), low: Vec::new(), shared: HashMap::new(), counts: BTreeMap::new() };
    let mut metrics = Map::new();
    let started = Instant::now();
    let cut = osm::selected(kinds, pbf, &binary, &placer.tiles, work)?;
    // The lifts wait for the buildings and the other lifts: a station shares their places. The buildings
    // of the region wait for the landuse areas that hold them; the features of the solvers for all of
    // their kind.
    let (mut lift_ways, mut lift_entry, mut buildings, mut houses, mut landuse) = (Vec::new(), None, Vec::new(), Vec::new(), Vec::new());
    let (mut waters, mut ways, mut stations, mut beyond, mut river_areas) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    osm::objects(kinds, &cut, &binary, |(selected, tags, mut parts)| {
        let Selected::Entry(e) = selected else {
            if placer.tiles.meets(MAX_ZOOM, &parts) {
                landuse.push((tags, parts));
            }
            return;
        };
        let entry = &kinds[e];
        let kind = entry.kind.as_str();
        if lifts::solves(kind, &tags) {
            lift_entry = Some(e);
            lift_ways.extend(parts.into_iter().map(|part| (tags.clone(), part.points)));
            return;
        }
        if kind == "building" {
            buildings.extend(parts.iter().filter(|p| p.outer).map(|p| p.points.clone()));
            if placer.tiles.meets(MAX_ZOOM, &parts) {
                houses.push((e, tags, parts));
            }
            return;
        }
        if placer.tiles.meets(MAX_ZOOM, &parts) {
            if solves_water(kind, &tags) {
                waters.push((e, tags, parts));
                return;
            }
            if solves_road(kind, &tags) {
                ways.push((e, tags, parts));
                return;
            }
            if rails::solves(kind) {
                stations.push((e, tags, parts));
                return;
            }
        }
        if kind == "river-area" {
            river_areas.push(parts.iter().map(|p| p.points.clone()).collect::<Vec<_>>());
        }
        if solves_road(kind, &tags) {
            beyond.push(road(kind, &tags, &parts, options));
        }
        parts.iter_mut().for_each(|p| p.z = None);
        placer.entry(entry, tags, parts);
    })?;
    eprintln!("osm: {:.1} s", started.elapsed().as_secs_f64());
    let values = landuse_of(&houses.iter().map(|(_, _, parts)| parts).collect::<Vec<_>>(), &landuse);
    for ((e, mut tags, parts), value) in houses.into_iter().zip(&values) {
        if let Some(value) = value {
            tags.insert("landuse".into(), value.clone());
        }
        placer.entry(&kinds[e], tags, parts);
    }
    let landuse_counts: Map<String, Value> =
        LANDUSE.iter().map(|l| (l.to_string(), values.iter().filter(|v| v.as_deref() == Some(l)).count().into())).collect();
    metrics.insert("landuse".into(), landuse_counts.into());
    drop(landuse);
    if let Some(e) = lift_entry {
        let (rigged, lift_metrics) = lifts::rigged(&lift_ways, &buildings, Some(terrain));
        for (tags, points) in rigged {
            placer.entry(&kinds[e], tags, line(points, None));
        }
        metrics.insert(
            "lifts".into(),
            json!({"lifts": lift_metrics.lifts, "added_pylons": lift_metrics.added_pylons,
                   "housed_ends": lift_metrics.housed_ends, "shared_ends": lift_metrics.shared_ends}),
        );
    }
    drop(buildings);
    eprintln!("placed: {:.1} s", started.elapsed().as_secs_f64());

    // The heights of a junction need every road that meets there, a river level its whole network, a
    // ford the water it crosses, and a platform its tracks.
    if !(waters.is_empty() && ways.is_empty() && stations.is_empty()) {
        let mut features: Vec<water::Feature> =
            waters.iter_mut().map(|(e, tags, parts)| water_feature(&kinds[*e].kind, tags, std::mem::take(parts))).collect();
        let pool = rayon::ThreadPoolBuilder::new().build().map_err(|e| e.to_string())?;
        let (levels, _, water_metrics) = pool.install(|| water::solve_by_network(&mut features, terrain));
        // The threads of the roads take over the memory of these.
        drop(pool);
        metrics.insert(
            "water".into(),
            json!({"lakes": water_metrics.lakes, "lakes_spilling": water_metrics.lakes_spilling,
                   "river_parts": water_metrics.river_parts, "turned": water_metrics.turned}),
        );
        // A waterway that OSM draws uphill has turned round in `features`.
        for ((_, _, parts), (feature, z)) in waters.iter_mut().zip(features.into_iter().zip(levels)) {
            *parts = feature.parts.into_iter().zip(z).map(|(p, z)| Part { z, ..p }).collect();
        }
        eprintln!("water: {:.1} s", started.elapsed().as_secs_f64());
        let is_river = |e: usize| RIVERS.contains(&kinds[e].kind.as_str());
        let rivers: Vec<River> = waters
            .iter()
            .filter(|(e, _, _)| is_river(*e))
            .flat_map(|(_, _, parts)| parts)
            .filter_map(|p| Some(River { points: p.points.clone(), levels: p.z.clone()? }))
            .collect();
        // A bridge or a tunnel runs between the ends of its chain, also where they lie past the region. A
        // chain that the input cuts has no ends to run between.
        let [west, south, east, north] = osm::reach(&placer.tiles, MAX_ZOOM, LIFT_REACH_M);
        let ([x0, y1], [x1, y0]) = (world(west, south), world(east, north));
        let inside = |[x, y]: [f64; 2]| x > x0 && x < x1 && y > y0 && y < y1;
        let road_features: Vec<roads::Feature> =
            ways.iter().map(|(e, tags, parts)| road(&kinds[*e].kind, tags, parts, options)).collect();
        let (context, cut) = roads::context(&road_features, &beyond, &inside);
        let kept: Vec<usize> = (0..road_features.len()).filter(|f| !cut.contains(f)).collect();
        let solved = roads::solve_by_area(
            &kept.iter().map(|&f| &road_features[f]).collect::<Vec<_>>(),
            &context.iter().map(|&f| &beyond[f]).collect::<Vec<_>>(),
            terrain,
            &rivers,
            &options.classes,
            memory,
        );
        drop((road_features, rivers, beyond));
        eprintln!("roads: {:.1} s", started.elapsed().as_secs_f64());
        let mut road_metrics = road_metrics(&solved.metrics);
        road_metrics.insert("cut_features".into(), cut.len().into());
        let culverts: Vec<usize> = solved.culverts.iter().map(|&i| kept[i]).collect();
        let underpasses: Vec<usize> = solved.underpasses.iter().map(|&i| kept[i]).collect();
        for (&f, parts) in kept.iter().zip(solved.heights) {
            for (part, solved) in ways[f].2.iter_mut().zip(parts) {
                if let Some((points, z)) = solved {
                    (part.points, part.z) = (points, Some(z));
                }
            }
        }
        for (e, tags, parts) in &waters {
            placer.entry(&kinds[*e], tags.clone(), parts.clone());
        }
        let mut tracks: Vec<(String, Tags, Part)> = Vec::new();
        if !ways.is_empty() {
            // A culvert is a road: the stamps build it over the ground.
            for &n in &culverts {
                ways[n].1.remove("bridge");
            }
            let decks: Vec<usize> =
                (0..ways.len()).filter(|&n| is_bridge(&kinds[ways[n].0].kind, &ways[n].1) && ways[n].2[0].z.is_some()).collect();
            let waterways: Vec<(f64, &[[f64; 2]])> = waters
                .iter()
                .filter(|(e, _, _)| is_river(*e))
                .flat_map(|(_, tags, parts)| parts.iter().map(move |p| (half_width(tags), p.points.as_slice())))
                .collect();
            let areas: Vec<Vec<&[[f64; 2]]>> = waters
                .iter()
                .filter(|(e, _, _)| LAKES.contains(&kinds[*e].kind.as_str()))
                .map(|(_, _, parts)| parts.iter().map(|p| p.points.as_slice()).collect())
                .chain(river_areas.iter().map(|rings| rings.iter().map(Vec::as_slice).collect()))
                .collect();
            let under: Vec<(f64, &[[f64; 2]])> = ways
                .iter()
                .filter(|(e, tags, _)| !is_bridge(&kinds[*e].kind, tags))
                .flat_map(|(_, tags, parts)| parts.iter().map(move |p| (road_half(tags), p.points.as_slice())))
                .collect();
            let deck_ways: Vec<structures::Way> = decks
                .iter()
                .map(|&n| {
                    let (e, tags, parts) = &ways[n];
                    (kinds[*e].kind.as_str(), tags, parts[0].points.as_slice(), parts[0].z.as_deref().unwrap())
                })
                .collect();
            let (added, piers, bridge_metrics) = structures::bridges(&deck_ways, &waterways, &areas, terrain, &under);
            let mut extra: HashMap<usize, Tags> =
                decks.iter().zip(added).map(|(&n, tags)| (n, tags.unwrap_or_default())).collect();
            let (mut floating, mut shallow) = (None, None);
            for (n, (e, tags, parts)) in ways.iter().enumerate() {
                let (mut entry, mut tags, mut parts) = (&kinds[*e], tags.clone(), parts.clone());
                if entry.kind == "gallery" && parts[0].z.is_some() {
                    let side = structures::uphill(&parts[0].points, terrain);
                    extra.insert(n, Tags::from([("uphill".to_string(), side.to_string())]));
                }
                let tunnel = entry.kind == "tunnel" && tags.get("tunnel").is_none_or(|t| t != "building_passage");
                if tunnel {
                    let count = parts.len();
                    parts.retain(|p| !p.z.as_ref().is_some_and(|z| structures::floating(&p.points, z, terrain)));
                    *floating.get_or_insert(0) += count - parts.len();
                    if parts.is_empty() {
                        continue;
                    }
                }
                let heights = || parts.iter().filter_map(|p| Some((p, p.z.as_ref()?)));
                if tunnel
                    && !underpasses.contains(&n)
                    && heights().next().is_some()
                    && heights().all(|(p, z)| structures::shallow(&tags, &p.points, z, terrain))
                {
                    // A tunnel under what the terrain leaves out, such as a building, is a way on the ground.
                    tags.remove("tunnel");
                    let mut ground = tags.clone();
                    ground.insert("tunnel".into(), "no".into());
                    entry = kind_of(kinds, Geometry::Line, &ground).unwrap_or(entry);
                    *shallow.get_or_insert(0) += 1;
                }
                let mut all = tags.clone();
                all.extend(extra.get(&n).cloned().unwrap_or_default());
                if is_track(&tags) {
                    tracks.extend(parts.iter().filter(|p| p.z.is_some()).map(|p| (entry.kind.clone(), tags.clone(), p.clone())));
                }
                placer.entry(entry, all, parts);
            }
            for (point, z, way) in piers {
                let min_zoom = kinds[ways[decks[way]].0].min_zoom;
                placer.place("bridge-pier", Geometry::Point, min_zoom, Tags::new(), line(vec![point], Some(vec![z])));
            }
            if let Some(count) = floating {
                road_metrics.insert("floating_tunnels".into(), count.into());
            }
            if let Some(count) = shallow {
                road_metrics.insert("shallow_tunnels".into(), count.into());
            }
            metrics.insert("roads".into(), road_metrics.into());
            metrics.insert(
                "bridges".into(),
                json!({"bridges": bridge_metrics.bridges, "cable_bridges": bridge_metrics.cable_bridges,
                       "piers": bridge_metrics.piers, "piers_moved": bridge_metrics.piers_moved}),
            );
        }
        let outside: Vec<rails::Track> = tracks.iter().filter(|t| t.0 != "tunnel").map(track).collect();
        let wired: Vec<(rails::Track, bool)> = tracks
            .iter()
            .filter(|t| t.0 == "railway" && rails::electrified(&t.1))
            .map(|t| (track(t), is_bridge(&t.0, &t.1)))
            .collect();
        for (point, z, tags) in rails::masts(&wired, &outside) {
            placer.place("catenary-mast", Geometry::Point, MAX_ZOOM, tags, line(vec![point], Some(vec![z])));
        }
        let platforms = rails::platforms(&stations.iter().map(|(_, _, parts)| parts.clone()).collect::<Vec<_>>(), &outside);
        let unknown = platforms.iter().filter(|p| p.is_none()).count();
        for ((e, tags, parts), solved) in stations.into_iter().zip(platforms) {
            placer.entry(&kinds[e], tags, solved.unwrap_or(parts));
        }
        let masts = placer.counts.get("catenary-mast").copied().unwrap_or(0);
        metrics.insert("rails".into(), json!({"tracks": tracks.len(), "masts": masts, "platforms_unknown": unknown}));
    }
    low_zooms(&mut placer);
    let size = write(&placer, options, output)?;
    eprintln!("tiles: {:.1} s", started.elapsed().as_secs_f64());
    let mut out = Map::new();
    out.insert("osmium".into(), version.lines().next().unwrap_or_default().into());
    out.insert("kinds".into(), json!(placer.counts));
    out.insert("tiles".into(), placer.tiles.tiles.len().into());
    out.insert("bytes".into(), size.into());
    out.insert("bytes_per_km2".into(), (size as f64 / area_km2(options.bounds)).round_ties_even().into());
    out.extend(metrics.into_iter().filter(|(_, v)| !v.as_object().is_some_and(Map::is_empty)));
    Ok(sorted(Value::Object(out)))
}

/// A JSON value with the keys of each object sorted, as Python's `json.dumps(sort_keys=True)` writes it.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let map: BTreeMap<String, Value> = map.into_iter().map(|(k, v)| (k, sorted(v))).collect();
            Value::Object(map.into_iter().collect())
        }
        Value::Array(list) => Value::Array(list.into_iter().map(sorted).collect()),
        other => other,
    }
}

fn track((_, tags, part): &(String, Tags, Part)) -> rails::Track<'_> {
    (tags, part.points.as_slice(), part.z.as_deref().unwrap())
}

fn solves_water(kind: &str, tags: &Tags) -> bool {
    (RIVERS.contains(&kind) || LAKES.contains(&kind) || MARKS.contains(&kind))
        && !(RIVERS.contains(&kind) && tags.get("tunnel").is_some_and(|t| t != "no"))
}

fn solves_road(kind: &str, tags: &Tags) -> bool {
    ROADS.contains(&kind) && tags.get("highway").is_none_or(|h| h != "via_ferrata")
}

/// The leading number of a tag, as JavaScript's parseFloat reads it: "8 m" is 8. NaN without one.
fn number(value: Option<&String>) -> f64 {
    let Some(value) = value else { return f64::NAN };
    let text = value.trim_start_matches(structures::is_space);
    let bytes = text.as_bytes();
    let mut end = usize::from(bytes.first().is_some_and(|b| *b == b'+' || *b == b'-'));
    let digits = |from: usize| from + bytes[from..].iter().take_while(|b| b.is_ascii_digit()).count();
    let whole = digits(end);
    let (mut at, fraction) = (whole, whole > end);
    if bytes.get(at) == Some(&b'.') {
        let after = digits(at + 1);
        if !fraction && after == at + 1 {
            return f64::NAN;
        }
        at = after;
    } else if !fraction {
        return f64::NAN;
    }
    end = at;
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(bytes.get(end + 1), Some(b'+' | b'-')));
        let exponent = digits(end + 1 + sign);
        if exponent > end + 1 + sign {
            end = exponent;
        }
    }
    text[..end].parse().unwrap_or(f64::NAN)
}

/// The half-width of a waterway in metres: its `width`, else that of its kind.
fn half_width(tags: &Tags) -> f64 {
    let width = number(tags.get("width"));
    if width.is_finite() {
        return width / 2.0;
    }
    match tags.get("waterway").map(String::as_str) {
        Some("river" | "canal") => 5.0,
        _ => 1.0,
    }
}

fn water_feature(kind: &str, tags: &Tags, parts: Vec<Part>) -> water::Feature {
    let kind = if RIVERS.contains(&kind) {
        Kind::River
    } else if LAKES.contains(&kind) {
        Kind::Lake
    } else {
        Kind::Mark
    };
    let level = if tags.get("water").is_some_and(|w| w == "pond") {
        Level::Pond
    } else if tags.get("water").is_some_and(|w| w == "reservoir") || tags.get("landuse").is_some_and(|l| l == "reservoir") {
        Level::Reservoir
    } else {
        Level::Natural
    };
    water::Feature { kind, level, half: half_width(tags), ele: number(tags.get("ele")), parts }
}

/// A way of the road solver, with the facts of its tags.
fn road(kind: &str, tags: &Tags, parts: &[Part], options: &Options) -> roads::Feature {
    let free = kind == "tunnel" || tags.get("bridge").is_some_and(|b| b != "no");
    let layer = tags.get("layer").and_then(|l| l.trim().parse::<i64>().ok()).unwrap_or(0);
    let name = class_of(tags);
    let rule = options.class_names.iter().position(|n| n == name).unwrap_or_else(|| {
        let fallback = if kind == "path" { "path" } else { "road" };
        options.class_names.iter().position(|n| n == fallback).unwrap()
    });
    roads::Feature {
        free,
        tunnel: kind == "tunnel",
        ford: tags.get("ford").is_some_and(|f| f != "no"),
        layer,
        layered: tags.contains_key("layer"),
        indoor: !free && layer < 0 && tags.get("tunnel").is_none_or(|t| t == "no"),
        half: road_half(tags),
        rule,
        parts: parts.iter().map(|p| p.points.clone()).collect(),
    }
}

fn round(value: f64, digits: i32) -> f64 {
    let scale = 10f64.powi(digits);
    (value * scale).round_ties_even() / scale
}

fn road_metrics(m: &roads::Metrics) -> Map<String, Value> {
    let value = json!({
        "ways": m.ways, "variables": m.variables, "known": m.known, "approaches": m.approaches, "fords": m.fords,
        "culverts": m.culverts, "rounds": m.rounds, "held": m.held, "clearances": m.clearances,
        "clearances_low_cm": m.clearances_low_cm, "passes": m.passes, "passes_near_cm": m.passes_near_cm,
        "grade_excess_m": round(m.grade_excess_m, 4), "grade_excess_at": m.grade_excess_at.map(|v| round(v, 5)),
        "grade_excess_over_1cm": m.grade_excess_over_1cm, "residual_p99_m": round(m.residual_p99_m, 2),
        "refinements_unconverged": m.refinements_unconverged,
    });
    value.as_object().unwrap().clone()
}

/// Per building, the `landuse` of the smallest of the `areas` that holds the middle of its outer ring,
/// or None. A point lies in an area when it lies inside an odd number of its rings.
fn landuse_of(buildings: &[&Vec<Part>], areas: &[(Tags, Vec<Part>)]) -> Vec<Option<String>> {
    let boxes: Vec<[f64; 4]> = areas
        .iter()
        .map(|(_, parts)| {
            parts.iter().flat_map(|p| &p.points).fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, p| {
                [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
            })
        })
        .collect();
    let twice = |r: &[[f64; 2]]| -> f64 {
        let n = r.len();
        let a: f64 = (0..n).map(|i| r[i][0] * r[(i + 1) % n][1]).sum();
        let b: f64 = (0..n).map(|i| r[i][1] * r[(i + 1) % n][0]).sum();
        (a - b).abs()
    };
    let sizes: Vec<f64> = areas
        .iter()
        .map(|(_, parts)| parts.iter().map(|p| twice(&p.points) * if p.outer { 1.0 } else { -1.0 }).sum())
        .collect();
    buildings
        .iter()
        .map(|parts| {
            let ring = &parts[0].points;
            let (sx, sy) = ring.iter().fold((0.0, 0.0), |(x, y), p| (x + p[0], y + p[1]));
            let (x, y) = (sx / ring.len() as f64, sy / ring.len() as f64);
            let hits = (0..areas.len()).filter(|&k| {
                let b = boxes[k];
                b[0] <= x && x <= b[2] && b[1] <= y && y <= b[3] && {
                    let rings: Vec<&[[f64; 2]]> = areas[k].1.iter().map(|p| p.points.as_slice()).collect();
                    structures::contains(&rings, [x, y])
                }
            });
            let smallest = hits.fold(None, |best: Option<usize>, k| match best {
                Some(b) if sizes[b] <= sizes[k] => Some(b),
                _ => Some(k),
            });
            smallest.and_then(|k| areas[k].0.get("landuse").cloned())
        })
        .collect()
}

/// The area of the region in km².
fn area_km2([west, south, east, north]: [f64; 4]) -> f64 {
    EARTH_RADIUS_KM.powi(2) * (east - west).to_radians() * (north.to_radians().sin() - south.to_radians().sin())
}

/// A string as JSON with every character past ASCII escaped, as Python's `json.dumps` writes it.
fn ascii_json(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e && c != '\u{7f}' => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A number as Python's `json.dumps` writes it.
fn python_number(value: &Value) -> String {
    match value.as_i64() {
        Some(integer) => integer.to_string(),
        None => format!("{:?}", value.as_f64().unwrap_or(f64::NAN)),
    }
}

/// Write the tiles to the archive at `output`; its bytes.
fn write(placer: &Placer, options: &Options, output: &Path) -> Result<u64, String> {
    let mut tiles: Vec<(u64, Vec<u8>)> =
        placer.tiles.tiles.iter().map(|(&(z, x, y), tile)| (pmtiles::tile_id(z, x, y), pmtiles::gzip(&tile.bytes()))).collect();
    if tiles.is_empty() {
        return Err("The region has no TrailView features".into());
    }
    tiles.sort_by_key(|t| t.0);
    let [west, south, east, north] = options.bounds;
    let e7 = |value: f64| (value * 1e7).round_ties_even() as i32;
    let header = Header {
        tile_type: 0,
        tile_compression: 2,
        bounds_e7: [e7(west), e7(south), e7(east), e7(north)],
        center_zoom: MAX_ZOOM,
        center_e7: [e7((west + east) / 2.0), e7((south + north) / 2.0)],
    };
    let bounds: Vec<String> = options.bounds_json.iter().map(python_number).collect();
    let metadata = format!(
        "{{\"features_format\": {FORMAT}, \"extent\": {EXTENT}, \"buffer_m\": {BUFFER_M:?}, \"attribution\": {}, \"bounds\": [{}]}}",
        ascii_json(&options.attribution),
        bounds.join(", ")
    );
    pmtiles::write(output, &tiles, &header, &metadata)
}

/// Add the features that reach the low zooms to them, simplified at each zoom. The vertices that
/// other features share stay.
fn low_zooms(placer: &mut Placer) {
    let low = std::mem::take(&mut placer.low);
    for feature in &low {
        let anchors: Vec<Vec<usize>> = feature
            .parts
            .iter()
            .map(|p| (0..p.points.len()).filter(|&i| placer.shared[&bits(p.points[i])] > 1).collect())
            .collect();
        for zoom in feature.min_zoom..MAX_ZOOM {
            let tolerance = TOLERANCE / ((1u64 << zoom) as f64 * EXTENT as f64);
            let parts = simplified(feature.geometry, &feature.parts, &anchors, tolerance);
            placer.tiles.add(zoom, &feature.kind, feature.geometry, &feature.tags, &parts);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_reads_its_leading_number_as_parse_float_does() {
        let n = |s: &str| number(Some(&s.to_string()));
        assert_eq!((n("8 m"), n(" -2.5e1x"), n(".5"), n("3.")), (8.0, -25.0, 0.5, 3.0));
        assert!(n("m8").is_nan() && n(".").is_nan() && n("").is_nan());
        assert_eq!(n("1e"), 1.0);
    }

    #[test]
    fn the_metadata_escapes_as_python_does() {
        assert_eq!(ascii_json("© OSM \"x\""), "\"\\u00a9 OSM \\\"x\\\"\"");
        assert_eq!(python_number(&json!(7.6)), "7.6");
        assert_eq!(python_number(&json!(8.0)), "8.0");
        assert_eq!(python_number(&json!(8)), "8");
    }
}
