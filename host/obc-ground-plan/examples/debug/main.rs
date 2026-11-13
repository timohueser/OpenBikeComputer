//! The debug tool of the ground plan: top-down pictures, stats, a glTF height field and probe points
//! of the cells about a list of places of the TrailView fixture.

#[path = "../../tests/fixture/mod.rs"]
mod fixture;

mod draw;
mod faces;
mod gltf;
mod graph;
mod heights;
mod heights_view;
mod mesh;
mod model;
mod requests;
mod storeys;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use core::f64::consts::PI;

use obc_ground_plan::frame::{from_lon_lat, metres_per_unit, CellKey, GlobalPoint, FRAME_BITS};
use obc_ground_plan::sections::{leading_number, Sections, HALF_MAX_M};
use obc_ground_plan::terrain::Terrain;
use obc_ground_plan::tile::{features, Geometry};
use obc_ground_plan::CellInput;
use serde_json::{json, Value};

use draw::Canvas;

const SQUARE: f64 = (1u64 << FRAME_BITS) as f64;
/// A probe point lies on a level-18 grid vertex: 64 world units.
const PROBE_GRID: i64 = 64 * 16;
/// The snap of a stamp past a way's slopes, and the clearance of a probe point from an area.
const SNAP_M: f64 = 2.0;
const AREA_CLEAR_M: f64 = 5.0;
/// The kinds that carry a carriageway, and the waterways.
pub const WAYS: [&str; 6] = ["main-road", "road", "path", "railway", "tunnel", "gallery"];
pub const WATERS: [&str; 2] = ["river", "stream"];

struct Options {
    places: Option<Vec<String>>,
    views: PathBuf,
    at: Vec<(String, f64, f64)>,
    fixture: PathBuf,
    out: PathBuf,
    size_m: f64,
    px: u32,
    layers: Vec<String>,
    gltf: bool,
    probe: usize,
    census: bool,
    bench: bool,
    faces_bench: bool,
    runs: usize,
    storeys: bool,
    oblique: (f64, f64),
}

fn options() -> Result<Options, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut o = Options {
        places: None,
        views: root.join("builder/web/tests/browser/trailview/road-views-top.json"),
        at: Vec::new(),
        fixture: fixture::default_dir(),
        out: root.join(".artifacts/ground-plan/debug"),
        size_m: 128.0,
        px: 1600,
        layers: ["terrain", "inputs", "cells"].map(String::from).to_vec(),
        gltf: false,
        probe: 0,
        census: false,
        bench: false,
        faces_bench: false,
        runs: 1,
        storeys: false,
        oblique: (30.0, 35.0),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--places" => o.places = Some(value()?.split(',').map(String::from).collect()),
            "--views" => o.views = value()?.into(),
            "--at" => {
                let text = value()?;
                let (name, at) = text.split_once('=').ok_or("--at NAME=LAT,LON")?;
                let (lat, lon) = at.split_once(',').ok_or("--at NAME=LAT,LON")?;
                o.at.push((
                    name.into(),
                    lat.parse().map_err(|_| "--at: a latitude")?,
                    lon.parse().map_err(|_| "--at: a longitude")?,
                ));
            }
            "--fixture" => o.fixture = value()?.into(),
            "--out" => o.out = value()?.into(),
            "--size-m" => o.size_m = value()?.parse().map_err(|_| "--size-m: a number")?,
            "--px" => o.px = value()?.parse().map_err(|_| "--px: a number")?,
            "--layers" => o.layers = value()?.split(',').map(String::from).collect(),
            "--gltf" => o.gltf = true,
            "--census" => o.census = true,
            "--bench" => o.bench = true,
            "--faces-bench" => o.faces_bench = true,
            "--storeys" => o.storeys = true,
            "--oblique" => {
                let text = value()?;
                let (h, p) = text.split_once(',').ok_or("--oblique HEADING,PITCH")?;
                o.oblique =
                    (h.parse().map_err(|_| "--oblique: a heading")?, p.parse().map_err(|_| "--oblique: a pitch")?);
            }
            "--runs" => o.runs = value()?.parse().map_err(|_| "--runs: a count")?,
            "--probe" => o.probe = value()?.parse().map_err(|_| "--probe: a count")?,
            _ => return Err(format!("unknown option {arg}")),
        }
    }
    if let Some(layer) =
        o.layers.iter().find(|l| !["terrain", "inputs", "cells", "graph", "faces", "levels"].contains(&l.as_str()))
    {
        return Err(format!("unknown layer {layer}: the layers are terrain, inputs, cells, graph, faces, levels"));
    }
    Ok(o)
}

/// The places: each `-top` view of the views file by its name without `-top`, at the `lat` and `lon`
/// of its query, and those of `--at`.
fn places(o: &Options) -> Result<Vec<(String, f64, f64)>, String> {
    let mut chosen = named_places(&o.views, o.places.as_deref().unwrap_or_default())?;
    chosen.extend(o.at.iter().cloned());
    if chosen.is_empty() {
        return Err("no place: give --places or --at".into());
    }
    Ok(chosen)
}

/// The places of `wanted`, from the `-top` views of the views file.
pub fn named_places(views: &Path, wanted: &[String]) -> Result<Vec<(String, f64, f64)>, String> {
    let mut all = Vec::new();
    if !wanted.is_empty() {
        let text = std::fs::read_to_string(views).map_err(|e| format!("{}: {e}", views.display()))?;
        let views: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", views.display()))?;
        for view in views.as_array().into_iter().flatten() {
            let (Some(name), Some(query)) = (view["name"].as_str(), view["view"].as_str()) else { continue };
            let Some(name) = name.strip_suffix("-top") else { continue };
            let field = |key: &str| {
                query
                    .trim_start_matches('?')
                    .split('&')
                    .find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))?
                    .parse()
                    .ok()
            };
            if let (Some(lat), Some(lon)) = (field("lat"), field("lon")) {
                all.push((name.to_string(), lat, lon));
            }
        }
    }
    let mut chosen = Vec::new();
    for wanted in wanted {
        let found: Vec<_> =
            all.iter().filter(|(n, ..)| n == wanted || n.starts_with(&format!("{wanted}-"))).cloned().collect();
        if found.is_empty() {
            let names: Vec<&str> = all.iter().map(|(n, ..)| n.as_str()).collect();
            return Err(format!("no place {wanted}: the places are {}", names.join(", ")));
        }
        chosen.extend(found);
    }
    Ok(chosen)
}

/// The cells of the square of `size_m` about a place.
pub fn square_cells(lat: f64, lon: f64, size_m: f64) -> Vec<CellKey> {
    let centre = from_lon_lat(lon, lat);
    let half = size_m / 2.0 / metres_per_unit(centre.y);
    let corner = |x: f64, y: f64| CellKey::of(GlobalPoint { x: x as i64, y: y as i64 });
    let (a, b) = (
        corner(centre.x as f64 - half, centre.y as f64 - half),
        corner(centre.x as f64 + half, centre.y as f64 + half),
    );
    (a.y..=b.y).flat_map(|y| (a.x..=b.x).map(move |x| CellKey { x, y })).collect()
}

/// Longitude and latitude of a frame point, for the probe file.
pub fn lon_lat(p: GlobalPoint) -> (f64, f64) {
    let lat = libm::atan(libm::sinh(PI * (1.0 - 2.0 * p.y as f64 / SQUARE)));
    (p.x as f64 / SQUARE * 360.0 - 180.0, lat.to_degrees())
}

fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        Some("requests") => requests::run(),
        Some("model") => model::run(),
        Some("heights") => heights::run(),
        Some("mesh") => mesh::run(),
        _ => run(),
    };
    if let Err(e) = result {
        eprintln!("debug: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--sheet" {
        return sheet(&args[2]);
    }
    let o = options()?;
    std::fs::create_dir_all(&o.out).map_err(|e| format!("{}: {e}", o.out.display()))?;
    let mut fix = fixture::Fixture::open(&o.fixture);
    let sections = Sections::embedded();
    if o.census || o.bench || o.faces_bench {
        let cells = graph::fixture_cells(&fix, &o.fixture);
        let rules = obc_ground_plan::faces::Rules::embedded();
        if o.faces_bench {
            let path = o.out.join("faces-bench.json");
            let bench = faces::bench(&mut fix, &cells, rules, o.runs);
            write(&path, serde_json::to_string_pretty(&bench).unwrap().as_bytes())?;
            println!("faces bench of {} cells -> {}", cells.len(), path.display());
        }
        if o.census {
            let structures = &obc_ground_plan::faces::Rules::embedded().structures;
            let mut levels = serde_json::Map::new();
            let mut graphs: BTreeMap<CellKey, _> = BTreeMap::new();
            for &k in &cells {
                let input = fix.cell(k);
                let g = obc_ground_plan::graph::build(k, &input, sections);
                let mut lv = g.clone();
                obc_ground_plan::graph::levels(&mut lv, &input, sections, structures);
                levels.insert(format!("{}/{}", k.x, k.y), fixture::levels_detail(&lv, &input.terrain));
                graphs.insert(k, g);
            }
            let path = o.out.join("junctions.json");
            write(&path, serde_json::to_string_pretty(&graph::census(&graphs, sections)).unwrap().as_bytes())?;
            write(&o.out.join("levels.json"), serde_json::to_string(&levels).unwrap().as_bytes())?;
            println!("census of {} cells -> {}", cells.len(), path.display());
        }
        if o.bench {
            let path = o.out.join("graph-bench.json");
            let bench = graph::bench(&mut fix, &cells, sections, 5);
            write(&path, serde_json::to_string_pretty(&bench).unwrap().as_bytes())?;
            println!(
                "bench of {} cells: {}, first {} -> {}",
                cells.len(),
                bench["ms"],
                bench["first_ms"],
                path.display()
            );
        }
        if o.places.is_none() && o.at.is_empty() {
            return Ok(());
        }
    }
    let places = places(&o)?;
    for (name, lat, lon) in places {
        let centre = from_lon_lat(lon, lat);
        let mpu = metres_per_unit(centre.y);
        let half = o.size_m / 2.0 / mpu;
        let square = [centre.x as f64 - half, centre.y as f64 - half, centre.x as f64 + half, centre.y as f64 + half];
        let keys = square_cells(lat, lon, o.size_m);
        let mut tiles = BTreeSet::new();
        let mut cells = BTreeMap::new();
        let mut terrain_ms = serde_json::Map::new();
        for &key in &keys {
            tiles.extend(key.feature_tiles());
            let pages = fix.pages(&Terrain::wanted(key));
            let start = Instant::now();
            drop(Terrain::new(key, pages));
            terrain_ms.insert(format!("{}/{}", key.x, key.y), json!(start.elapsed().as_secs_f64() * 1e3));
            let cell = fix.try_cell(key).map_err(|e| format!("{name}: a tile of cell {}/{}: {e}", key.x, key.y))?;
            cells.insert(key, cell);
        }
        // The decode of each tile read, once more for its time; and the counts of its features.
        let mut decode_ms = serde_json::Map::new();
        let mut counts: BTreeMap<String, [usize; 3]> = BTreeMap::new();
        let mut aligned = 0;
        for &(column, row) in &tiles {
            let Some(bytes) = fix.tile(column, row) else { continue };
            let start = Instant::now();
            let decoded = features(&bytes, column, row).map_err(|e| format!("{name}: tile 13/{column}/{row}: {e}"))?;
            decode_ms.insert(format!("13/{column}/{row}"), json!(start.elapsed().as_secs_f64() * 1e3));
            for f in &decoded {
                let count = counts.entry(format!("{} {:?}", f.kind, f.geometry)).or_default();
                count[0] += 1;
                count[1] += f.parts.len();
                count[2] += f.parts.iter().map(|p| p.points.len()).sum::<usize>();
                aligned += f.parts.iter().filter(|p| p.plan.is_some()).count();
            }
        }
        let mut canvas = Canvas::new(o.px, square[0], square[1], 2.0 * half / f64::from(o.px), mpu);
        let heights = draw::heights(&canvas, &cells);
        if o.layers.iter().any(|l| l == "terrain") {
            draw::terrain(&mut canvas, &heights.values);
        }
        if o.layers.iter().any(|l| l == "inputs") {
            draw::inputs(&mut canvas, &cells, sections);
        }
        if o.layers.iter().any(|l| l == "cells") {
            draw::cells(&mut canvas, &keys, centre);
        }
        // Under the graph, so that the centre lines show over the faces.
        let mut corners = Value::Null;
        if o.layers.iter().any(|l| l == "faces") {
            let rules = obc_ground_plan::faces::Rules::embedded();
            let plans = faces::plans(&cells, rules);
            faces::layer(&mut canvas, &plans, &cells, rules);
            corners = faces::corners(&canvas, &plans);
        }
        // Over the cells' marks, so that a crossing's letter at the centre shows.
        let mut points = Value::Null;
        if o.layers.iter().any(|l| l == "graph" || l == "levels") {
            let structures = &obc_ground_plan::faces::Rules::embedded().structures;
            let graphs: BTreeMap<CellKey, _> = (cells.iter())
                .map(|(&k, input)| {
                    let mut g = obc_ground_plan::graph::build(k, input, sections);
                    obc_ground_plan::graph::levels(&mut g, input, sections, structures);
                    (k, g)
                })
                .collect();
            if o.layers.iter().any(|l| l == "levels") {
                graph::levels(&mut canvas, &graphs);
            }
            if o.layers.iter().any(|l| l == "graph") {
                graph::layer(&mut canvas, &graphs);
                points = graph::points(&canvas, &graphs, sections);
            }
        }
        let png = o.out.join(format!("{name}.png"));
        // The background is opaque, so the premultiplied pixels are the straight ones.
        let (w, h) = (canvas.pixmap.width(), canvas.pixmap.height());
        image::save_buffer(&png, canvas.pixmap.data(), w, h, image::ColorType::Rgba8)
            .map_err(|e| format!("{}: {e}", png.display()))?;
        let capped: BTreeSet<String> = (cells.values().flat_map(|c| &c.features))
            .filter(|f| {
                f.geometry == Geometry::Line && (WAYS.contains(&f.kind.as_str()) || WATERS.contains(&f.kind.as_str()))
            })
            .filter(|f| f.tags.get("width").and_then(|w| leading_number(w)).is_some_and(|w| w / 2.0 > HALF_MAX_M))
            .map(|f| format!("{} {:?}", f.kind, f.tags))
            .collect();
        let stats = json!({
            "place": name,
            "lat": lat,
            "lon": lon,
            "size_m": o.size_m,
            "cells": keys.iter().map(|k| [k.x, k.y]).collect::<Vec<_>>(),
            "tiles": tiles.iter().map(|&(c, r)| [c, r]).collect::<Vec<_>>(),
            "counts": counts.iter().map(|(k, [f, p, v])| (k.clone(), json!({"features": f, "parts": p, "vertices": v}))).collect::<serde_json::Map<_, _>>(),
            "aligned_parts": aligned,
            "unknown_px": heights.unknown,
            "height_range_m": heights.range,
            "capped_widths": capped,
            "times": {"decode_ms": decode_ms, "terrain_build_ms": terrain_ms, "height_ns": heights.ns},
            "graph": points,
            "corners": corners,
        });
        write(&o.out.join(format!("{name}.json")), serde_json::to_string_pretty(&stats).unwrap().as_bytes())?;
        if o.gltf {
            let mesh = gltf::terrain(&cells, centre, o.size_m);
            write(&o.out.join(format!("{name}.glb")), &gltf::glb(&[mesh]))?;
        }
        if o.storeys {
            let rules = obc_ground_plan::faces::Rules::embedded();
            let built = storeys::cells(&mut fix, &keys, rules)?;
            let mut top = Canvas::new(o.px, square[0], square[1], 2.0 * half / f64::from(o.px), mpu);
            draw::terrain(&mut top, &heights.values);
            storeys::top(&mut top, &built);
            if o.layers.iter().any(|l| l == "inputs") {
                draw::inputs(&mut top, &cells, sections);
            }
            draw::cells(&mut top, &keys, centre);
            let path = o.out.join(format!("{name}-storeys.png"));
            storeys::save(&top.pixmap, &path)?;
            storeys::oblique(&o.out.join(format!("{name}-oblique.png")), &built, &cells, centre, o.size_m, o.oblique)?;
            let stats = storeys::stats(&built);
            write(
                &o.out.join(format!("{name}-storeys.json")),
                serde_json::to_string_pretty(&stats).unwrap().as_bytes(),
            )?;
            if o.gltf {
                write(
                    &o.out.join(format!("{name}-storeys.glb")),
                    &gltf::glb(&storeys::gltf_meshes(&built, &cells, centre, o.size_m)),
                )?;
            }
            println!("{name}: storeys {}", serde_json::to_string(&stats["triangles"]).unwrap());
        }
        if o.probe > 0 {
            let (points, free) = probe(&cells, sections, square, o.probe);
            let json = json!({"place": name, "wanted": o.probe, "free": free, "points": points});
            write(&o.out.join(format!("{name}.probe.json")), serde_json::to_string_pretty(&json).unwrap().as_bytes())?;
        }
        println!(
            "{name}: {} cells, {} tiles, {} px unknown -> {}",
            keys.len(),
            tiles.len(),
            heights.unknown,
            png.display()
        );
    }
    Ok(())
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Distance in frame units from p to the segment a–b.
fn distance(p: GlobalPoint, a: GlobalPoint, b: GlobalPoint) -> f64 {
    let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
    let (px, py) = ((p.x - a.x) as f64, (p.y - a.y) as f64);
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 { ((px * dx + py * dy) / length).clamp(0.0, 1.0) } else { 0.0 };
    let (ex, ey) = (px - t * dx, py - t * dy);
    (ex * ex + ey * ey).sqrt()
}

/// Up to `count` points [lon, lat, height] on level-18 grid vertices of the square, free of every
/// stamp: past each way's or water's half-width, shoulder or sidewalk, reach and the snap, and
/// `AREA_CLEAR_M` outside every area. Evenly picked from the free ones, row by row; and the count of
/// the free ones.
fn probe(
    cells: &BTreeMap<CellKey, CellInput>,
    sections: &Sections,
    square: [f64; 4],
    count: usize,
) -> (Vec<[f64; 3]>, usize) {
    let grid = |v: f64| (v / PROBE_GRID as f64).ceil() as i64 * PROBE_GRID;
    let mut free = Vec::new();
    let mut y = grid(square[1]);
    while (y as f64) < square[3] {
        let mut x = grid(square[0]);
        while (x as f64) < square[2] {
            let p = GlobalPoint { x, y };
            let cell = &cells[&CellKey::of(p)];
            let mpu = metres_per_unit(y);
            if let Some(h) = cell.terrain.height(p).filter(|_| clear(cell, sections, p, mpu)) {
                free.push((p, h));
            }
            x += PROBE_GRID;
        }
        y += PROBE_GRID;
    }
    let picked = count.min(free.len());
    let points = (0..picked)
        .map(|k| {
            let (p, h) = free[k * free.len() / picked];
            let (lon, lat) = lon_lat(p);
            [lon, lat, h]
        })
        .collect();
    (points, free.len())
}

fn clear(cell: &CellInput, sections: &Sections, p: GlobalPoint, mpu: f64) -> bool {
    for f in &cell.features {
        let reach_m = match (f.geometry, f.kind.as_str()) {
            (Geometry::Line, kind) if WAYS.contains(&kind) => {
                let s = sections.way(kind, &f.tags, false);
                s.half_m + s.shoulder_m.max(s.sidewalk_m) + s.reach_m + SNAP_M
            }
            (Geometry::Line, kind) if WATERS.contains(&kind) => {
                let s = sections.water(kind, &f.tags);
                s.half_m + s.reach_m + SNAP_M
            }
            (Geometry::Line, _) => AREA_CLEAR_M,
            (Geometry::Area, _) => AREA_CLEAR_M,
            (Geometry::Point, _) => continue,
        };
        let reach = reach_m / mpu;
        // An area's copy from the tile that owns the point: the joins of the other copies' cuts are not its edges.
        for part in f.parts.iter().filter(|part| f.geometry != Geometry::Area || draw::holds(part.tile, p)) {
            let n = part.points.len();
            let edges = if f.geometry == Geometry::Area { n } else { n.saturating_sub(1) };
            if (0..edges).any(|i| distance(p, part.points[i], part.points[(i + 1) % n]) < reach) {
                return false;
            }
        }
        if f.geometry == Geometry::Area && draw::inside(f, p) {
            return false;
        }
    }
    true
}

/// `--sheet COLUMNS,out.png,a.png,b.png,...`: the pictures in rows of COLUMNS, with white gaps.
fn sheet(names: &str) -> Result<(), String> {
    let names: Vec<&str> = names.split(',').collect();
    let (Some(cols), Some(out)) = (names.first().and_then(|c| c.parse::<usize>().ok()), names.get(1)) else {
        return Err("--sheet COLUMNS,out.png,a.png,...".into());
    };
    let open = |n: &&str| image::open(n).map(|i| i.to_rgba8()).map_err(|e| format!("{n}: {e}"));
    let pics = names[2..].iter().map(open).collect::<Result<Vec<_>, _>>()?;
    let (w, h) =
        (pics.iter().map(|p| p.width()).max().unwrap_or(0), pics.iter().map(|p| p.height()).max().unwrap_or(0));
    let (cols, rows) = (cols.max(1) as u32, pics.len().div_ceil(cols.max(1)) as u32);
    let mut img = image::RgbaImage::from_pixel(cols * (w + 12) - 12, rows * (h + 12) - 12, image::Rgba([255; 4]));
    for (k, p) in pics.iter().enumerate() {
        let (c, r) = (k as u32 % cols, k as u32 / cols);
        image::imageops::overlay(&mut img, p, i64::from(c * (w + 12)), i64::from(r * (h + 12)));
    }
    img.save(out).map_err(|e| format!("{out}: {e}"))
}
