//! The job file of the solvers and the file of their heights, as `tools/trailview/solvers.py` writes
//! and reads them. Numbers are little-endian; a point is two f64 world coordinates.
//!
//! The job: a u64 length and a JSON header (`terrain`: `{"grid": {path: file}}` or `{"mbtiles":
//! path}`; `classes`: per class `grade` (null for none), `smooth_m` and `rail`; `path`, the first
//! class of a path; `inside`, the box [x0, y0, x1, y1] past which the input cuts a chain of bridges or
//! tunnels; `memory`, the bytes that the road jobs may take). Then the waterways and lakes: a u64
//! count, and per feature a u8 kind (0 river, 1 lake, 2 mark), a u8 level (0 natural, 1 pond,
//! 2 reservoir), f64 half-width, f64 `ele` (NaN for none), a u32 count of parts, and per part a u8
//! outer, a u32 count of points and the points. Then the roads: a u64 count of the features to solve
//! and a u64 count of the features past the region, and per feature a u8 of flags (1 bridge or
//! tunnel, 2 tunnel, 4 ford, 8 layered, 16 indoor), a u32 class, an i64 layer, an f64 half-width, a
//! u32 count of parts, and per part a u32 count of points and the points.
//!
//! The heights: a u64 length and a JSON header (`water` and `roads`: the metrics; `culverts`,
//! `underpasses` and `cut`: numbers of road features). Then per waterway and lake, per part, a u8
//! that is 1 where the part turned round, an i64 count of heights (−1 for none) and the heights. Then
//! per road feature, per part, an i64 count of vertices (−1 for none) and per vertex x, y and z. A
//! height is NaN where it is unknown.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{json, Value};

use crate::roads::{self, Class, Classes, River};
use crate::terrain::Tiles;
use crate::water::{self, Kind, Level};
use crate::Part;

pub struct Job {
    pub header: Value,
    pub water: Vec<water::Feature>,
    pub roads: Vec<roads::Feature>,
    pub beyond: Vec<roads::Feature>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let bytes = self.bytes.get(self.at..self.at + N).ok_or("the job file ends early")?;
        self.at += N;
        Ok(bytes.try_into().unwrap())
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take::<1>()?[0])
    }
    fn u32(&mut self) -> Result<usize, String> {
        Ok(u32::from_le_bytes(self.take()?) as usize)
    }
    fn u64(&mut self) -> Result<usize, String> {
        Ok(u64::from_le_bytes(self.take()?) as usize)
    }
    fn i64(&mut self) -> Result<i64, String> {
        Ok(i64::from_le_bytes(self.take()?))
    }
    fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_le_bytes(self.take()?))
    }
    fn points(&mut self) -> Result<Vec<[f64; 2]>, String> {
        (0..self.u32()?).map(|_| Ok([self.f64()?, self.f64()?])).collect()
    }
    fn road(&mut self) -> Result<roads::Feature, String> {
        let flags = self.u8()?;
        let (rule, layer, half) = (self.u32()?, self.i64()?, self.f64()?);
        let parts = (0..self.u32()?).map(|_| self.points()).collect::<Result<_, _>>()?;
        let flag = |bit: u8| flags & bit != 0;
        Ok(roads::Feature {
            free: flag(1),
            tunnel: flag(2),
            ford: flag(4),
            layered: flag(8),
            indoor: flag(16),
            layer,
            half,
            rule,
            parts,
        })
    }
}

impl Job {
    pub fn read(path: &Path) -> Result<Job, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut r = Reader { bytes: &bytes, at: 0 };
        let length = r.u64()?;
        let header = r.bytes.get(8..8 + length).ok_or("the job file ends early")?;
        let header: Value = serde_json::from_slice(header).map_err(|e| format!("the header of the job: {e}"))?;
        r.at += length;
        let water = (0..r.u64()?)
            .map(|_| {
                let kind = [Kind::River, Kind::Lake, Kind::Mark][r.u8()? as usize];
                let level = [Level::Natural, Level::Pond, Level::Reservoir][r.u8()? as usize];
                let (half, ele) = (r.f64()?, r.f64()?);
                let parts = (0..r.u32()?)
                    .map(|_| Ok(Part { outer: r.u8()? != 0, points: r.points()? }))
                    .collect::<Result<_, String>>()?;
                Ok(water::Feature { kind, level, half, ele, parts })
            })
            .collect::<Result<_, String>>()?;
        let (count, past) = (r.u64()?, r.u64()?);
        let roads = (0..count).map(|_| r.road()).collect::<Result<_, _>>()?;
        let beyond = (0..past).map(|_| r.road()).collect::<Result<_, _>>()?;
        Ok(Job { header, water, roads, beyond })
    }

    fn terrain(&self) -> Result<Tiles, String> {
        let terrain = &self.header["terrain"];
        if let Some(path) = terrain["mbtiles"].as_str() {
            return Tiles::mbtiles(Path::new(path));
        }
        let files: BTreeMap<String, PathBuf> =
            serde_json::from_value(terrain["grid"].clone()).map_err(|e| format!("the terrain of the job: {e}"))?;
        Tiles::grid(&files)
    }

    fn classes(&self) -> Result<Classes, String> {
        let rows = self.header["classes"].as_array().ok_or("the job has no classes")?;
        let rows = rows
            .iter()
            .map(|row| Class {
                grade: row["grade"].as_f64().unwrap_or(f64::INFINITY),
                smooth_m: row["smooth_m"].as_f64().unwrap_or_default(),
                rail: row["rail"].as_bool().unwrap_or_default(),
            })
            .collect();
        Ok(Classes { rows, path: self.header["path"].as_u64().ok_or("the job has no path class")? as usize })
    }

    /// Solve the job: the levels of the water, then the heights of the roads with the rivers for their
    /// fords and decks. Writes the heights to `out` and the time of each phase to standard error.
    pub fn run(mut self, out: &Path) -> Result<(), String> {
        let terrain = self.terrain()?;
        let classes = self.classes()?;
        let cores = std::thread::available_parallelism().map_or(1, usize::from);
        let started = Instant::now();
        let pool = rayon::ThreadPoolBuilder::new().num_threads(cores).build().map_err(|e| e.to_string())?;
        let (levels, turned, water_metrics) = pool.install(|| water::solve_by_network(&mut self.water, &terrain));
        // The threads of the roads take over the memory of these.
        drop(pool);
        eprintln!("water: {:.1} s on {cores} threads", started.elapsed().as_secs_f64());
        let rivers: Vec<River> = self
            .water
            .iter()
            .zip(&levels)
            .filter(|(feature, _)| feature.kind == Kind::River)
            .flat_map(|(feature, parts)| feature.parts.iter().zip(parts))
            .filter_map(|(part, levels)| Some(River { points: part.points.clone(), levels: levels.clone()? }))
            .collect();
        let started = Instant::now();
        let inside: Vec<f64> = serde_json::from_value(self.header["inside"].clone()).map_err(|e| e.to_string())?;
        let within = |[x, y]: [f64; 2]| x > inside[0] && x < inside[2] && y > inside[1] && y < inside[3];
        let (context, cut) = roads::context(&self.roads, &self.beyond, &within);
        let kept: Vec<usize> = (0..self.roads.len()).filter(|f| !cut.contains(f)).collect();
        let features: Vec<&roads::Feature> = kept.iter().map(|&f| &self.roads[f]).collect();
        let context: Vec<&roads::Feature> = context.iter().map(|&f| &self.beyond[f]).collect();
        let memory = self.header["memory"].as_u64().unwrap_or(u64::MAX) as usize;
        let solved = roads::solve_by_area(&features, &context, &terrain, &rivers, &classes, memory);
        eprintln!("roads: {:.1} s", started.elapsed().as_secs_f64());
        let mut heights: roads::Heights = self.roads.iter().map(|f| vec![None; f.parts.len()]).collect();
        for (&f, parts) in kept.iter().zip(solved.heights) {
            heights[f] = parts;
        }
        let m = &solved.metrics;
        let header = json!({
            "water": {"lakes": water_metrics.lakes, "lakes_spilling": water_metrics.lakes_spilling,
                      "river_parts": water_metrics.river_parts, "turned": water_metrics.turned},
            "roads": {"ways": m.ways, "variables": m.variables, "known": m.known, "approaches": m.approaches,
                      "fords": m.fords, "culverts": m.culverts, "rounds": m.rounds, "held": m.held,
                      "clearances": m.clearances, "clearances_low_cm": m.clearances_low_cm, "passes": m.passes,
                      "passes_near_cm": m.passes_near_cm, "grade_excess_m": m.grade_excess_m,
                      "grade_excess_at": m.grade_excess_at, "grade_excess_over_1cm": m.grade_excess_over_1cm,
                      "residual_p99_m": m.residual_p99_m, "refinements_unconverged": m.refinements_unconverged,
                      "cut_features": cut.len()},
            "culverts": solved.culverts.iter().map(|&i| kept[i]).collect::<Vec<_>>(),
            "underpasses": solved.underpasses.iter().map(|&i| kept[i]).collect::<Vec<_>>(),
        });
        let mut bytes = Vec::new();
        let header = header.to_string();
        bytes.extend((header.len() as u64).to_le_bytes());
        bytes.extend(header.as_bytes());
        for (f, parts) in levels.iter().enumerate() {
            for (p, part) in parts.iter().enumerate() {
                bytes.push(u8::from(turned.binary_search(&(f, p)).is_ok()));
                let z = part.as_deref().unwrap_or_default();
                bytes.extend(part.as_ref().map_or(-1, |z| z.len() as i64).to_le_bytes());
                z.iter().for_each(|v| bytes.extend(v.to_le_bytes()));
            }
        }
        for parts in &heights {
            for part in parts {
                bytes.extend(part.as_ref().map_or(-1, |(points, _)| points.len() as i64).to_le_bytes());
                for (point, z) in part.iter().flat_map(|(points, z)| points.iter().zip(z)) {
                    [point[0], point[1], *z].iter().for_each(|v| bytes.extend(v.to_le_bytes()));
                }
            }
        }
        let mut file = std::fs::File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
        file.write_all(&bytes).map_err(|e| format!("{}: {e}", out.display()))
    }
}
