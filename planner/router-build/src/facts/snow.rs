//! The snow window of a route: the longest run of day steps on which it is usually free of snow,
//! from `snow-seasons.pmtiles` (`specs/planner-snow-tiles.md`) at its samples.

use std::collections::{BTreeMap, BTreeSet};

use obc_data::pmtiles::Archive;
use planner_router::shape::P;
use rayon::prelude::*;

/// The day steps of a season, from 1 September.
pub const STEPS: usize = 183;
/// A sample with fewer seasons with data is left out.
pub const MIN_SEASONS: u16 = 5;
/// A route keeps its window when at most one in this many of its samples is left out.
pub const LEFT_OUT: usize = 5;
const NO_SNOW: u8 = 253;
const ALL_SEASON: u8 = 254;
const NO_DATA: u8 = 255;
/// The shallowest and the deepest zoom of `snow-seasons`.
const ZOOMS: [u8; 2] = [2, 14];
const PIXELS: u32 = 64;
const PLANE: usize = (PIXELS * PIXELS) as usize;
/// A season of a tile body: the onset plane, then the melt-out plane.
const SEASON: usize = 2 * PLANE;

/// A set of day steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Steps([u64; 3]);

impl Steps {
    fn insert(&mut self, step: usize) {
        self.0[step / 64] |= 1 << (step % 64);
    }

    pub fn contains(&self, step: usize) -> bool {
        (self.0[step / 64] >> (step % 64)) & 1 == 1
    }

    fn union(self, other: Steps) -> Steps {
        Steps([0, 1, 2].map(|i| self.0[i] | other.0[i]))
    }
}

/// The steps on which a quarter or more of the seasons with data had snow at a pixel, from the
/// onset and the melt-out of each season. None with fewer than `MIN_SEASONS` seasons with data.
pub fn snowy(seasons: impl Iterator<Item = (u8, u8)>) -> Option<Steps> {
    let mut snow = [0u16; STEPS];
    let mut known = 0;
    for (onset, melt_out) in seasons {
        let days = match onset {
            NO_DATA => continue,
            NO_SNOW => None,
            ALL_SEASON => Some(0..=STEPS - 1),
            onset => Some(usize::from(onset)..=usize::from(melt_out).min(STEPS - 1)),
        };
        known += 1;
        days.into_iter().flatten().for_each(|step| snow[step] += 1);
    }
    let mut steps = Steps::default();
    (0..STEPS).filter(|&step| 4 * snow[step] >= known).for_each(|step| steps.insert(step));
    (known >= MIN_SEASONS).then_some(steps)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    /// The first and the last step of the longest run of free steps; first > last when the run
    /// crosses 1 September.
    Free(u8, u8),
    /// Data, and no free step.
    Never,
    /// Too many samples are left out.
    NoData,
}

/// The snow window of a route and the count of its samples that are left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snow {
    pub window: Window,
    pub left_out: u32,
}

/// The window of a route from the snowy steps of its samples: a step is free when no sample with
/// data is snowy on it.
pub fn window(samples: &[Option<Steps>]) -> Snow {
    let known = samples.iter().flatten();
    let left_out = samples.len() - known.clone().count();
    let window = match left_out * LEFT_OUT <= samples.len() {
        true => longest(known.fold(Steps::default(), |all, steps| all.union(*steps))),
        false => Window::NoData,
    };
    Snow { window, left_out: left_out as u32 }
}

/// The longest run of the steps outside `snowy`, around the season; of two of the same length,
/// the one with the smaller first step.
fn longest(snowy: Steps) -> Window {
    let Some(blocked) = (0..STEPS).find(|&step| snowy.contains(step)) else { return Window::Free(0, STEPS as u8 - 1) };
    let mut best: Option<(usize, usize)> = None;
    let mut run = 0;
    for i in 1..=STEPS {
        let step = (blocked + i) % STEPS;
        if !snowy.contains(step) {
            run += 1;
            continue;
        }
        let first = (step + STEPS - run) % STEPS;
        if run > 0 && best.is_none_or(|(length, start)| run > length || (run == length && first < start)) {
            best = Some((run, first));
        }
        run = 0;
    }
    match best {
        Some((length, first)) => Window::Free(first as u8, ((first + length - 1) % STEPS) as u8),
        None => Window::Never,
    }
}

/// The snowy steps of each point, from its pixel in the deepest tile that the archive holds
/// there; None where no tile holds it or with too few seasons with data. Each tile reads once.
pub fn read(archive: &Archive, points: &[P]) -> Result<Vec<Option<Steps>>, String> {
    let [top, deepest] = ZOOMS;
    let last = (PIXELS << deepest) - 1;
    let pixels: Vec<(u32, u32)> = points
        .iter()
        .map(|p| {
            let (x, y) = crate::grid::mercator(f64::from(p[0]) * 1e-6, f64::from(p[1]) * 1e-6, deepest.into());
            let pixel = |value: f64| ((value * f64::from(PIXELS)).max(0.0) as u32).min(last);
            (pixel(x), pixel(y))
        })
        .collect();
    let deep: BTreeSet<(u32, u32)> = pixels.iter().map(|&(x, y)| (x / PIXELS, y / PIXELS)).collect();
    let deep: Vec<(u32, u32)> = deep.into_iter().collect();
    // For each tile of the deepest zoom, the count of zooms up to the tile that the archive holds.
    let up = deep.par_iter().map(|&(x, y)| -> Result<Option<u8>, String> {
        for k in 0..=deepest - top {
            if archive.get(deepest - k, x >> k, y >> k)?.is_some() {
                return Ok(Some(k));
            }
        }
        Ok(None)
    });
    let up: BTreeMap<(u32, u32), Option<u8>> = deep.iter().copied().zip(up.collect::<Result<Vec<_>, _>>()?).collect();
    let mut tiles: BTreeMap<(u8, u32, u32), Vec<(usize, usize)>> = BTreeMap::new();
    for (i, &(x, y)) in pixels.iter().enumerate() {
        let Some(k) = up[&(x / PIXELS, y / PIXELS)] else { continue };
        let (x, y) = (x >> k, y >> k);
        let pixel = ((y % PIXELS) * PIXELS + x % PIXELS) as usize;
        tiles.entry((deepest - k, x / PIXELS, y / PIXELS)).or_default().push((i, pixel));
    }
    let tiles: Vec<_> = tiles.into_iter().collect();
    let values = tiles.par_iter().map(|&((z, x, y), ref points)| -> Result<Vec<(usize, Option<Steps>)>, String> {
        let body = archive.tile(z, x, y)?.unwrap_or_default();
        if body.is_empty() || body.len() % SEASON != 0 {
            return Err(format!("the snow tile {z}/{x}/{y} has {} bytes", body.len()));
        }
        let seasons = |pixel: usize| {
            body.as_chunks::<SEASON>().0.iter().map(move |season| (season[pixel], season[PLANE + pixel]))
        };
        Ok(points.iter().map(|&(i, pixel)| (i, snowy(seasons(pixel)))).collect())
    });
    let mut snowy = vec![None; points.len()];
    for (i, steps) in values.collect::<Result<Vec<_>, String>>()?.into_iter().flatten() {
        snowy[i] = steps;
    }
    Ok(snowy)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The steps from `first` to `last`, also around the season.
    fn steps(runs: &[(usize, usize)]) -> Steps {
        let mut steps = Steps::default();
        for &(first, last) in runs {
            let length = (last + STEPS - first) % STEPS + 1;
            (0..length).for_each(|i| steps.insert((first + i) % STEPS));
        }
        steps
    }

    /// A route whose samples are snowy on `runs`.
    fn route(runs: &[(usize, usize)]) -> Window {
        window(&[Some(steps(runs)), Some(Steps::default())]).window
    }

    #[test]
    fn the_window_is_the_longest_free_run_around_the_season() {
        // Free from 170 to 10 across 1 September (24 steps), and from 50 to 60.
        assert_eq!(route(&[(11, 49), (61, 169)]), Window::Free(170, 10));
        assert_eq!(route(&[]), Window::Free(0, 182));
        assert_eq!(route(&[(0, 182)]), Window::Never);
        // Two runs of 10: 20 to 29 and 100 to 109; 178 to 4 and 50 to 59.
        assert_eq!(route(&[(30, 99), (110, 19)]), Window::Free(20, 29));
        assert_eq!(route(&[(5, 49), (60, 177)]), Window::Free(50, 59));
        assert_eq!(route(&[(0, 0)]), Window::Free(1, 182));
    }

    #[test]
    fn a_step_is_snowy_when_a_quarter_of_the_seasons_with_data_had_snow() {
        // Eight seasons with data: two with snow from 40 to 100, one more from 90 to 120.
        let mut seasons = vec![(40, 100), (40, 100), (90, 120), (NO_DATA, NO_DATA)];
        seasons.extend([(NO_SNOW, NO_SNOW); 5]);
        assert_eq!(snowy(seasons.iter().copied()), Some(steps(&[(40, 100)])));
        // An all-season snow is snow on each step; one of the nine seasons is less than a quarter.
        seasons.push((ALL_SEASON, ALL_SEASON));
        assert_eq!(snowy(seasons.iter().copied()), Some(steps(&[(40, 100)])));
        assert_eq!(snowy([(ALL_SEASON, ALL_SEASON); 5].into_iter()), Some(steps(&[(0, 182)])));
    }

    #[test]
    fn a_sample_needs_five_seasons_and_a_route_four_in_five_samples() {
        let four = [(NO_SNOW, NO_SNOW); 4];
        assert_eq!(snowy(four.into_iter().chain([(NO_DATA, NO_DATA); 9])), None);
        assert_eq!(snowy(four.into_iter().chain([(NO_SNOW, NO_SNOW)])), Some(Steps::default()));
        // One sample in a forest of MODIS (no data in each season) of five is left out.
        let snowy = Some(steps(&[(0, 100)]));
        let samples = [None, snowy, snowy, Some(Steps::default()), snowy];
        assert_eq!(window(&samples), Snow { window: Window::Free(101, 182), left_out: 1 });
        assert_eq!(window(&samples[..4]), Snow { window: Window::NoData, left_out: 1 });
        assert_eq!(window(&[None]).window, Window::NoData);
    }

    #[test]
    fn read_takes_the_pixel_of_the_deepest_tile() {
        // A tile of zoom 13 and none of zoom 14: a point reads its pixel of zoom 13. Its five seasons
        // have snow from 10 to 20 at pixel (3, 2), and none elsewhere.
        let (x, y) = (4288, 2888);
        let mut body = vec![NO_SNOW; 5 * SEASON];
        for season in body.as_chunks_mut::<SEASON>().0 {
            let pixel = 2 * PIXELS as usize + 3;
            (season[pixel], season[PLANE + pixel]) = (10, 20);
        }
        let id = obc_data::pmtiles::tile_id(13, x, y);
        let path = std::env::temp_dir().join(format!("obc-snow-{}.pmtiles", std::process::id()));
        std::fs::write(&path, crate::facts::shares::tests::archive(id, &body)).unwrap();
        let archive = Archive::open(&path).unwrap();
        // The centre of a pixel of zoom 14 that lies in pixel (3, 2) of the tile of zoom 13.
        let point = |x: u32, y: u32| {
            let n = f64::from(PIXELS << 14);
            let (x, y) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            let lat = (std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan().to_degrees();
            [((x / n * 360.0 - 180.0) * 1e6).round() as i32, (lat * 1e6).round() as i32]
        };
        let deep = |column: u32, row: u32| point(2 * (x * PIXELS + column), 2 * (y * PIXELS + row));
        let read = read(&archive, &[deep(3, 2), deep(4, 2), point(0, 0)]).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(read, [Some(steps(&[(10, 20)])), Some(Steps::default()), None]);
    }
}
