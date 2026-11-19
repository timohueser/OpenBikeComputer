//! The shares of the length of each route, from its samples: paved, near a big road, under
//! canopy, along water.

use obc_data::pmtiles::Archive;
use planner_router::shape::P;

use super::{Grid, Route};
use crate::grid::mercator;
use crate::source::{Table, Ways};

/// The nearest highway decides whether a sample is paved, within this distance in metres.
const PAVED_M: f64 = 15.0;
const BIG_ROAD_M: f64 = 200.0;
const WATER_M: f64 = 100.0;
/// The zoom of the canopy archive, and the cells of a tile on each side.
const CANOPY_ZOOM: u8 = 12;
const CANOPY_CELLS: f64 = 256.0;
/// A canopy cell with this cover code (× 10 %) or more is forest for the share.
const COVER: u16 = 5;

/// How a highway is paved: by its `surface`, or by its class while it has no `surface`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Paved {
    No,
    Surface,
    Class,
}

pub(crate) fn paved(highway: &str, surface: Option<&str>) -> Paved {
    let roads = ["motorway", "trunk", "primary", "secondary", "tertiary", "unclassified", "residential"];
    match surface {
        Some("asphalt" | "concrete" | "paving_stones" | "sett") => Paved::Surface,
        Some(_) => Paved::No,
        None if roads.contains(&highway.trim_end_matches("_link")) => Paved::Class,
        None => Paved::No,
    }
}

#[derive(Clone, Copy)]
struct Sample {
    /// The distance to the nearest highway within `PAVED_M`, and how that highway is paved.
    highway: Option<(f64, Paved)>,
    big_road: bool,
    water: bool,
    /// Whether the sample is in a canopy cell of `COVER` or more; None where the archive does not
    /// know the cell.
    canopy: Option<bool>,
}

/// The samples of every route in one grid, and what lies near each.
pub struct Samples {
    grid: Grid,
    points: Vec<P>,
    samples: Vec<Sample>,
    /// The first sample of each route, and the end of the last.
    starts: Vec<usize>,
    canopy: bool,
}

/// The shares of one route in whole percent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shares {
    pub paved: u8,
    pub big_road: u8,
    pub water: u8,
    /// None without the canopy archive, or when it does not know the cell of a sample.
    pub canopy: Option<u8>,
}

impl Samples {
    pub fn new(routes: &[Route], lat: f64) -> Self {
        let mut grid = Grid::new(BIG_ROAD_M, lat);
        let (mut points, mut starts) = (Vec::new(), vec![0]);
        for route in routes {
            for &p in &route.samples {
                grid.insert(p, p, points.len() as u32);
                points.push(p);
            }
            starts.push(points.len());
        }
        let empty = Sample { highway: None, big_road: false, water: false, canopy: None };
        Self { grid, samples: vec![empty; points.len()], points, starts, canopy: false }
    }

    /// Reads the highways of `ways`: the surface of the nearest, and the big roads.
    pub fn highways(&mut self, ways: &Table<Ways>) {
        for way in ways.iter() {
            let Some(highway) = way.tags.get("highway") else { continue };
            for run in way.points.split(|node| !node.present()) {
                let line: Vec<P> = run.iter().map(|node| node.point()).map(|p| [p.lon, p.lat]).collect();
                self.road(highway, way.tags.get("surface"), &line);
            }
        }
    }

    pub(crate) fn road(&mut self, highway: &str, surface: Option<&str>, line: &[P]) {
        let paved = paved(highway, surface);
        let big = matches!(highway, "motorway" | "trunk" | "primary");
        let reach = if big { BIG_ROAD_M } else { PAVED_M };
        for pair in line.windows(2) {
            let samples = &mut self.samples;
            self.grid.near(pair[0], pair[1], reach, |i, metres| {
                let sample = &mut samples[i as usize];
                sample.big_road |= big;
                if metres <= PAVED_M && sample.highway.is_none_or(|(nearest, _)| metres < nearest) {
                    sample.highway = Some((metres, paved));
                }
            });
        }
    }

    /// Reads the lines of the water.
    pub fn water(&mut self, lines: &[Vec<P>]) {
        for pair in lines.iter().flat_map(|line| line.windows(2)) {
            let samples = &mut self.samples;
            self.grid.near(pair[0], pair[1], WATER_M, |i, _| samples[i as usize].water = true);
        }
    }

    /// Reads the cell of each sample in the canopy archive, one tile at a time.
    pub fn canopy(&mut self, archive: &Archive) -> Result<(), String> {
        self.canopy = true;
        let mut cells: Vec<((u32, u32), usize, usize)> = (0..self.points.len())
            .map(|i| {
                let p = self.points[i];
                let (x, y) = mercator(p[0] as f64 * 1e-6, p[1] as f64 * 1e-6, CANOPY_ZOOM.into());
                let cell = |v: f64| ((v.fract() * CANOPY_CELLS) as usize).min(CANOPY_CELLS as usize - 1);
                ((x as u32, y as u32), cell(y) * CANOPY_CELLS as usize + cell(x), i)
            })
            .collect();
        cells.sort_unstable();
        for tile in cells.chunk_by(|a, b| a.0 == b.0) {
            let (x, y) = tile[0].0;
            let codes = match archive.get(CANOPY_ZOOM, x, y)? {
                Some(bytes) => codes(&bytes)?,
                None => Vec::new(),
            };
            for &(_, cell, i) in tile {
                self.samples[i].canopy = codes.get(cell).filter(|&&code| code != u16::MAX).map(|&code| forest(code));
            }
        }
        Ok(())
    }

    /// The count of the samples, of those that are paved, and of those paved by the class of a
    /// highway without `surface`.
    pub fn paved(&self) -> (usize, usize, usize) {
        let by = |test: &dyn Fn(Paved) -> bool| {
            self.samples.iter().filter(|s| s.highway.is_some_and(|(_, paved)| test(paved))).count()
        };
        (self.samples.len(), by(&|paved| paved != Paved::No), by(&|paved| paved == Paved::Class))
    }

    /// The shares of each route, in the order of the routes.
    pub fn shares(&self) -> Vec<Shares> {
        let percent = |count: usize, of: usize| (100.0 * count as f64 / of.max(1) as f64).round() as u8;
        self.starts
            .windows(2)
            .map(|range| {
                let samples = &self.samples[range[0]..range[1]];
                let count =
                    |test: &dyn Fn(&Sample) -> bool| percent(samples.iter().filter(|s| test(s)).count(), samples.len());
                let canopy = samples.iter().map(|sample| sample.canopy).collect::<Option<Vec<bool>>>();
                Shares {
                    paved: count(&|s| s.highway.is_some_and(|(_, paved)| paved != Paved::No)),
                    big_road: count(&|s| s.big_road),
                    water: count(&|s| s.water),
                    canopy: canopy
                        .filter(|_| self.canopy)
                        .map(|forest| percent(forest.iter().filter(|&&f| f).count(), forest.len())),
                }
            })
            .collect()
    }
}

/// A canopy code is forest of `COVER` or more: its cover bits 7 to 10, while it is a forest cell.
pub(crate) fn forest(code: u16) -> bool {
    code != 0 && (code >> 7) & 0xf >= COVER
}

/// The code of each cell of a canopy tile, row by row: red + 256 × green.
fn codes(webp: &[u8]) -> Result<Vec<u16>, String> {
    let image = image::load_from_memory_with_format(webp, image::ImageFormat::WebP).map_err(|e| e.to_string())?;
    Ok(image.to_rgb8().pixels().map(|pixel| u16::from(pixel[0]) | u16::from(pixel[1]) << 8).collect())
}
