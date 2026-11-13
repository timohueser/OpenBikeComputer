//! The bake of a region: the features of the kind table with their heights, cut into the tiles of
//! one archive.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use obc_ground_plan::sections::{is_track, leading_number, water_width_m, WidthRules};
use serde_json::{json, Map, Value};

use crate::gate::{Census, GATE_M};
use crate::geometry::{area, bounds, contains, key, Boxes};
use crate::kinds::{kind_of, Entry, Waterway};
use crate::osm::{self, Selected, LANDUSE, LIFT_REACH_M};
use crate::pmtiles::{self, Header};
use crate::roads::{self, Classes, River, Surroundings};
use crate::structures::{self, is_bridge, GROUND};
use crate::tiles::{simplified, Geometry, Tiles, BUFFER_M, EXTENT, FORMAT, MAX_ZOOM, MIN_ZOOM, TOLERANCE};
use crate::water::{self, Kind, Level};
use crate::{lifts, metres, rails, sea, world, Part, Tags, Terrain};

const EARTH_RADIUS_KM: f64 = 6371.0088;
/// The waterways and lakes that the water solver sets, and the falls, weirs and dams that keep the
/// smoothing of a level from crossing them.
const RIVERS: [&str; 2] = ["river", "stream"];
const LAKES: [&str; 1] = ["lake"];
const MARKS: [&str; 3] = ["waterfall", "dam", "dam-point"];
/// The ways that the road solver sets the heights of, but a cable route on rock: it has no bench to stamp.
const ROADS: [&str; 6] = ["main-road", "road", "path", "railway", "tunnel", "gallery"];
/// The `tunnel` values of a waterway part that can be a culvert: as `joinCulverts` of terrain/culverts.ts.
const CULVERT_TUNNELS: [&str; 2] = ["culvert", "yes"];
/// The `surface` values of a way that is not paved, as `surfaceOf` of bins.ts reads them.
const UNPAVED: [&str; 12] = [
    "gravel",
    "compacted",
    "fine_gravel",
    "pebblestone",
    "dirt",
    "ground",
    "earth",
    "mud",
    "unpaved",
    "sand",
    "grass",
    "grass_paver",
];

/// What the step bakes: the kind table, the region, the credit, the road classes, their widths and the
/// archive of the water polygons, without which the tiles hold no sea.
pub struct Options {
    pub kinds: Vec<Entry>,
    pub bounds: [f64; 4],
    pub attribution: String,
    pub classes: Classes,
    pub class_names: Vec<String>,
    pub widths: WidthRules,
    pub waterways: BTreeMap<String, Waterway>,
    pub sea: Option<PathBuf>,
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
                *self.shared.entry(key(*point)).or_default() += 1;
            }
            self.low.push(Placed { kind: kind.to_string(), geometry, min_zoom, tags, parts });
        }
    }

    fn entry(&mut self, entry: &Entry, tags: Tags, parts: Vec<Part>) {
        self.place(&entry.kind, entry.geometry, entry.min_zoom, tags, parts);
    }
}

fn line(points: Vec<[f64; 2]>, z: Option<Vec<f64>>) -> Vec<Part> {
    vec![Part { outer: false, points, z, plan: None }]
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
    let (mut lift_ways, mut lift_entry, mut buildings, mut houses, mut landuse) =
        (Vec::new(), None, Vec::new(), Vec::new(), Vec::new());
    let (mut waters, mut ways, mut stations, mut beyond, mut river_areas, mut culverts) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    osm::objects(kinds, &cut, &binary, |(selected, mut tags, mut parts)| {
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
        if RIVERS.contains(&kind) && tags.get("tunnel").is_some_and(|t| CULVERT_TUNNELS.contains(&t.as_str())) {
            culverts.extend(parts.iter().map(|p| (kind.to_string(), tags.clone(), p.points.clone())));
        }
        if solves_road(kind, &tags) {
            beyond.push(road(kind, &tags, &parts, options));
        }
        decided(&mut tags);
        parts.iter_mut().for_each(|p| p.z = None);
        placer.entry(entry, tags, parts);
    })?;
    // The cut of a large region takes GBs, and the solvers run long after it is read.
    std::fs::remove_file(&cut).map_err(|e| e.to_string())?;
    eprintln!("osm: {:.1} s", started.elapsed().as_secs_f64());
    let values = landuse_of(&houses.iter().map(|(_, _, parts)| parts).collect::<Vec<_>>(), &landuse);
    for ((e, mut tags, parts), value) in houses.into_iter().zip(&values) {
        if let Some(value) = value {
            tags.insert("landuse".into(), value.clone());
        }
        placer.entry(&kinds[e], tags, parts);
    }
    let landuse_counts: Map<String, Value> = LANDUSE
        .iter()
        .map(|l| (l.to_string(), values.iter().filter(|v| v.as_deref() == Some(l)).count().into()))
        .collect();
    metrics.insert("landuse".into(), landuse_counts.into());
    drop(landuse);
    if let Some(e) = lift_entry {
        let (rigged, lift_metrics) = lifts::rigged(&lift_ways, &buildings, terrain);
        for (tags, points) in rigged {
            placer.entry(&kinds[e], tags, line(points, None));
        }
        metrics.insert(
            "lifts".into(),
            json!({"lifts": lift_metrics.lifts, "added_pylons": lift_metrics.added_pylons,
                   "housed_ends": lift_metrics.housed_ends, "shared_ends": lift_metrics.shared_ends}),
        );
    }
    // The sidewalks count the buildings near a road by their middles.
    let middles = buildings.iter().map(|ring| {
        let (sx, sy) = ring.iter().fold((0.0, 0.0), |(x, y), p| (x + p[0], y + p[1]));
        [sx / ring.len() as f64, sy / ring.len() as f64]
    });
    let mut around = Surroundings { buildings: rstar::RTree::bulk_load(middles.collect()), ..Default::default() };
    drop(buildings);
    eprintln!("placed: {:.1} s", started.elapsed().as_secs_f64());

    let (mut ground_census, mut water_census) = (Census::default(), Census::default());
    // The heights of a junction need every road that meets there, a river level its whole network, a
    // ford the water it crosses, and a platform its tracks.
    if !(waters.is_empty() && ways.is_empty() && stations.is_empty()) {
        let mut features: Vec<water::Feature> = waters
            .iter_mut()
            .map(|(e, tags, parts)| water_feature(&kinds[*e].kind, tags, std::mem::take(parts), options))
            .collect();
        let pool = rayon::ThreadPoolBuilder::new().build().map_err(|e| e.to_string())?;
        let (levels, drawn, _, water_metrics) = pool.install(|| water::solve_by_network(&mut features, terrain));
        // The threads of the roads take over the memory of these.
        drop(pool);
        metrics.insert(
            "water".into(),
            json!({"lakes": water_metrics.lakes, "lakes_spilling": water_metrics.lakes_spilling,
                   "river_parts": water_metrics.river_parts, "turned": water_metrics.turned,
                   "added_vertices": water_metrics.added}),
        );
        // A waterway that OSM draws uphill has turned round in `features`.
        for ((_, _, parts), (feature, z)) in waters.iter_mut().zip(features.into_iter().zip(levels)) {
            *parts = feature.parts.into_iter().zip(z).map(|(p, z)| Part { z, ..p }).collect();
        }
        eprintln!("water: {:.1} s", started.elapsed().as_secs_f64());
        let is_river = |e: usize| RIVERS.contains(&kinds[e].kind.as_str());
        around.rivers = waters
            .iter()
            .filter(|(e, _, _)| is_river(*e))
            .flat_map(|(e, tags, parts)| parts.iter().map(move |p| (kinds[*e].kind.as_str(), tags, p)))
            .filter_map(|(kind, tags, p)| Some(River { levels: p.z.clone()?, ..river(kind, tags, &p.points, options) }))
            .collect();
        around.culverts = culvert_levels(&culverts, &around.rivers, options);
        drop(culverts);
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
            &around,
            &options.classes,
            memory,
        );
        drop((road_features, around, beyond));
        eprintln!("roads: {:.1} s", started.elapsed().as_secs_f64());
        let m = &solved.metrics;
        eprintln!(
            "roads: jobs at the round cap per scale {:?}, largest move of their last round {:?} m",
            m.capped,
            m.capped_step_m.map(|v| round(v, 3))
        );
        let mut road_metrics = road_metrics(&solved.metrics);
        road_metrics.insert("cut_features".into(), cut.len().into());
        let culverts: Vec<usize> = solved.culverts.iter().map(|&i| kept[i]).collect();
        let underpasses: Vec<usize> = solved.underpasses.iter().map(|&i| kept[i]).collect();
        for (&f, parts) in kept.iter().zip(solved.ways) {
            for (part, way) in ways[f].2.iter_mut().zip(parts) {
                if let Some(way) = way {
                    (part.points, part.z, part.plan) = (way.points, Some(way.z), Some(way.plan));
                }
            }
        }
        // The roads read the levels at the map's vertices, the tiles and the census the vertices that the levels
        // add: this road solver's active set moves places far from any changed input. The cost: at a crossing a
        // road reads the straight level between two vertices of the map, which on a long span can lie metres off
        // the water of the tiles.
        for ((e, tags, parts), drawn) in waters.iter().zip(drawn) {
            let parts: Vec<Part> = (parts.iter().zip(drawn))
                .map(|(p, k)| k.map_or_else(|| p.clone(), |(points, z)| Part { points, z: Some(z), ..p.clone() }))
                .collect();
            if is_river(*e) && tags.get("bridge").is_none_or(|b| GROUND.contains(&b.as_str())) {
                water_census.add(&kinds[*e].kind, &parts, 0.0, terrain, &|_| false);
            }
            placer.entry(&kinds[*e], tags.clone(), parts);
        }
        let mut tracks: Vec<(String, Tags, Part)> = Vec::new();
        if !ways.is_empty() {
            // A culvert is a road: the stamps build it over the ground.
            for &n in &culverts {
                ways[n].1.remove("bridge");
            }
            ways.iter_mut().for_each(|(_, tags, _)| decided(tags));
            let decks: Vec<usize> = (0..ways.len())
                .filter(|&n| is_bridge(&kinds[ways[n].0].kind, &ways[n].1) && ways[n].2[0].z.is_some())
                .collect();
            let waterways: Vec<(f64, &[[f64; 2]])> = waters
                .iter()
                .filter(|(e, _, _)| is_river(*e))
                .flat_map(|(e, tags, parts)| {
                    let half = half_width(waterway(&kinds[*e].kind, tags, options), tags);
                    parts.iter().map(move |p| (half, p.points.as_slice()))
                })
                .collect();
            let areas: Vec<Vec<&[[f64; 2]]>> = waters
                .iter()
                .filter(|(e, _, _)| LAKES.contains(&kinds[*e].kind.as_str()))
                .map(|(_, _, parts)| parts.iter().map(|p| p.points.as_slice()).collect())
                .chain(river_areas.iter().map(|rings| rings.iter().map(Vec::as_slice).collect()))
                .collect();
            // A pier stands on the curve of its deck, and off the outer half-width of a road under it.
            let deck_curves: Vec<Part> = decks.iter().map(|&n| roads::polyline(&ways[n].2[0])).collect();
            let grounded =
                ways.iter().filter(|(e, tags, _)| !is_bridge(&kinds[*e].kind, tags)).flat_map(|(e, tags, parts)| {
                    let outer = options.widths.way(&kinds[*e].kind, tags, false).outer_m();
                    parts.iter().map(move |p| (outer, p))
                });
            let under = near_decks(&deck_curves, grounded);
            let under: Vec<(f64, &[[f64; 2]])> = under.iter().map(|(half, p)| (*half, p.points.as_slice())).collect();
            let deck_ways: Vec<structures::Way> = decks
                .iter()
                .zip(&deck_curves)
                .map(|(&n, curve)| {
                    let (e, tags, _) = &ways[n];
                    (kinds[*e].kind.as_str(), tags, curve.points.as_slice(), curve.z.as_deref().unwrap())
                })
                .collect();
            let (added, piers, bridge_metrics) = structures::bridges(&deck_ways, &waterways, &areas, terrain, &under);
            let mut extra: HashMap<usize, Tags> =
                decks.iter().zip(added).map(|(&n, tags)| (n, tags.unwrap_or_default())).collect();
            let (mut floating, mut shallow) = (None, None);
            // The vertices of the decks: a way on the ground that meets a deck there takes its height.
            let on_deck: HashSet<[u64; 2]> =
                decks.iter().flat_map(|&n| ways[n].2.iter().flat_map(|p| p.points.iter().map(|&q| key(q)))).collect();
            for (n, (e, tags, parts)) in ways.iter().enumerate() {
                let (mut entry, mut parts) = (&kinds[*e], parts.clone());
                if entry.kind == "gallery" && parts[0].z.is_some() {
                    let side = structures::uphill(&parts[0].points, terrain);
                    extra.insert(n, Tags::from([("uphill".to_string(), side.to_string())]));
                }
                let tunnel = entry.kind == "tunnel";
                if tunnel {
                    let count = parts.len();
                    parts.retain(|p| {
                        std::env::var_os("OBC_X_KEEPFLOAT").is_some()
                            || !p.z.as_ref().is_some_and(|z| structures::floating(&p.points, z, terrain))
                    });
                    *floating.get_or_insert(0) += count - parts.len();
                    if parts.is_empty() {
                        continue;
                    }
                }
                let heights = || parts.iter().filter_map(|p| Some((p, p.z.as_ref()?)));
                if tunnel
                    && !underpasses.contains(&n)
                    && heights().next().is_some()
                    && heights().all(|(p, z)| structures::shallow(tags, &p.points, z, terrain))
                {
                    // A tunnel under what the terrain leaves out, such as a building, is a way on the ground. It
                    // keeps its tunnel tag: the kind decides how it draws, the tag where the drawing may hide it.
                    let mut ground = tags.clone();
                    ground.insert("tunnel".into(), "no".into());
                    entry = kind_of(kinds, Geometry::Line, &ground).map_or(entry, |i| &kinds[i]);
                    *shallow.get_or_insert(0) += 1;
                }
                if entry.kind != "tunnel" && tags.get("bridge").is_none_or(|b| GROUND.contains(&b.as_str())) {
                    let half = options.widths.way(&kinds[*e].kind, tags, false).half_m;
                    ground_census.add(&entry.kind, &parts, half, terrain, &|p| on_deck.contains(&key(p)));
                }
                let mut all = tags.clone();
                all.extend(extra.get(&n).cloned().unwrap_or_default());
                if is_track(tags) {
                    let curves = parts.iter().filter(|p| p.z.is_some()).map(roads::polyline);
                    tracks.extend(curves.map(|p| (entry.kind.clone(), tags.clone(), p)));
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
        let platforms =
            rails::platforms(&stations.iter().map(|(_, _, parts)| parts.clone()).collect::<Vec<_>>(), &outside);
        let unknown = platforms.iter().filter(|p| p.is_none()).count();
        for ((e, tags, parts), solved) in stations.into_iter().zip(platforms) {
            placer.entry(&kinds[e], tags, solved.unwrap_or(parts));
        }
        let masts = placer.counts.get("catenary-mast").copied().unwrap_or(0);
        metrics.insert("rails".into(), json!({"tracks": tracks.len(), "masts": masts, "platforms_unknown": unknown}));
    }
    if let Some(zip) = &options.sea {
        for parts in sea::polygons(zip, &placer.tiles)? {
            placer.place("sea", Geometry::Area, MIN_ZOOM, Tags::new(), parts);
        }
        eprintln!("sea: {:.1} s", started.elapsed().as_secs_f64());
    }
    metrics.insert("census".into(), json!({"ground": ground_census.metrics(), "water": water_census.metrics()}));
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
    // The gate: no vertex of a way on the ground lies more than GATE_M off the terrain. The census of
    // the rivers and streams is in the metrics only.
    let past = ground_census.over() + ground_census.under();
    if past > 0 {
        return Err(format!(
            "the census gate: {past} vertices of ways on the ground lie more than {GATE_M} m off the terrain. \
             Highest: {}. Lowest: {}",
            ground_census.places(true),
            ground_census.places(false)
        ));
    }
    Ok(Value::Object(out))
}

/// The curves of the `ways`, (outer half-width, part), that can hold a pier off a deck. Only these get a
/// polyline: the ways of a region would take gigabytes of them.
fn near_decks<'a>(decks: &[Part], ways: impl Iterator<Item = (f64, &'a Part)>) -> Vec<(f64, Part)> {
    let reach = |p: &Part| {
        let [x0, y0, x1, y1] = bounds(&p.points);
        let r = structures::REACH_M / metres(y0);
        [x0 - r, y0 - r, x1 + r, y1 + r]
    };
    let boxes = Boxes::new(&decks.iter().map(reach).collect::<Vec<_>>());
    ways.filter(|(_, p)| !boxes.overlapping(bounds(&p.points)).is_empty())
        .map(|(half, p)| (half, roads::polyline(p)))
        .collect()
}

fn track((_, tags, part): &(String, Tags, Part)) -> rails::Track<'_> {
    (tags, part.points.as_slice(), part.z.as_deref().unwrap())
}

fn solves_water(kind: &str, tags: &Tags) -> bool {
    (RIVERS.contains(&kind) || LAKES.contains(&kind) || MARKS.contains(&kind))
        && !(RIVERS.contains(&kind) && tags.get("tunnel").is_some_and(|t| t != "no"))
}

/// Remove the `sidewalk` tags of a way: the alignment of its tiles holds the sidewalks that the bake decides.
fn decided(tags: &mut Tags) {
    tags.retain(|key, _| key != "sidewalk" && !key.starts_with("sidewalk:"));
}

/// Per side, left and right of a way, whether OSM tags a sidewalk there: `sidewalk`, then `sidewalk:both`,
/// then `sidewalk:left` and `sidewalk:right`; `no` and `separate` give none. A way that is no paved road,
/// a ford or a gallery has none.
fn sidewalks(kind: &str, tags: &Tags) -> [Option<bool>; 2] {
    let surface = tags.get("surface").map_or("", String::as_str);
    if kind == "gallery" || UNPAVED.contains(&surface) || tags.get("ford").is_some_and(|f| f != "no") {
        return [Some(false); 2];
    }
    let value = |key: &str| match tags.get(key).map(String::as_str) {
        Some("yes") => Some(true),
        Some("no" | "none" | "separate") => Some(false),
        _ => None,
    };
    let mut sides = match tags.get("sidewalk").map(String::as_str) {
        Some("both" | "yes") => [Some(true); 2],
        Some("left") => [Some(true), Some(false)],
        Some("right") => [Some(false), Some(true)],
        Some("no" | "none" | "separate") => [Some(false); 2],
        _ => [None; 2],
    };
    if let Some(both) = value("sidewalk:both") {
        sides = [Some(both); 2];
    }
    for (side, key) in sides.iter_mut().zip(["sidewalk:left", "sidewalk:right"]) {
        *side = value(key).or(*side);
    }
    sides
}

fn solves_road(kind: &str, tags: &Tags) -> bool {
    ROADS.contains(&kind) && tags.get("highway").is_none_or(|h| h != "via_ferrata")
}

/// The half-width of a waterway in metres, with no cap: the water solve also serves rivers wider than
/// the ground plan's widest way.
fn half_width(rules: &Waterway, tags: &Tags) -> f64 {
    water_width_m(tags, rules.width_m) / 2.0
}

/// The rules of a waterway: of its `waterway` value, else of its kind, `river` or `stream`.
fn waterway<'a>(kind: &str, tags: &Tags, options: &'a Options) -> &'a Waterway {
    let value = tags.get("waterway").filter(|w| options.waterways.contains_key(*w));
    &options.waterways[value.map_or(kind, String::as_str)]
}

/// A waterway of the road solver with no levels.
fn river(kind: &str, tags: &Tags, points: &[[f64; 2]], options: &Options) -> River {
    let rules = waterway(kind, tags, options);
    let (half, cover, lift) = (half_width(rules, tags), rules.cover_m, rules.cover_lift_max_m);
    // Water on a bridge, as an aqueduct, runs over the ways that cross it.
    let untagged_culvert = 2.0 * half <= rules.culvert_width_m && tags.get("bridge").is_none_or(|b| b == "no");
    River { points: points.to_vec(), levels: Vec::new(), half, cover, lift, untagged_culvert }
}

/// The culvert parts of `culverts` with the levels of their water. Parts that meet end to end are one
/// chain (`chains`). A chain up to `culvert_max_m` of its first part's kind long is a culvert: its water lies on
/// the straight line between the levels of the waterways of `rivers` at its two mouths, or at the level of the
/// one mouth that has one. A longer chain is a buried stream, and a culvert with neither level has none.
fn culvert_levels(culverts: &[(String, Tags, Vec<[f64; 2]>)], rivers: &[River], options: &Options) -> Vec<River> {
    let mut levels: HashMap<[u64; 2], f64> = HashMap::new();
    for river in rivers {
        for (point, &level) in river.points.iter().zip(&river.levels).filter(|(_, z)| z.is_finite()) {
            levels.entry(key(*point)).or_insert(level);
        }
    }
    let ends: Vec<[[f64; 2]; 2]> = culverts.iter().map(|(_, _, p)| [p[0], p[p.len() - 1]]).collect();
    let mut out = Vec::new();
    let stops: HashSet<[u64; 2]> = levels.keys().copied().collect();
    for chain in chains(&ends, &stops) {
        // The points of the chain in its order, and the parts with the place of their first point.
        let mut points: Vec<[f64; 2]> = Vec::new();
        let mut parts = Vec::new();
        for &(n, reversed) in &chain {
            let mut part = culverts[n].2.clone();
            if reversed {
                part.reverse();
            }
            parts.push((n, reversed, points.len().saturating_sub(1)));
            points.extend(part.into_iter().skip(usize::from(!points.is_empty())));
        }
        let at = |point: &[f64; 2]| levels.get(&key(*point)).copied();
        let (a, b) = match (at(&points[0]), at(&points[points.len() - 1])) {
            (Some(a), Some(b)) => (a, b),
            (Some(z), None) | (None, Some(z)) => (z, z),
            (None, None) => continue,
        };
        let mut along = vec![0.0];
        for pair in points.windows(2) {
            let step =
                (pair[1][0] - pair[0][0]).hypot(pair[1][1] - pair[0][1]) * metres((pair[0][1] + pair[1][1]) / 2.0);
            along.push(along[along.len() - 1] + step);
        }
        let total = along[along.len() - 1].max(1e-300);
        let (kind, tags, _) = &culverts[chain[0].0];
        if total > waterway(kind, tags, options).culvert_max_m {
            continue;
        }
        for (n, reversed, first) in parts {
            let (kind, tags, part) = &culverts[n];
            let mut z: Vec<f64> = along[first..first + part.len()].iter().map(|s| a + (b - a) * s / total).collect();
            if reversed {
                z.reverse();
            }
            out.push(River { levels: z, ..river(kind, tags, part, options) });
        }
    }
    out
}

/// The parts with these ends as chains of (part, reversed) that meet end to end. A chain stops where other
/// than two ends meet, and at a point of `stops`: where open water has a level. A loop with no such point is
/// one chain.
fn chains(ends: &[[[f64; 2]; 2]], stops: &HashSet<[u64; 2]>) -> Vec<Vec<(usize, bool)>> {
    let mut at: HashMap<[u64; 2], Vec<(usize, usize)>> = HashMap::new();
    for (n, pair) in ends.iter().enumerate() {
        for (e, point) in pair.iter().enumerate() {
            at.entry(key(*point)).or_default().push((n, e));
        }
    }
    let next = |n: usize, e: usize| {
        let point = key(ends[n][e]);
        let meeting = &at[&point];
        (meeting.len() == 2 && !stops.contains(&point))
            .then(|| meeting.iter().copied().find(|&(m, _)| m != n))
            .flatten()
    };
    let mut used = vec![false; ends.len()];
    let mut out = Vec::new();
    for first in 0..ends.len() {
        if used[first] {
            continue;
        }
        // Walk back to the start of the chain, then forward through it.
        let (mut head, mut seen) = ((first, 0), vec![first]);
        while let Some((m, e)) = next(head.0, head.1) {
            if seen.contains(&m) || used[m] {
                break;
            }
            seen.push(m);
            head = (m, 1 - e);
        }
        let mut chain = Vec::new();
        let mut piece = Some((head.0, head.1 == 1));
        while let Some((n, reversed)) = piece.filter(|p| !used[p.0]) {
            used[n] = true;
            chain.push((n, reversed));
            piece = next(n, usize::from(!reversed)).map(|(m, e)| (m, e == 1));
        }
        out.push(chain);
    }
    out
}

fn water_feature(name: &str, tags: &Tags, parts: Vec<Part>, options: &Options) -> water::Feature {
    let kind = if RIVERS.contains(&name) {
        Kind::River
    } else if LAKES.contains(&name) {
        Kind::Lake
    } else {
        Kind::Mark
    };
    let level = if tags.get("water").is_some_and(|w| w == "pond") {
        Level::Pond
    } else if tags.get("water").is_some_and(|w| w == "reservoir")
        || tags.get("landuse").is_some_and(|l| l == "reservoir")
    {
        Level::Reservoir
    } else {
        Level::Natural
    };
    let (half, bank, freeboard, curve) = if kind == Kind::River {
        let rules = waterway(name, tags, options);
        let curve = water::Curve { turn_max_deg: rules.curve_turn_max_deg, off_max_m: rules.curve_off_max_m };
        (half_width(rules, tags), rules.bank_m, rules.freeboard_m, curve)
    } else {
        (0.0, 0.0, 0.0, water::Curve::default())
    };
    water::Feature {
        kind,
        level,
        half,
        bank,
        freeboard,
        curve,
        ele: tags.get("ele").and_then(|e| leading_number(e)).unwrap_or(f64::NAN),
        parts,
    }
}

/// A way of the road solver, with the facts of its tags. Its class's rank in the width rule is its row
/// of the class table: both keep the order of `solver_rows`.
fn road(kind: &str, tags: &Tags, parts: &[Part], options: &Options) -> roads::Feature {
    let tag = |key: &str| tags.get(key).map(String::as_str);
    let free = kind == "tunnel" || tag("bridge").is_some_and(|b| !GROUND.contains(&b));
    let layer = tag("layer").and_then(|l| l.trim().parse::<i64>().ok()).unwrap_or(0);
    let widths = options.widths.way(kind, tags, false);
    let stones = kind == "path" && tag("ford") == Some("stepping_stones");
    let ford = !stones && (tag("ford").is_some_and(|f| f != "no") || tag("bridge") == Some("low_water_crossing"));
    roads::Feature {
        free,
        tunnel: kind == "tunnel",
        ford,
        stones,
        layer,
        layered: tags.contains_key("layer"),
        indoor: !free && layer < 0 && tag("tunnel").is_none_or(|t| t == "no"),
        half: widths.half_m,
        outer: widths.outer_m(),
        sidewalks: sidewalks(kind, tags),
        rule: widths.rank,
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
        "culverts": m.culverts, "rounds": m.rounds, "rounds_capped": m.capped,
        "rounds_capped_step_m": m.capped_step_m.map(|v| round(v, 3)), "held": m.held, "clearances": m.clearances,
        "clearances_low_cm": m.clearances_low_cm, "passes": m.passes, "passes_near_cm": m.passes_near_cm,
        "grade_excess_m": round(m.grade_excess_m, 4), "grade_excess_at": m.grade_excess_at.map(|v| round(v, 5)),
        "grade_excess_over_1cm": m.grade_excess_over_1cm, "grounded": m.grounded,
        "residual_p99_m": round(m.residual_p99_m, 2),
        "refinements_unconverged": m.refinements_unconverged, "covers": m.covers, "covers_lifted": m.covers_lifted,
        "covers_untagged": m.covers_untagged, "covers_untagged_lifted": m.covers_untagged_lifted,
        "covers_capped": m.covers_capped, "covers_untagged_capped": m.covers_untagged_capped,
        "cover_lift_top_m": round(m.cover_lift_top_m, 2),
        "sidewalk_tagged_m": m.sidewalks.tagged_m.round(), "sidewalk_built_m": m.sidewalks.built_m.round(),
        "sidewalk_steep_m": m.sidewalks.steep_m.round(), "sidewalk_bend_m": m.sidewalks.bend_m.round(),
        "sidewalk_bed_m": m.sidewalks.bed_m.round(), "sidewalk_short_m": m.sidewalks.short_m.round(),
    });
    value.as_object().unwrap().clone()
}

/// Per building, the `landuse` of the smallest of the `areas` that holds the middle of its outer ring,
/// or None. A point lies in an area when it lies inside an odd number of its rings.
fn landuse_of(buildings: &[&Vec<Part>], areas: &[(Tags, Vec<Part>)]) -> Vec<Option<String>> {
    let boxes: Vec<[f64; 4]> = areas.iter().map(|(_, parts)| bounds(parts.iter().flat_map(|p| &p.points))).collect();
    let boxes = Boxes::new(&boxes);
    let sizes: Vec<f64> = areas.iter().map(|(_, parts)| area(parts)).collect();
    buildings
        .iter()
        .map(|parts| {
            let ring = &parts[0].points;
            let (sx, sy) = ring.iter().fold((0.0, 0.0), |(x, y), p| (x + p[0], y + p[1]));
            let (x, y) = (sx / ring.len() as f64, sy / ring.len() as f64);
            let hits = boxes.overlapping([x, y, x, y]).into_iter().filter(|&k| {
                let rings: Vec<&[[f64; 2]]> = areas[k].1.iter().map(|p| p.points.as_slice()).collect();
                contains(&rings, [x, y])
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

/// Write the tiles to the archive at `output`; its bytes.
fn write(placer: &Placer, options: &Options, output: &Path) -> Result<u64, String> {
    let mut tiles: Vec<(u64, Vec<u8>)> = placer
        .tiles
        .tiles
        .iter()
        .map(|(&(z, x, y), tile)| (pmtiles::tile_id(z, x, y), pmtiles::gzip(&tile.bytes())))
        .collect();
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
    let metadata = json!({"features_format": FORMAT, "extent": EXTENT, "buffer_m": BUFFER_M,
                          "attribution": options.attribution, "bounds": options.bounds});
    pmtiles::write(output, &tiles, &header, &metadata.to_string())
}

/// Add the features that reach the low zooms to them, simplified at each zoom. The vertices that
/// other features share stay.
fn low_zooms(placer: &mut Placer) {
    let low = std::mem::take(&mut placer.low);
    for feature in &low {
        let anchors: Vec<Vec<usize>> = feature
            .parts
            .iter()
            .map(|p| (0..p.points.len()).filter(|&i| placer.shared[&key(p.points[i])] > 1).collect())
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

    fn tags(pairs: &[(&str, &str)]) -> Tags {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn a_road_beside_a_deck_holds_its_piers_off_and_one_far_away_gets_no_polyline() {
        let y = 0.3218;
        let unit = 1.0 / metres(y);
        let line = |north: f64, east: f64| Part {
            points: vec![[0.25, y - north * unit], [0.25 + east * unit, y - north * unit]],
            ..Part::default()
        };
        let deck = line(0.0, 120.0);
        let (beside, far) = (line(3.5, 120.0), line(300.0, 120.0));
        let near = near_decks(std::slice::from_ref(&deck), [(3.0, &beside), (3.0, &far)].into_iter());
        assert_eq!(near.len(), 1);
        let none = Tags::new();
        let ways = [("road", &none, deck.points.as_slice(), &[510.0, 510.0][..])];
        let under = [(near[0].0, near[0].1.points.as_slice())];
        let flat = |_: f64, _: f64| 500.0;
        assert!(structures::bridges(&ways, &[], &[], &flat, &under).1.is_empty(), "the road takes the whole deck");
    }

    /// The options of the committed tables, with no kinds.
    fn options() -> Options {
        let table = |text: &str| serde_json::from_str::<Value>(text).unwrap();
        let roads = crate::kinds::solver_rows(&table(include_str!("../../../data/trailview/roads.json")));
        let finishes = crate::kinds::finish_rows(&table(include_str!("../../../data/trailview/finishes.json")));
        let (class_names, classes) = crate::kinds::classes(&roads).unwrap();
        let waterways = table(include_str!("../../../data/trailview/waterways.json"));
        Options {
            kinds: Vec::new(),
            bounds: [0.0; 4],
            attribution: String::new(),
            classes,
            class_names,
            widths: crate::kinds::widths(&roads, &finishes).unwrap(),
            waterways: crate::kinds::waterways(&crate::kinds::waterway_rows(&waterways)).unwrap(),
            sea: None,
        }
    }

    #[test]
    fn a_way_of_the_solver_takes_the_shared_widths_and_the_ground_and_ford_tags_of_the_graph() {
        let options = options();
        let of = |kind: &str, pairs: &[(&str, &str)]| road(kind, &tags(pairs), &[], &options);
        let boardwalk = of("path", &[("highway", "footway"), ("bridge", "boardwalk")]);
        assert!(!boardwalk.free && !boardwalk.ford);
        let low = of("road", &[("highway", "track"), ("bridge", "low_water_crossing")]);
        assert!(!low.free && low.ford);
        let stones = of("path", &[("highway", "path"), ("ford", "stepping_stones")]);
        assert!(stones.stones && !stones.ford);
        let track = of("road", &[("highway", "track"), ("ford", "stepping_stones")]);
        assert!(!track.stones && track.ford);
        let primary = of("main-road", &[("highway", "primary")]);
        assert_eq!((primary.half, primary.outer, options.class_names[primary.rule].as_str()), (4.0, 6.0, "primary"));
        assert_eq!(of("road", &[("highway", "residential"), ("width", "60")]).half, 15.0);
        assert!(of("road", &[("highway", "residential"), ("bridge", "yes")]).free);
    }

    #[test]
    fn a_culvert_of_three_ways_takes_its_levels_between_its_two_mouths() {
        let options = options();
        // A stream flows west to east: its culvert is three ways, the middle one drawn against the flow.
        let at = |east: f64| [0.5 + east / metres(0.3), 0.3];
        let stream = tags(&[("waterway", "stream"), ("tunnel", "culvert")]);
        let pipe = tags(&[("waterway", "stream"), ("tunnel", "yes")]);
        let part = |points: Vec<[f64; 2]>| ("stream".to_string(), stream.clone(), points);
        let culverts = [part(vec![at(0.0), at(10.0)]), part(vec![at(20.0), at(10.0)]), part(vec![at(20.0), at(30.0)])];
        let open = |points: Vec<[f64; 2]>, level: f64| River {
            levels: vec![level; 2],
            ..river("stream", &stream, &points, &options)
        };
        let rivers = [open(vec![at(-10.0), at(0.0)], 10.0), open(vec![at(30.0), at(40.0)], 7.0)];
        let solved = culvert_levels(&culverts, &rivers, &options);
        let levels: Vec<Vec<f64>> =
            solved.iter().map(|r| r.levels.iter().map(|z| (z * 100.0).round() / 100.0).collect()).collect();
        assert_eq!(levels, [vec![10.0, 9.0], vec![8.0, 9.0], vec![8.0, 7.0]]);
        // Twin pipes from P to Q, and two parts with a ditch that joins between them, take the levels at their
        // own ends.
        let twins = [part(vec![at(0.0), at(20.0)]), part(vec![at(0.0), at(20.0)])];
        let rivers = [open(vec![at(-10.0), at(0.0)], 10.0), open(vec![at(20.0), at(30.0)], 8.0)];
        let twin_levels: Vec<Vec<f64>> =
            culvert_levels(&twins, &rivers, &options).iter().map(|r| r.levels.clone()).collect();
        assert_eq!(twin_levels, [vec![10.0, 8.0], vec![10.0, 8.0]]);
        let rivers = [
            open(vec![at(-10.0), at(0.0)], 10.0),
            open(vec![[0.5 + 10.0 / metres(0.3), 0.3 - 10.0 / metres(0.3)], at(10.0)], 9.5),
            open(vec![at(20.0), at(30.0)], 8.0),
        ];
        let ditch = [part(vec![at(0.0), at(10.0)]), part(vec![at(10.0), at(20.0)])];
        let ditch_levels: Vec<Vec<f64>> =
            culvert_levels(&ditch, &rivers, &options).iter().map(|r| r.levels.clone()).collect();
        assert_eq!(ditch_levels, [vec![10.0, 9.5], vec![9.5, 8.0]]);
        // A `tunnel=yes` chain of 50 m is a culvert; one of 70 m is a buried stream, with no levels.
        for (length, culvert) in [(50.0, true), (70.0, false)] {
            let pipes = [
                ("stream".to_string(), pipe.clone(), vec![at(0.0), at(length / 2.0)]),
                ("stream".to_string(), pipe.clone(), vec![at(length / 2.0), at(length)]),
            ];
            let rivers = [open(vec![at(-10.0), at(0.0)], 10.0), open(vec![at(length), at(length + 10.0)], 7.0)];
            assert_eq!(culvert_levels(&pipes, &rivers, &options).len() == 2, culvert, "{length} m");
        }
    }

    #[test]
    fn the_sidewalk_tags_decide_each_side_and_a_road_that_is_not_paved_has_none() {
        let of = |pairs: &[(&str, &str)]| sidewalks("road", &tags(pairs));
        assert_eq!(of(&[("sidewalk", "separate")]), [Some(false); 2]);
        assert_eq!(of(&[("sidewalk", "both"), ("sidewalk:right", "no")]), [Some(true), Some(false)]);
        assert_eq!(of(&[("sidewalk:left", "yes")]), [Some(true), None]);
        assert_eq!(of(&[("sidewalk", "both"), ("surface", "gravel")]), [Some(false); 2]);
        assert_eq!(of(&[]), [None; 2]);
        let mut kept = tags(&[("highway", "residential"), ("sidewalk", "left"), ("sidewalk:left", "yes")]);
        decided(&mut kept);
        assert_eq!(kept, tags(&[("highway", "residential")]));
    }
}
