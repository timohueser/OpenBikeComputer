//! `heights --out DIR (--places A,B | --at NAME=LAT,LON) [--size-m M] [--px N] [--sections H,...]`: per
//! place, a hillshade with contours every 0.1 m, the face edges and the steps by class
//! (`NAME-steps.png`), the ripple of the carriageways (`NAME-ripple.png`), cross-sections along
//! headings through the place (`NAME-section-H.png`) and `heights-view.json` with the counts.

use std::collections::BTreeMap;
use std::path::Path;

use obc_ground_plan::faces::heights::check::{at, ripple, terrain_at, Class, STEP_M};
use obc_ground_plan::faces::heights::{height_at_sides, Height, Kind, Step};
use obc_ground_plan::faces::records::{self, label};
use obc_ground_plan::faces::Rules;
use obc_ground_plan::frame::{from_lon_lat, metres_per_unit, CellKey, GlobalPoint};
use obc_ground_plan::graph;
use obc_ground_plan::CellInput;
use serde_json::{json, Value};

use crate::fixture::{default_dir, Fixture};
use crate::heights::ripple_stats;

const THREADS: usize = 4;

struct Place {
    cells: BTreeMap<CellKey, (Vec<u32>, CellInput)>,
}

/// A point of a place: its height, its label in its cell and its cell.
#[derive(Clone, Copy)]
struct Sample {
    h: Height,
    label: Option<u32>,
    cell: CellKey,
    t: f64,
}

impl Place {
    fn new(fix: &mut Fixture, lat: f64, lon: f64, size_m: f64) -> Place {
        let rules = Rules::embedded();
        let mut cells = BTreeMap::new();
        for key in crate::square_cells(lat, lon, size_m + 2.0) {
            let Ok(input) = fix.try_cell(key) else { continue };
            let graph = graph::build(key, &input, &rules.sections, &rules.structures);
            cells.insert(key, (records::build(&graph, &input, rules, key, 0, true).near.words, input));
        }
        Place { cells }
    }

    fn local(&self, g: [f64; 2]) -> Option<(CellKey, [f64; 2])> {
        let key = CellKey::of(GlobalPoint { x: g[0].floor() as i64, y: g[1].floor() as i64 });
        let o = key.origin();
        self.cells.contains_key(&key).then_some((key, [g[0] - o.x as f64, g[1] - o.y as f64]))
    }

    fn at(&self, g: [f64; 2]) -> Option<Sample> {
        let (cell, q) = self.local(g)?;
        let (words, input) = &self.cells[&cell];
        let (h, label) = at(words, &input.terrain, cell, q)?;
        Some(Sample { h, label, cell, t: terrain_at(&input.terrain, cell, q)? })
    }

    /// The class of the jump from a to b, found down to one frame unit, or None for no step.
    fn step(&self, a: [f64; 2], b: [f64; 2], sa: Sample, sb: Sample) -> Option<Class> {
        let (mut a, mut b, mut va, mut vb) = (a, b, sa, sb);
        loop {
            let d = (vb.h.h - va.h.h).abs();
            if d <= STEP_M {
                return None;
            }
            if libm::hypot(b[0] - a[0], b[1] - a[1]) <= 1.0 {
                break;
            }
            let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            let vm = self.at(m)?;
            let (d1, d2) = ((vm.h.h - va.h.h).abs(), (vb.h.h - vm.h.h).abs());
            if d1.max(d2) < 0.75 * d {
                return None;
            }
            if d1 >= d2 {
                (b, vb) = (m, vm);
            } else {
                (a, va) = (m, vm);
            }
        }
        if va.label == vb.label && va.cell == vb.cell && sa.label == sb.label {
            return Some(Class::Inside);
        }
        let (cell, q) = self.local(a)?;
        let words = &self.cells[&cell].0;
        let apart = |id: Option<u32>| {
            id.is_some_and(|id| {
                let l = label(words, id);
                (l[0] >> 8) & 0xff == 3 || (l[0] >> 24 >= 2 && l[2] == u32::MAX)
            })
        };
        if va.cell != vb.cell {
            return Some(Class::Unnamed);
        }
        let sides = height_at_sides(words, q, [va.label, vb.label], va.t);
        let same = ((sides[1].h - sides[0].h) - (vb.h.h - va.h.h)).abs() <= STEP_M;
        Some(if same && sides[0].step != Step::None {
            Class::Named(sides[0].step)
        } else if apart(va.label) || apart(vb.label) {
            Class::Apart
        } else {
            Class::Unnamed
        })
    }
}

fn par_map<T: Send, F: Fn(usize) -> T + Sync>(n: usize, f: F) -> Vec<T> {
    let chunk = n.div_ceil(THREADS);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..THREADS)
            .map(|k| {
                let f = &f;
                s.spawn(move || (k * chunk..((k + 1) * chunk).min(n)).map(f).collect::<Vec<T>>())
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    })
}

fn save(path: &Path, w: u32, h: u32, rgb: Vec<u8>) -> Result<(), String> {
    image::RgbImage::from_raw(w, h, rgb).ok_or("image size")?.save(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn base_colour(words: &[u32], s: &Sample) -> [f64; 3] {
    let w = s.label.map_or(0, |id| label(words, id)[0]);
    let junction = (w >> 8) & 0xff == 2;
    match (w & 0xff, s.h.kind) {
        (0, _) => [0.80, 0.84, 0.74],
        (1, _) => [0.86, 0.80, 0.62],
        (2, _) if junction => [0.62, 0.55, 0.70],
        (2 | 6, _) => [0.55, 0.55, 0.58],
        (3, _) => [0.72, 0.70, 0.66],
        (4, _) => [0.70, 0.80, 0.58],
        (7, _) => [0.55, 0.45, 0.38],
        (_, Kind::Paved) => [0.6, 0.6, 0.6],
        _ => [0.9, 0.6, 0.3],
    }
}

fn class_colour(c: Class) -> [u8; 3] {
    match c {
        Class::Inside => [255, 0, 0],
        Class::Unnamed => [255, 140, 0],
        Class::Apart => [170, 70, 210],
        Class::Named(_) => [0, 110, 255],
    }
}

pub fn run(out: &Path, places: &[(String, f64, f64)], size_m: f64, px: u32, sections: &[f64]) -> Result<(), String> {
    let mut fix = Fixture::open(&default_dir());
    let mut report = Vec::new();
    for (name, lat, lon) in places {
        let r = picture(&mut fix, out, name, *lat, *lon, size_m, px, sections)?;
        println!("{r}");
        report.push(r);
    }
    std::fs::write(out.join("heights-view.json"), serde_json::to_string_pretty(&report).unwrap())
        .map_err(|e| e.to_string())
}

#[allow(clippy::too_many_arguments)]
fn picture(
    fix: &mut Fixture,
    out: &Path,
    name: &str,
    lat: f64,
    lon: f64,
    size_m: f64,
    px: u32,
    sections: &[f64],
) -> Result<Value, String> {
    let place = Place::new(fix, lat, lon, size_m);
    let centre = from_lon_lat(lon, lat);
    let mpu = metres_per_unit(centre.y);
    let (n, pm) = (px as usize, size_m / f64::from(px));
    let corner = [centre.x as f64 - size_m / 2.0 / mpu, centre.y as f64 - size_m / 2.0 / mpu];
    let g = |i: usize, j: usize| [corner[0] + (i as f64 + 0.5) * pm / mpu, corner[1] + (j as f64 + 0.5) * pm / mpu];
    let samples: Vec<Option<Sample>> = par_map(n * n, |k| place.at(g(k % n, k / n)));
    let words_of = |s: &Sample| &place.cells[&s.cell].0;
    // Steps between neighbours, by class.
    let steps: Vec<Vec<Class>> = par_map(n * n, |k| {
        let (i, j) = (k % n, k / n);
        let mut v = Vec::new();
        for nb in [(i + 1 < n).then_some(k + 1), (j + 1 < n).then_some(k + n)].into_iter().flatten() {
            if let (Some(a), Some(b)) = (samples[k], samples[nb]) {
                if let Some(c) = place.step(g(i, j), g(nb % n, nb / n), a, b) {
                    v.push(c);
                }
            }
        }
        v
    });
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for c in steps.iter().flatten() {
        *counts.entry(format!("{c:?}")).or_default() += 1;
    }
    // Hillshade from the north-west, contours every 0.1 m (dark every 1 m), face edges black.
    let h = |k: usize| samples[k].map(|s| s.h.h);
    let mut rgb = vec![0u8; n * n * 3];
    for k in 0..n * n {
        let (i, j) = (k % n, k / n);
        let Some(s) = samples[k] else {
            rgb[3 * k..3 * k + 3].copy_from_slice(&[255, 0, 255]);
            continue;
        };
        let e =
            h(if i + 1 < n { k + 1 } else { k }).unwrap_or(s.h.h) - h(if i > 0 { k - 1 } else { k }).unwrap_or(s.h.h);
        let so =
            h(if j + 1 < n { k + n } else { k }).unwrap_or(s.h.h) - h(if j > 0 { k - n } else { k }).unwrap_or(s.h.h);
        let nrm = [-e / (2.0 * pm), so / (2.0 * pm), 1.0];
        let l = [-0.5, 0.5, std::f64::consts::FRAC_1_SQRT_2];
        let len = (nrm[0] * nrm[0] + nrm[1] * nrm[1] + nrm[2] * nrm[2]).sqrt();
        let shade = ((nrm[0] * l[0] + nrm[1] * l[1] + nrm[2] * l[2]) / len).clamp(0.0, 1.0);
        let mut col = base_colour(words_of(&s), &s).map(|v| v * (0.35 + 0.75 * shade));
        let band = |v: f64, step: f64| (v / step).floor();
        let mut line = 0;
        for nb in [(i + 1 < n).then_some(k + 1), (j + 1 < n).then_some(k + n)].into_iter().flatten() {
            let Some(o) = samples[nb] else { continue };
            if o.label != s.label || o.cell != s.cell {
                line = 3;
            } else if band(o.h.h, 1.0) != band(s.h.h, 1.0) {
                line = line.max(2);
            } else if band(o.h.h, 0.1) != band(s.h.h, 0.1) {
                line = line.max(1);
            }
        }
        col = match line {
            1 => col.map(|v| v * 0.72),
            2 => col.map(|v| v * 0.4),
            3 => [0.0; 3],
            _ => col,
        };
        for (c, v) in col.iter().enumerate() {
            rgb[3 * k + c] = (v.clamp(0.0, 1.0) * 255.0) as u8;
        }
        if let Some(c) = steps[k].iter().max_by_key(|c| match c {
            Class::Inside => 3,
            Class::Unnamed => 2,
            Class::Apart => 0,
            Class::Named(_) => 1,
        }) {
            rgb[3 * k..3 * k + 3].copy_from_slice(&class_colour(*c));
        }
    }
    save(&out.join(format!("{name}-steps.png")), px, px, rgb)?;
    // The ripple of the carriageways: grey paved, green slope; yellow from 0.5 mm, red at 5 mm.
    let ripples: Vec<Option<[f64; 2]>> = par_map(n * n, |k| {
        let s = samples[k]?;
        let (cell, q) = place.local(g(k % n, k / n))?;
        let (words, input) = &place.cells[&cell];
        ripple(words, &input.terrain, cell, q, (s.h, s.label))
    });
    let mut rgb = Vec::with_capacity(n * n * 3);
    for (s, r) in samples.iter().zip(&ripples) {
        let base: [f64; 3] = match s.map(|s| s.h.kind) {
            Some(Kind::Paved) => [150.0, 150.0, 155.0],
            Some(Kind::Slope) => [140.0, 170.0, 110.0],
            Some(Kind::Terrain) => [90.0, 110.0, 80.0],
            None => [40.0, 40.0, 40.0],
        };
        let d = r.map_or(0.0, |r| r[0].abs().max(r[1].abs()) * 1000.0);
        let d = if d.is_nan() { 0.0 } else { d };
        let t = ((d - 0.5) / 4.5).clamp(0.0, 1.0);
        let c = if d < 0.5 { base } else { [255.0, 255.0 * (1.0 - t), 0.0] };
        rgb.extend(c.map(|v| v as u8));
    }
    save(&out.join(format!("{name}-ripple.png")), px, px, rgb)?;
    // The ripple's statistics on its own grid.
    let rn = (size_m / 0.25) as usize;
    let grid: Vec<Option<[f64; 2]>> = par_map(rn * rn, |k| {
        let gp = [corner[0] + (k % rn) as f64 * 0.25 / mpu, corner[1] + (k / rn) as f64 * 0.25 / mpu];
        let (cell, q) = place.local(gp)?;
        let (words, input) = &place.cells[&cell];
        let (h, l) = at(words, &input.terrain, cell, q)?;
        ripple(words, &input.terrain, cell, q, (h, l))
    });
    let stats = [0, 1].map(|a| ripple_stats(grid.iter().flatten().map(|r| r[a]).filter(|x| !x.is_nan()).collect()));
    // Sections along headings through the place's centre.
    for &hd in sections {
        let (sn, cs) = (libm::sin(hd.to_radians()), libm::cos(hd.to_radians()));
        let half = size_m * 0.45;
        let m = 1600usize;
        let pts: Vec<(f64, Option<Sample>)> = (0..m)
            .map(|k| {
                let f = k as f64 / (m - 1) as f64;
                let off = [(-sn + 2.0 * f * sn) * half, (cs - 2.0 * f * cs) * half];
                (f * 2.0 * half, place.at([centre.x as f64 + off[0] / mpu, centre.y as f64 + off[1] / mpu]))
            })
            .collect();
        section_png(&out.join(format!("{name}-section-{:03}.png", hd as i32)), &pts, 2.0 * half)?;
    }
    Ok(
        json!({"name": name, "lat": lat, "lon": lon, "px_m": pm, "steps": counts, "ripple_mm": {"along": stats[0], "across": stats[1]}}),
    )
}

/// A cross-section: the height (dark: paved, brown: slope, green: terrain) and the plain terrain
/// (light green) along a line, true scale where it fits.
fn section_png(path: &Path, pts: &[(f64, Option<Sample>)], len_m: f64) -> Result<(), String> {
    let (w, h) = (1600u32, 560u32);
    let mut rgb = vec![255u8; (w * h * 3) as usize];
    let hs: Vec<f64> = pts
        .iter()
        .filter_map(|(_, s)| s.map(|s| s.h.h))
        .chain(pts.iter().filter_map(|(_, s)| s.map(|s| s.t)))
        .collect();
    if hs.is_empty() {
        return Ok(());
    }
    let lo = hs.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = hs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let scale_x = f64::from(w) / len_m;
    let scale_y = (f64::from(h - 60) / (hi - lo).max(0.5)).min(scale_x * 4.0);
    let y_of = |z: f64| f64::from(h - 40) - (z - lo) * scale_y;
    let mut put = |x: i64, y: i64, c: [u8; 3]| {
        if x >= 0 && y >= 0 && x < i64::from(w) && y < i64::from(h) {
            let k = ((y as u32 * w + x as u32) * 3) as usize;
            rgb[k..k + 3].copy_from_slice(&c);
        }
    };
    let mut z = lo.floor();
    while z <= hi {
        for x in 0..i64::from(w) {
            put(x, y_of(z) as i64, [225, 225, 225]);
        }
        z += 1.0;
    }
    let mut prev: [Option<(i64, i64)>; 2] = [None, None];
    for (along, s) in pts {
        let x = (along * scale_x) as i64;
        let Some(s) = s else {
            prev = [None, None];
            continue;
        };
        let col = match s.h.kind {
            Kind::Terrain => [60, 140, 60],
            Kind::Slope => [190, 140, 40],
            Kind::Paved => [40, 40, 40],
        };
        for (k, (v, c)) in [(s.t, [120, 200, 120]), (s.h.h, col)].into_iter().enumerate() {
            let y = y_of(v) as i64;
            if let Some((px0, py0)) = prev[k] {
                for yv in py0.min(y)..=py0.max(y) {
                    put(px0, yv, c);
                }
            }
            put(x, y, c);
            put(x, y + 1, c);
            prev[k] = Some((x, y));
        }
        for yv in (i64::from(h) - 30)..(i64::from(h) - 20) {
            put(x, yv, col);
        }
    }
    save(path, w, h, rgb)
}
