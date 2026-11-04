//! `heights`: close-up proof of the one road model's heights (`faces::heights::height_at`) at places:
//! a hillshade with contours every 0.1 m and the label boundaries, a step map, cross-sections, and a
//! height grid for the 3D page. Prototype of the one road model; not for merge.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use obc_ground_plan::faces::heights::{height_at, Kind};
use obc_ground_plan::faces::records::{self, find, LABEL};
use obc_ground_plan::faces::Rules;
use obc_ground_plan::frame::{from_lon_lat, metres_per_unit, CellKey, GlobalPoint};
use obc_ground_plan::graph;
use obc_ground_plan::CellInput;
use serde_json::json;

use crate::fixture::{self, Fixture};

/// A step: a jump over one frame unit (about 1.6 mm) of more than this.
const STEP_M: f64 = 0.01;
const THREADS: usize = 4;

struct Place {
    blocks: BTreeMap<CellKey, Vec<u32>>,
    inputs: BTreeMap<CellKey, CellInput>,
}

#[derive(Clone, Copy)]
struct Sample {
    h: f64,
    t: f64,
    label: u32,
    kind: u8,
}

impl Place {
    fn new(fix: &mut Fixture, lat: f64, lon: f64, size_m: f64) -> Place {
        let rules = Rules::embedded();
        let mut blocks = BTreeMap::new();
        let mut inputs = BTreeMap::new();
        for key in crate::square_cells(lat, lon, size_m + 2.0) {
            let Ok(input) = fix.try_cell(key) else { continue };
            let graph = graph::build(key, &input, &rules.sections);
            let rec = records::records(&graph, &input, rules, key, 0, true);
            blocks.insert(key, rec.words);
            inputs.insert(key, input);
        }
        Place { blocks, inputs }
    }

    /// The sample at global frame point g (f64), with the label that `find` gives, or with `side`
    /// forced (for both sides of a boundary).
    fn at(&self, g: [f64; 2]) -> Option<Sample> {
        let key = CellKey::of(GlobalPoint { x: g[0].floor() as i64, y: g[1].floor() as i64 });
        let words = self.blocks.get(&key)?;
        let input = self.inputs.get(&key)?;
        let o = key.origin();
        let q = [g[0] - o.x as f64, g[1] - o.y as f64];
        let t = input.terrain.height(GlobalPoint { x: g[0].round() as i64, y: g[1].round() as i64 })?;
        let label = find(words, q).label;
        let h = height_at(words, q, label, t);
        let kind = match h.kind {
            Kind::Terrain => 0,
            Kind::Slope => 1,
            Kind::Paved => 2,
        };
        Some(Sample { h: h.h, t, label: label.unwrap_or(u32::MAX) ^ (key.x << 20) ^ (key.y << 8), kind })
    }

    /// The surface code of a sample's label (for colours), 255 for terrain.
    fn surface(&self, g: [f64; 2]) -> u32 {
        let key = CellKey::of(GlobalPoint { x: g[0].floor() as i64, y: g[1].floor() as i64 });
        let Some(words) = self.blocks.get(&key) else { return 255 };
        let o = key.origin();
        match find(words, [g[0] - o.x as f64, g[1] - o.y as f64]).label {
            Some(id) => {
                let w0 = words[words[5] as usize + LABEL * id as usize];
                (w0 & 0xff) | ((w0 >> 8) & 0xff) << 8
            }
            None => 255,
        }
    }
}

fn par_map<T: Send, F: Fn(usize) -> T + Sync>(n: usize, f: F) -> Vec<T> {
    let chunk = n.div_ceil(THREADS);
    let mut out: Vec<Vec<T>> = Vec::new();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..THREADS)
            .map(|k| {
                let f = &f;
                s.spawn(move || (k * chunk..((k + 1) * chunk).min(n)).map(f).collect::<Vec<T>>())
            })
            .collect();
        for h in handles {
            out.push(h.join().unwrap());
        }
    });
    out.into_iter().flatten().collect()
}

fn base_colour(surface: u32, kind: u8) -> [f64; 3] {
    let s = surface & 0xff;
    let junction = (surface >> 8) & 0xff == records::JUNCTION;
    match s {
        255 | 0 => [0.80, 0.84, 0.74],
        1 => [0.86, 0.80, 0.62],
        2 if junction => [0.62, 0.55, 0.70],
        2 => [0.55, 0.55, 0.58],
        3 => [0.72, 0.70, 0.66],
        4 => [0.70, 0.80, 0.58],
        6 => [0.75, 0.40, 0.40],
        7 => [0.55, 0.45, 0.38],
        _ if kind == 2 => [0.6, 0.6, 0.6],
        _ => [0.9, 0.6, 0.3],
    }
}

/// Whether the jump from a to b (global points, heights ha, hb) holds down to one frame unit: the span
/// halves, keeping the half whose heights change more, while that half keeps most of the change.
fn is_step(place: &Place, a: [f64; 2], b: [f64; 2], ha: f64, hb: f64) -> Option<([f64; 2], f64)> {
    let (mut a, mut b, mut ha, mut hb) = (a, b, ha, hb);
    loop {
        let d = (hb - ha).abs();
        if d <= STEP_M {
            return None;
        }
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        if len <= 1.0 {
            return Some((a, hb - ha));
        }
        let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let hm = place.at(m)?.h;
        let (d1, d2) = ((hm - ha).abs(), (hb - hm).abs());
        if d1.max(d2) < 0.75 * d {
            return None;
        }
        if d1 >= d2 {
            b = m;
            hb = hm;
        } else {
            a = m;
            ha = hm;
        }
    }
}

struct Pictures<'a> {
    out: &'a Path,
    name: &'a str,
}

fn save(path: &Path, w: u32, h: u32, rgb: Vec<u8>) -> Result<(), String> {
    image::RgbImage::from_raw(w, h, rgb).ok_or("image size")?.save(path).map_err(|e| format!("{}: {e}", path.display()))
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
) -> Result<serde_json::Value, String> {
    let place = Place::new(fix, lat, lon, size_m);
    let centre = from_lon_lat(lon, lat);
    let mpu = metres_per_unit(centre.y);
    let upp = size_m / f64::from(px) / mpu;
    let corner = [centre.x as f64 - upp * f64::from(px) / 2.0, centre.y as f64 - upp * f64::from(px) / 2.0];
    let g = |i: f64, j: f64| [corner[0] + (i + 0.5) * upp, corner[1] + (j + 0.5) * upp];
    let n = (px * px) as usize;
    let t0 = std::time::Instant::now();
    let samples: Vec<Option<Sample>> = par_map(n, |k| place.at(g((k % px as usize) as f64, (k / px as usize) as f64)));
    let surfaces: Vec<u32> = par_map(n, |k| place.surface(g((k % px as usize) as f64, (k / px as usize) as f64)));
    let eval_s = t0.elapsed().as_secs_f64();
    if let Ok(probe) = std::env::var("HEIGHTS_PROBE") {
        for pt in probe.split(';') {
            let v: Vec<f64> = pt.split(',').map(|x| x.parse().unwrap()).collect();
            let gp = g(v[0], v[1]);
            let key = CellKey::of(GlobalPoint { x: gp[0].floor() as i64, y: gp[1].floor() as i64 });
            let o = key.origin();
            let q = [gp[0] - o.x as f64, gp[1] - o.y as f64];
            let words = &place.blocks[&key];
            let s = place.at(gp);
            let fd = find(words, q);
            let lab = fd.label.map(|id| {
                let a = words[5] as usize + LABEL * id as usize;
                [words[a], words[a + 1], words[a + 2]]
            });
            let kinds: Vec<u32> = fd.covered.iter().map(|&c| words[words[4] as usize + 12 * c as usize]).collect();
            for l in 0..words[3].min(4) {
                let a = words[6] as usize + 12 * l as usize;
                let f = |k: usize| f32::from_bits(words[a + k]);
                let first = words[a + 2] as usize;
                let sm = words[7] as usize + 8 * first;
                eprintln!(
                    "  line {l} way {} class {} reach {} shoulder {} verge {} half {} sample0 h(units) {} mpu {}",
                    words[a],
                    words[a + 7],
                    f(6),
                    f(8),
                    f(9),
                    f(11),
                    f32::from_bits(words[sm + 2]),
                    f32::from_bits(words[10])
                );
            }
            eprintln!(
                "  label {:?} surface {} rule {} covered kinds {:?}",
                lab,
                lab.map_or(0, |l| l[0] & 0xff),
                lab.map_or(0, |l| l[0] >> 24),
                kinds
            );
            eprintln!(
                "probe {pt} cell {} {} q {:.0},{:.0} bin {},{} h {:?} recs {:?}",
                key.x,
                key.y,
                q[0],
                q[1],
                (q[0] / 4096.0).floor(),
                (q[1] / 4096.0).floor(),
                s.map(|s| (s.h, s.t, s.kind)),
                obc_ground_plan::faces::heights::explain(words, q)
            );
        }
    }
    let pm = size_m / f64::from(px);
    let hs = |k: usize| samples[k].map(|s| s.h);
    // Steps between neighbours, by whether they lie on a label boundary.
    let pairs: Vec<(usize, usize)> = (0..n)
        .flat_map(|k| {
            let (i, j) = (k % px as usize, k / px as usize);
            let mut v = Vec::new();
            if i + 1 < px as usize {
                v.push((k, k + 1));
            }
            if j + 1 < px as usize {
                v.push((k, k + px as usize));
            }
            v
        })
        .collect();
    let steps: Vec<Option<(usize, usize, f64, bool)>> = par_map(pairs.len(), |e| {
        let (a, b) = pairs[e];
        let (Some(sa), Some(sb)) = (samples[a], samples[b]) else { return None };
        if (sb.h - sa.h).abs() <= STEP_M {
            return None;
        }
        let ga = g((a % px as usize) as f64, (a / px as usize) as f64);
        let gb = g((b % px as usize) as f64, (b / px as usize) as f64);
        let (at, dh) = is_step(&place, ga, gb, sa.h, sb.h)?;
        let _ = at;
        Some((a, b, dh, sa.label != sb.label))
    });
    let steps: Vec<(usize, usize, f64, bool)> = steps.into_iter().flatten().collect();
    // Picture 1: hillshade, contours every 0.1 m (dark every 1 m), label boundaries.
    let mut rgb = vec![0u8; n * 3];
    for k in 0..n {
        let (i, j) = (k % px as usize, k / px as usize);
        let Some(s) = samples[k] else {
            rgb[3 * k..3 * k + 3].copy_from_slice(&[255, 0, 255]);
            continue;
        };
        let e = hs(if i + 1 < px as usize { k + 1 } else { k }).unwrap_or(s.h)
            - hs(if i > 0 { k - 1 } else { k }).unwrap_or(s.h);
        let sth = hs(if j + 1 < px as usize { k + px as usize } else { k }).unwrap_or(s.h)
            - hs(if j > 0 { k - px as usize } else { k }).unwrap_or(s.h);
        let (dx, dy) = (e / (2.0 * pm), sth / (2.0 * pm));
        // Light from the north-west, 45° up.
        let nrm = [-dx, dy, 1.0];
        let l = [-0.5, 0.5, 0.7071];
        let len = (nrm[0] * nrm[0] + nrm[1] * nrm[1] + nrm[2] * nrm[2]).sqrt();
        let shade = ((nrm[0] * l[0] + nrm[1] * l[1] + nrm[2] * l[2]) / len).clamp(0.0, 1.0);
        let c = base_colour(surfaces[k], s.kind);
        let mut col = c.map(|v| v * (0.35 + 0.75 * shade));
        let band = |h: f64, step: f64| (h / step).floor();
        let mut contour = 0;
        for nb in [
            if i + 1 < px as usize { Some(k + 1) } else { None },
            if j + 1 < px as usize { Some(k + px as usize) } else { None },
        ]
        .into_iter()
        .flatten()
        {
            if let Some(hn) = hs(nb) {
                if band(hn, 1.0) != band(s.h, 1.0) {
                    contour = 2;
                } else if band(hn, 0.1) != band(s.h, 0.1) && contour == 0 {
                    contour = 1;
                }
                if samples[nb].map(|x| x.label) != Some(s.label) {
                    contour = 3;
                }
            }
        }
        match contour {
            1 => col = col.map(|v| v * 0.72),
            2 => col = col.map(|v| v * 0.4),
            3 => col = [0.0, 0.0, 0.0],
            _ => {}
        }
        for (c, v) in col.iter().enumerate() {
            rgb[3 * k + c] = (v.clamp(0.0, 1.0) * 255.0) as u8;
        }
    }
    let mut marked = rgb.clone();
    for &(a, b, _, boundary) in &steps {
        let c = if boundary { [255, 140, 0] } else { [255, 0, 0] };
        for k in [a, b] {
            marked[3 * k..3 * k + 3].copy_from_slice(&c);
        }
    }
    // Section lines on the picture.
    let half = size_m * 0.45;
    let mut lines_px = Vec::new();
    for &hd in sections {
        let (s, c) = (hd.to_radians().sin(), hd.to_radians().cos());
        // Heading from north, clockwise: east = sin, south = −cos.
        let (a, b) = ([-s * half, c * half], [s * half, -c * half]);
        lines_px.push((hd, a, b));
        for k in 0..2000 {
            let f = k as f64 / 1999.0;
            let m = [a[0] + f * (b[0] - a[0]), a[1] + f * (b[1] - a[1])];
            let (i, j) = ((m[0] / pm + f64::from(px) / 2.0) as i64, (m[1] / pm + f64::from(px) / 2.0) as i64);
            if i >= 0 && j >= 0 && i < i64::from(px) && j < i64::from(px) && k % 6 < 3 {
                let kk = (j * i64::from(px) + i) as usize;
                marked[3 * kk..3 * kk + 3].copy_from_slice(&[0, 90, 255]);
            }
        }
    }
    let p = Pictures { out, name };
    save(&p.out.join(format!("{}-hillshade.png", p.name)), px, px, marked)?;
    // Picture 2: slope magnitude (blue flat → yellow 0.5 → red 1 → black 2) with steps.
    let mut srgb = vec![0u8; n * 3];
    for k in 0..n {
        let (i, j) = (k % px as usize, k / px as usize);
        let Some(s) = samples[k] else { continue };
        let e = hs(if i + 1 < px as usize { k + 1 } else { k }).unwrap_or(s.h) - s.h;
        let sth = hs(if j + 1 < px as usize { k + px as usize } else { k }).unwrap_or(s.h) - s.h;
        let sl = (e * e + sth * sth).sqrt() / pm;
        let c = if sl < 0.5 {
            let f = sl / 0.5;
            [0.2 + 0.8 * f, 0.4 + 0.5 * f, 0.9 - 0.8 * f]
        } else if sl < 1.0 {
            let f = (sl - 0.5) / 0.5;
            [1.0, 0.9 - 0.7 * f, 0.1]
        } else {
            let f = ((sl - 1.0) / 1.0).min(1.0);
            [1.0 - 0.8 * f, 0.2 - 0.2 * f, 0.1 - 0.1 * f]
        };
        let edge = [k + 1, k + px as usize].iter().any(|&nb| nb < n && samples[nb].map(|x| x.label) != Some(s.label));
        let c = if edge { [0.3, 0.3, 0.3] } else { c };
        for (ci, v) in c.iter().enumerate() {
            srgb[3 * k + ci] = (v.clamp(0.0, 1.0) * 255.0) as u8;
        }
    }
    for &(a, b, _, boundary) in &steps {
        let c = if boundary { [255, 255, 255] } else { [255, 0, 255] };
        for k in [a, b] {
            srgb[3 * k..3 * k + 3].copy_from_slice(&c);
        }
    }
    save(&p.out.join(format!("{}-slope.png", p.name)), px, px, srgb)?;
    // Cross-sections.
    let mut sec_json = Vec::new();
    for (hd, a, b) in &lines_px {
        let m = 1600usize;
        let pts: Vec<(f64, Option<Sample>)> = (0..m)
            .map(|k| {
                let f = k as f64 / (m - 1) as f64;
                let off = [a[0] + f * (b[0] - a[0]), a[1] + f * (b[1] - a[1])];
                (f * 2.0 * half, place.at([centre.x as f64 + off[0] / mpu, centre.y as f64 + off[1] / mpu]))
            })
            .collect();
        section_png(&p.out.join(format!("{}-section-{:03}.png", p.name, *hd as i32)), &pts, 2.0 * half)?;
        sec_json.push(json!({"heading": hd, "h": pts.iter().map(|(_, s)| s.map(|s| (s.h * 1000.0).round() / 1000.0)).collect::<Vec<_>>()}));
    }
    // The grid for the 3D page: every 4th pixel.
    let stride = 4usize;
    let gw = (px as usize).div_ceil(stride);
    let mut grid_h = Vec::new();
    let mut grid_s = Vec::new();
    for j in (0..px as usize).step_by(stride) {
        for i in (0..px as usize).step_by(stride) {
            let k = j * px as usize + i;
            grid_h.push(samples[k].map_or(f64::NAN, |s| s.h));
            grid_s.push(surfaces[k] & 0xffff);
        }
    }
    let step_list: Vec<[f64; 3]> = steps
        .iter()
        .map(|&(a, _, dh, _)| {
            [
                ((a % px as usize) as f64 + 0.5) * pm - size_m / 2.0,
                ((a / px as usize) as f64 + 0.5) * pm - size_m / 2.0,
                dh,
            ]
        })
        .collect();
    let inside = steps.iter().filter(|s| !s.3).count();
    let boundary = steps.len() - inside;
    let grid = json!({"name": name, "size_m": size_m, "n": gw, "h": grid_h.iter().map(|h| if h.is_nan() { None } else { Some((h * 1000.0).round() / 1000.0) }).collect::<Vec<_>>(), "s": grid_s, "steps": step_list});
    std::fs::write(p.out.join(format!("{name}.grid.json")), grid.to_string()).map_err(|e| e.to_string())?;
    Ok(
        json!({"name": name, "lat": lat, "lon": lon, "px_m": pm, "eval_s": eval_s, "ns_per_point": eval_s * 1e9 / n as f64 * THREADS as f64, "steps_inside_faces": inside, "steps_on_label_edges": boundary, "max_step_m": steps.iter().map(|s| s.2.abs()).fold(0.0, f64::max), "edge_list": steps.iter().filter(|s| s.3).step_by(1 + steps.iter().filter(|s| s.3).count() / 30).map(|s| json!([s.0 % px as usize, s.0 / px as usize, s.1 % px as usize, s.1 / px as usize, (s.2 * 1000.0).round() / 1000.0])).collect::<Vec<_>>(), "inside_list": steps.iter().filter(|s| !s.3).take(30).map(|s| json!([s.0 % px as usize, s.0 / px as usize, s.1 % px as usize, s.1 / px as usize, (s.2 * 1000.0).round() / 1000.0])).collect::<Vec<_>>(), "sections": sec_json.len()}),
    )
}

fn section_png(path: &Path, pts: &[(f64, Option<Sample>)], len_m: f64) -> Result<(), String> {
    let (w, h) = (1600u32, 560u32);
    let mut rgb = vec![255u8; (w * h * 3) as usize];
    let hs: Vec<f64> =
        pts.iter().filter_map(|(_, s)| s.map(|s| s.h)).chain(pts.iter().filter_map(|(_, s)| s.map(|s| s.t))).collect();
    if hs.is_empty() {
        return Ok(());
    }
    let lo = hs.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = hs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    // True scale where it fits, else exaggerated (written in the picture's name list).
    let scale_x = f64::from(w) / len_m;
    let span = (hi - lo).max(0.5);
    let scale_y = (f64::from(h - 60) / span).min(scale_x * 4.0);
    let y_of = |z: f64| f64::from(h - 40) - (z - lo) * scale_y;
    let put = |x: i64, y: i64, c: [u8; 3], rgb: &mut Vec<u8>| {
        if x >= 0 && y >= 0 && x < i64::from(w) && y < i64::from(h) {
            let k = ((y as u32 * w + x as u32) * 3) as usize;
            rgb[k..k + 3].copy_from_slice(&c);
        }
    };
    // Grid lines every 1 m in height and 5 m along.
    let mut z = lo.floor();
    while z <= hi {
        let y = y_of(z) as i64;
        for x in 0..i64::from(w) {
            put(x, y, [225, 225, 225], &mut rgb);
        }
        z += 1.0;
    }
    let mut a = 0.0;
    while a <= len_m {
        let x = (a * scale_x) as i64;
        for y in 0..i64::from(h) {
            put(x, y, [225, 225, 225], &mut rgb);
        }
        a += 5.0;
    }
    let mut prev: Option<(i64, i64)> = None;
    let mut prev_t: Option<(i64, i64)> = None;
    for (along, s) in pts {
        let x = (along * scale_x) as i64;
        let Some(s) = s else {
            prev = None;
            continue;
        };
        let (y, yt) = (y_of(s.h) as i64, y_of(s.t) as i64);
        let col = match s.kind {
            0 => [60, 140, 60],
            1 => [190, 140, 40],
            _ => [40, 40, 40],
        };
        for (p, yy, c) in [(&mut prev_t, yt, [120, 200, 120]), (&mut prev, y, col)] {
            if let Some((px0, py0)) = *p {
                let (y0, y1) = (py0.min(yy), py0.max(yy));
                for yv in y0..=y1 {
                    put(x, yv, c, &mut rgb);
                    put(px0, yv, c, &mut rgb);
                }
            }
            put(x, yy, c, &mut rgb);
            put(x, yy + 1, c, &mut rgb);
            *p = Some((x, yy));
        }
        // The kind as a bar along the bottom.
        for yv in (i64::from(h) - 30)..(i64::from(h) - 20) {
            put(x, yv, col, &mut rgb);
        }
    }
    save(path, w, h, rgb)
}

pub fn run() -> Result<(), String> {
    let (mut out, mut at, mut size_m, mut px, mut sections) =
        (None::<PathBuf>, Vec::new(), 64.0f64, 1280u32, vec![0.0, 45.0, 90.0, 135.0]);
    let mut names: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(2);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--out" => out = Some(value()?.into()),
            "--size-m" => size_m = value()?.parse().map_err(|_| "--size-m: a number")?,
            "--px" => px = value()?.parse().map_err(|_| "--px: a number")?,
            "--sections" => sections = value()?.split(',').map(|s| s.parse().unwrap_or(0.0)).collect(),
            "--places" => names = value()?.split(',').map(String::from).collect(),
            "--at" => {
                let text = value()?;
                let (name, ll) = text.split_once('=').ok_or("--at NAME=LAT,LON")?;
                let (lat, lon) = ll.split_once(',').ok_or("--at NAME=LAT,LON")?;
                at.push((
                    name.to_string(),
                    lat.parse::<f64>().map_err(|e| e.to_string())?,
                    lon.parse::<f64>().map_err(|e| e.to_string())?,
                ));
            }
            _ => return Err(format!("heights: unknown option {arg}")),
        }
    }
    let out =
        out.ok_or("heights --out DIR (--places A,B | --at NAME=LAT,LON) [--size-m 64] [--px 1280] [--sections 0,45]")?;
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let views =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../builder/web/tests/browser/trailview/road-views-top.json");
    let mut places = crate::named_places(&views, &names)?;
    places.extend(at);
    let mut fix = Fixture::open(&fixture::default_dir());
    if std::env::var("HEIGHTS_JUNCTIONS").is_ok() {
        let rules = Rules::embedded();
        for (name, lat, lon) in &places {
            for key in crate::square_cells(*lat, *lon, size_m) {
                let Ok(input) = fix.try_cell(key) else { continue };
                let g = graph::build(key, &input, &rules.sections);
                for j in g.junctions.iter().filter(|j| j.owned) {
                    let o = key.origin();
                    let gp = GlobalPoint { x: o.x + i64::from(j.pos[0]), y: o.y + i64::from(j.pos[1]) };
                    let (jlon, jlat) = crate::lon_lat(gp);
                    let c = from_lon_lat(*lon, *lat);
                    let mpu = metres_per_unit(c.y);
                    let (dx, dy) = ((gp.x - c.x) as f64 * mpu, (gp.y - c.y) as f64 * mpu);
                    if dx.abs() < size_m / 2.0 && dy.abs() < size_m / 2.0 {
                        eprintln!(
                            "{name} junction {jlat:.6},{jlon:.6} at {dx:.1} m east {:.1} m north, arms {}",
                            -dy,
                            j.arms.len()
                        );
                    }
                }
            }
        }
        return Ok(());
    }
    let mut report = Vec::new();
    for (name, lat, lon) in &places {
        let r = picture(&mut fix, &out, name, *lat, *lon, size_m, px, &sections)?;
        eprintln!("{r}");
        report.push(r);
    }
    std::fs::write(out.join("heights-report.json"), serde_json::to_string_pretty(&report).unwrap())
        .map_err(|e| e.to_string())?;
    Ok(())
}
