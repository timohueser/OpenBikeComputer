//! The OSM objects of the step: Osmium cuts the input to the region and to what the kind table can
//! select, assembles the areas and exports the objects; this module reads them.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::kinds::{kind_of, selected_by, Entry, Values};
use crate::tiles::{Geometry, Tiles, MAX_ZOOM};
use crate::{world, Part, Tags};

/// A lift station shares its place with the buildings and the lifts near it: the near area of the
/// input reaches this far past the tiles of the region, more than the longest aerial lift.
pub const LIFT_REACH_M: f64 = 20000.0;
const METRES_PER_DEGREE: f64 = 111320.0;
/// The landuse areas whose value a building in them keeps as its tag `landuse`. They are not features.
pub const LANDUSE: [&str; 2] = ["farmyard", "industrial"];

/// The Osmium program: `OBC_OSMIUM`, else `osmium` on the path; and its version.
pub fn probe() -> Result<(PathBuf, String), String> {
    let binary = PathBuf::from(std::env::var_os("OBC_OSMIUM").unwrap_or_else(|| "osmium".into()));
    let out = Command::new(&binary)
        .arg("--version")
        .output()
        .map_err(|e| format!("Prepare Osmium, or set OBC_OSMIUM to its executable: {e}"))?;
    if !out.status.success() {
        return Err(format!("{} --version failed", binary.display()));
    }
    Ok((binary, String::from_utf8_lossy(&out.stdout).trim().to_string()))
}

fn run(binary: &Path, args: &[&dyn AsRef<std::ffi::OsStr>]) -> Result<(), String> {
    let out = Command::new(binary).args(args.iter().map(|a| a.as_ref())).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("Prepared Osmium failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}

/// The box (west, south, east, north) of the tiles of the region at `zoom`, one more tile around them
/// and `reach_m` more.
pub fn reach(tiles: &Tiles, zoom: u8, reach_m: f64) -> [f64; 4] {
    let [c0, r0, c1, r1] = tiles.span(zoom);
    let n = (1u64 << zoom) as f64;
    let lon = |x: i64| x as f64 / n * 360.0 - 180.0;
    let lat = |y: i64| (std::f64::consts::PI * (1.0 - 2.0 * y as f64 / n)).sinh().atan().to_degrees();
    let (west, north, east, south) = (lon(c0 - 1), lat(r0 - 1), lon(c1 + 2), lat(r1 + 2));
    let dlat = reach_m / METRES_PER_DEGREE;
    let dlon = dlat / (south.abs().max(north.abs()) + dlat).min(85.0).to_radians().cos();
    [(west - dlon).max(-180.0), (south - dlat).max(-85.0), (east + dlon).min(180.0), (north + dlat).min(85.0)]
}

/// The OSM of `pbf` that the bake can read, in the order of `pbf`: each object that an active kind can
/// select within one zoom-13 tile and LIFT_REACH_M of the region, and each that a kind below zoom 13
/// can select within one tile of its zoom.
pub fn selected(entries: &[Entry], pbf: &Path, binary: &Path, tiles: &Tiles, work: &Path) -> Result<PathBuf, String> {
    let active: Vec<&Entry> = entries.iter().filter(|e| e.active).collect();
    let low: Vec<&Entry> = active.iter().copied().filter(|e| e.min_zoom < MAX_ZOOM).collect();
    // A filter that keeps each object that a selector of the entries matches: by its first key.
    let expressions = |entries: &[&Entry]| -> Vec<String> {
        let firsts = entries.iter().flat_map(|e| &e.selectors).map(|s| &s[0]);
        let set: std::collections::BTreeSet<String> = firsts
            .map(|(key, values)| match values {
                Values::Any => format!("nwr/{key}"),
                Values::List(list) => format!("nwr/{key}={}", list.join(",")),
            })
            .collect();
        set.into_iter().collect()
    };
    let area = |zoom: u8, reach_m: f64| reach(tiles, zoom, reach_m).map(|v| format!("{v:.7}")).join(",");
    // The cut comes first: the filter keeps the nodes of each way that it keeps, so alone it leaves most
    // of a file.
    let cut = |name: &str, bounds: String, entries: &[&Entry], more: &[String]| -> Result<PathBuf, String> {
        let (area_pbf, kept) = (work.join(format!("{name}-area.osm.pbf")), work.join(format!("{name}.osm.pbf")));
        run(binary, &[&"extract", &"-b", &bounds, &"-s", &"smart", &pbf, &"-o", &area_pbf])?;
        let mut args: Vec<&dyn AsRef<std::ffi::OsStr>> = vec![&"tags-filter", &area_pbf];
        let filters: Vec<String> = expressions(entries).into_iter().chain(more.iter().cloned()).collect();
        args.extend(filters.iter().map(|f| f as &dyn AsRef<std::ffi::OsStr>));
        args.extend([&"-o" as &dyn AsRef<std::ffi::OsStr>, &kept]);
        run(binary, &args)?;
        std::fs::remove_file(&area_pbf).map_err(|e| e.to_string())?;
        Ok(kept)
    };
    // A building of the region keeps the landuse of the area that holds it.
    let landuse = format!("nwr/landuse={}", LANDUSE.join(","));
    let mut paths = vec![cut("near", area(MAX_ZOOM, LIFT_REACH_M), &active, &[landuse])?];
    if let Some(zoom) = low.iter().map(|e| e.min_zoom).min() {
        paths.push(cut("wide", area(zoom, 0.0), &low, &[])?);
    }
    let out = work.join("osm.pbf");
    let mut args: Vec<&dyn AsRef<std::ffi::OsStr>> = vec![&"merge"];
    args.extend(paths.iter().map(|p| p as &dyn AsRef<std::ffi::OsStr>));
    args.extend([&"-o" as &dyn AsRef<std::ffi::OsStr>, &out]);
    run(binary, &args)?;
    Ok(out)
}

/// What an exported object becomes: a feature of an entry, or a landuse area that a building takes
/// its `landuse` from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Selected {
    Entry(usize),
    Landuse,
}

/// An exported object: what it becomes, the tags that it keeps, and its parts. A ring does not repeat
/// its first point, and each outer ring precedes its holes.
pub type Object = (Selected, Tags, Vec<Part>);

/// Each object of an active kind to `each`, the points and the lines first and then the areas, each in
/// the order of the file. Osmium writes a point or a line as it reads it but buffers the areas, so one
/// export of both would put an area among the lines where its buffer ran full.
pub fn objects(entries: &[Entry], pbf: &Path, binary: &Path, mut each: impl FnMut(Object)) -> Result<(), String> {
    for types in ["point,linestring", "polygon"] {
        let mut child = Command::new(binary)
            .arg("export")
            .arg(pbf)
            .args(["-f", "geojsonseq", "--geometry-types", types, "-x", "print_record_separator=false", "-o", "-"])
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        for line in BufReader::new(child.stdout.take().unwrap()).lines() {
            let line = line.map_err(|e| e.to_string())?;
            let feature: Value = serde_json::from_str(&line).map_err(|e| format!("the export of Osmium: {e}"))?;
            if let Some(found) = object(entries, &feature) {
                each(found);
            }
        }
        if !child.wait().map_err(|e| e.to_string())?.success() {
            return Err("Prepared Osmium export failed".into());
        }
    }
    Ok(())
}

fn object(entries: &[Entry], feature: &Value) -> Option<Object> {
    let geometry = &feature["geometry"];
    let tags: Tags = feature["properties"]
        .as_object()?
        .iter()
        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
        .collect();
    let kind = match geometry["type"].as_str()? {
        "Point" => Geometry::Point,
        "LineString" => Geometry::Line,
        "MultiPolygon" => Geometry::Area,
        _ => return None,
    };
    let found = kind_of(entries, kind, &tags).map(|e| (Selected::Entry(entries.iter().position(|x| x == e).unwrap()), e));
    let (selected, keep): (Selected, Vec<String>) = match found {
        Some((_, entry)) if !entry.active => return None,
        Some((selected, entry)) => (selected, entry.keep.clone()),
        None if kind == Geometry::Area && tags.get("landuse").is_some_and(|l| LANDUSE.contains(&l.as_str())) => {
            (Selected::Landuse, vec!["landuse".to_string()])
        }
        None => return None,
    };
    let point = |v: &Value| -> Option<[f64; 2]> { Some(world(v[0].as_f64()?, v[1].as_f64()?)) };
    let coordinates = &geometry["coordinates"];
    let parts = match kind {
        Geometry::Point => vec![Part { outer: false, points: vec![point(coordinates)?], z: None }],
        Geometry::Line => {
            let line = coordinates.as_array()?;
            // Osmium gives a closed way as a line and as an area. When one tag selects it for both, as
            // `waterway=dam` does, it is an area.
            if line.first() == line.last() && tags.get("area").is_none_or(|a| a != "no") {
                if let (Some(area), Selected::Entry(e)) = (kind_of(entries, Geometry::Area, &tags), selected) {
                    if !selected_by(&entries[e], &tags).is_disjoint(&selected_by(area, &tags)) {
                        return None;
                    }
                }
            }
            vec![Part { outer: false, points: line.iter().map(point).collect::<Option<_>>()?, z: None }]
        }
        Geometry::Area => {
            let mut parts = Vec::new();
            for polygon in coordinates.as_array()? {
                for (index, ring) in polygon.as_array()?.iter().enumerate() {
                    let ring = ring.as_array()?;
                    let points = ring[..ring.len().saturating_sub(1)].iter().map(point).collect::<Option<_>>()?;
                    parts.push(Part { outer: index == 0, points, z: None });
                }
            }
            parts
        }
    };
    let kept = keep.iter().filter_map(|k| Some((k.clone(), tags.get(k)?.clone()))).collect();
    Some((selected, kept, parts))
}
