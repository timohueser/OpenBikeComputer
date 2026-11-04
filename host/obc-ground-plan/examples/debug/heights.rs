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
            eprintln!("  quads {:?}", obc_ground_plan::faces::heights::explain_quads(words, q));
            eprintln!("  discs {:?}", obc_ground_plan::faces::heights::explain_discs(words, q));
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
    // The grid for the 3D page: every 4th pixel, heights in cm over the least, and per point a byte:
    // surface (bits 0-3), kind (4-5), on a label edge (6), a crease (7: the slope turns by more than
    // 0.2 between the two neighbours along x or y at full resolution).
    let stride = 4usize;
    let gw = (px as usize).div_ceil(stride);
    let z0 = samples.iter().flatten().map(|s| s.h).fold(f64::INFINITY, f64::min);
    let mut grid_h: Vec<u8> = Vec::new();
    let mut grid_s: Vec<u8> = Vec::new();
    let pxu = px as usize;
    for j in (0..pxu).step_by(stride) {
        for i in (0..pxu).step_by(stride) {
            let k = j * pxu + i;
            let v = samples[k].map_or(65535u16, |s| ((s.h - z0) * 100.0).round().clamp(0.0, 65534.0) as u16);
            grid_h.extend(v.to_le_bytes());
            let surf = (surfaces[k] & 0xf) as u8;
            let kind = samples[k].map_or(0, |s| s.kind);
            let edge = [k + 1, k + pxu]
                .iter()
                .any(|&nb| nb < n && samples[nb].map(|x| x.label) != samples[k].map(|x| x.label));
            let d2 = |a: usize, b: usize| match (samples[a], samples[k], samples[b]) {
                (Some(x), Some(y), Some(z)) => (x.h - 2.0 * y.h + z.h).abs() / pm,
                _ => 0.0,
            };
            let crease =
                i > 0 && j > 0 && i + 1 < pxu && j + 1 < pxu && d2(k - 1, k + 1).max(d2(k - pxu, k + pxu)) > 0.2;
            grid_s.push(surf | kind << 4 | u8::from(edge) << 6 | u8::from(crease) << 7);
        }
    }
    let b64 = |v: &[u8]| {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(v.len().div_ceil(3) * 4);
        for c in v.chunks(3) {
            let w =
                u32::from(c[0]) << 16 | u32::from(*c.get(1).unwrap_or(&0)) << 8 | u32::from(*c.get(2).unwrap_or(&0));
            for k in 0..4 {
                out.push(if k <= c.len() { A[(w >> (18 - 6 * k) & 63) as usize] as char } else { '=' });
            }
        }
        out
    };
    let step_list: Vec<[f64; 3]> = steps
        .iter()
        .map(|&(a, _, dh, _)| {
            [
                ((((a % pxu) as f64 + 0.5) * pm - size_m / 2.0) * 100.0).round() / 100.0,
                ((((a / pxu) as f64 + 0.5) * pm - size_m / 2.0) * 100.0).round() / 100.0,
                (dh * 1000.0).round() / 1000.0,
            ]
        })
        .collect();
    let grid = json!({"name": name, "lat": lat, "lon": lon, "size_m": size_m, "n": gw, "z0": z0, "h": b64(&grid_h), "s": b64(&grid_s), "steps": step_list});
    std::fs::write(p.out.join(format!("{name}.grid.json")), grid.to_string()).map_err(|e| e.to_string())?;
    let inside = steps.iter().filter(|s| !s.3).count();
    let boundary = steps.len() - inside;
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

/// One step found in the fixture scan.
#[derive(Clone, Debug)]
struct Step {
    cell: CellKey,
    q: [f64; 2],
    dh: f64,
    edge: bool,
    sides: [String; 2],
}

struct CellScan {
    key: CellKey,
    points: u64,
    evals: u64,
    eval_ns: f64,
    steps: Vec<Step>,
    /// Border samples: (axis 0 for x = const, the line's global coordinate, the global coordinate
    /// along, side 0 west or north / 1 east or south) -> (height, rule kind).
    border: Vec<((u8, i64, i64, u8), (f64, u8))>,
    ns_paved: f64,
    ns_slope: f64,
    vertices: f64,
    mesh_ms: f64,
}

fn side_name(words: &[u32], h: &obc_ground_plan::faces::heights::Height) -> String {
    let kind = match h.kind {
        Kind::Terrain => "terrain",
        Kind::Slope => "slope",
        Kind::Paved => "paved",
    };
    if h.owner == u32::MAX {
        return kind.to_string();
    }
    let class = words[words[6] as usize + records::LINE * h.owner as usize + 7] as usize;
    format!("{kind}:{}", obc_ground_plan::sections::Sections::embedded().rank_name(false, class))
}

fn scan_cell(key: CellKey, input: &CellInput, grid_m: f64) -> CellScan {
    use obc_ground_plan::frame::CELL_UNITS;
    let rules = Rules::embedded();
    let graph = graph::build(key, input, &rules.sections);
    let words = records::records(&graph, input, rules, key, 0, true).words;
    let o = key.origin();
    let mpu = metres_per_unit(o.y + CELL_UNITS / 2);
    let step = grid_m / mpu;
    let n = (CELL_UNITS as f64 / step).ceil() as usize;
    let eval = |q: [f64; 2]| -> Option<obc_ground_plan::faces::heights::Height> {
        let t = input.terrain.height(GlobalPoint { x: o.x + q[0].round() as i64, y: o.y + q[1].round() as i64 })?;
        let label = find(&words, q).label;
        Some(height_at(&words, q, label, t))
    };
    let lab = |q: [f64; 2]| find(&words, q).label;
    let mut grid: Vec<Option<(obc_ground_plan::faces::heights::Height, Option<u32>)>> = Vec::with_capacity(n * n);
    let t0 = std::time::Instant::now();
    let mut evals = 0u64;
    for j in 0..n {
        for i in 0..n {
            let q = [(i as f64 + 0.5) * step, (j as f64 + 0.5) * step];
            // The last row and column can lie past the cell's square: no record holds them.
            if q[0] >= CELL_UNITS as f64 || q[1] >= CELL_UNITS as f64 {
                grid.push(None);
                continue;
            }
            let h = eval(q);
            if h.is_some_and(|h| h.kind != Kind::Terrain) {
                evals += 1;
            }
            grid.push(h.map(|h| (h, lab(q))));
        }
    }
    let eval_ns = t0.elapsed().as_secs_f64() * 1e9 / evals.max(1) as f64;
    // GP6's proxy of the near mesh's vertices: a 0.8 m grid on paved faces, the level-18 grid
    // (1.63 m) on slope faces; and the time of height_at alone per kind, on a sample of them.
    let (mut paved_n, mut slope_n) = (0u64, 0u64);
    let mut sample_paved = Vec::new();
    let mut sample_slope = Vec::new();
    for (k, g) in grid.iter().enumerate() {
        let Some((h, label)) = g else { continue };
        let q = [((k % n) as f64 + 0.5) * step, ((k / n) as f64 + 0.5) * step];
        let t = input
            .terrain
            .height(GlobalPoint { x: o.x + q[0].round() as i64, y: o.y + q[1].round() as i64 })
            .unwrap_or(0.0);
        match h.kind {
            Kind::Paved => {
                paved_n += 1;
                if sample_paved.len() < 4000 && k % 7 == 0 {
                    sample_paved.push((q, *label, t));
                }
            }
            Kind::Slope => {
                slope_n += 1;
                if sample_slope.len() < 4000 && k % 7 == 0 {
                    sample_slope.push((q, *label, t));
                }
            }
            Kind::Terrain => {}
        }
    }
    let time = |v: &[([f64; 2], Option<u32>, f64)]| {
        if v.is_empty() {
            return 0.0;
        }
        let t0 = std::time::Instant::now();
        let mut acc = 0.0;
        for _ in 0..3 {
            for &(q, l, t) in v {
                acc += height_at(&words, q, l, t).h;
            }
        }
        std::hint::black_box(acc);
        t0.elapsed().as_secs_f64() * 1e9 / (3 * v.len()) as f64
    };
    let ns_paved = time(&sample_paved);
    let ns_slope = time(&sample_slope);
    let cell_area = grid_m * grid_m;
    let vertices = paved_n as f64 * cell_area / 0.64 + slope_n as f64 * cell_area / (1.63 * 1.63);
    let mesh_ms =
        (paved_n as f64 * cell_area / 0.64 * ns_paved + slope_n as f64 * cell_area / (1.63 * 1.63) * ns_slope) / 1e6;
    let mut steps = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let k = j * n + i;
            for (nb, di, dj) in
                [(i + 1 < n).then(|| k + 1).map(|v| (v, 1.0, 0.0)), (j + 1 < n).then(|| k + n).map(|v| (v, 0.0, 1.0))]
                    .into_iter()
                    .flatten()
            {
                let (Some((ha, la)), Some((hb, lb))) = (grid[k], grid[nb]) else { continue };
                if (hb.h - ha.h).abs() <= STEP_M {
                    continue;
                }
                let (mut a, mut b) = (
                    [(i as f64 + 0.5) * step, (j as f64 + 0.5) * step],
                    [(i as f64 + 0.5 + di) * step, (j as f64 + 0.5 + dj) * step],
                );
                let (mut va, mut vb) = (ha, hb);
                let found = loop {
                    let d = (vb.h - va.h).abs();
                    if d <= STEP_M {
                        break None;
                    }
                    if ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt() <= 1.0 {
                        break Some(());
                    }
                    let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                    let Some(vm) = eval(m) else { break None };
                    let (d1, d2) = ((vm.h - va.h).abs(), (vb.h - vm.h).abs());
                    if d1.max(d2) < 0.75 * d {
                        break None;
                    }
                    if d1 >= d2 {
                        (b, vb) = (m, vm);
                    } else {
                        (a, va) = (m, vm);
                    }
                };
                if found.is_some() {
                    let mut sides = [side_name(&words, &va), side_name(&words, &vb)];
                    sides.sort();
                    steps.push(Step { cell: key, q: a, dh: vb.h - va.h, edge: la != lb || lab(a) != lab(b), sides });
                }
            }
        }
    }
    // Border samples half a unit inside each border.
    let mut border = Vec::new();
    let c = CELL_UNITS as f64;
    for k in 0..n {
        let along = (k as f64 + 0.5) * step;
        for (axis, at, side) in [(0u8, 0.5, 1u8), (0u8, c - 0.5, 0u8), (1u8, 0.5, 1u8), (1u8, c - 0.5, 0u8)] {
            let q = if axis == 0 { [at, along] } else { [along, at] };
            let line = if axis == 0 { o.x + at.round() as i64 } else { o.y + at.round() as i64 };
            let line = if side == 1 { line } else { line + 1 } - if side == 1 { 0 } else { 0 };
            let line = if side == 1 { line } else { line - 1 + 1 };
            let g_along = if axis == 0 { o.y + along.round() as i64 } else { o.x + along.round() as i64 };
            // The border's own coordinate: the east cell's origin for x, the south cell's for y.
            let border_at = if axis == 0 {
                if side == 1 {
                    o.x
                } else {
                    o.x + CELL_UNITS
                }
            } else if side == 1 {
                o.y
            } else {
                o.y + CELL_UNITS
            };
            let _ = line;
            if let Some(h) = eval(q) {
                let kind = match h.kind {
                    Kind::Terrain => 0,
                    Kind::Slope => 1,
                    Kind::Paved => 2,
                };
                border.push(((axis, border_at, g_along, side), (h.h, kind)));
            }
        }
    }
    CellScan { key, points: (n * n) as u64, evals, eval_ns, steps, border, ns_paved, ns_slope, vertices, mesh_ms }
}

fn fixture_steps(out: &Path, grid_m: f64) -> Result<(), String> {
    let dir = fixture::default_dir();
    let mut fix = Fixture::open(&dir);
    let mut cells = crate::graph::fixture_cells(&fix, &dir);
    if let Ok(only) = std::env::var("HEIGHTS_CELLS") {
        let want: Vec<(u32, u32)> = only
            .split(';')
            .filter_map(|c| c.split_once(','))
            .map(|(x, y)| (x.parse().unwrap(), y.parse().unwrap()))
            .collect();
        cells.retain(|k| want.contains(&(k.x, k.y)));
    }
    let scans = fix.map(&cells, |key, input| {
        let s = scan_cell(key, &input, grid_m);
        eprintln!(
            "cell {} {} points {} evals {} {:.0} ns/eval steps {} (edge {})",
            key.x,
            key.y,
            s.points,
            s.evals,
            s.eval_ns,
            s.steps.len(),
            s.steps.iter().filter(|x| x.edge).count()
        );
        s
    });
    // Steps by the pair of sides.
    let mut by: BTreeMap<(String, String, bool), (u64, f64, Option<&Step>)> = BTreeMap::new();
    let mut all: Vec<&Step> = Vec::new();
    for s in &scans {
        for st in &s.steps {
            let e = by.entry((st.sides[0].clone(), st.sides[1].clone(), st.edge)).or_insert((0, 0.0, None));
            e.0 += 1;
            if st.dh.abs() > e.1 {
                e.1 = st.dh.abs();
                e.2 = Some(st);
            }
            all.push(st);
        }
    }
    all.sort_by(|a, b| b.dh.abs().total_cmp(&a.dh.abs()));
    let worst: Vec<serde_json::Value> = all
        .iter()
        .take(40)
        .map(|st| {
            let o = st.cell.origin();
            let (lon, lat) = crate::lon_lat(GlobalPoint { x: o.x + st.q[0] as i64, y: o.y + st.q[1] as i64 });
            json!({"lat": lat, "lon": lon, "dh": (st.dh * 1000.0).round() / 1000.0, "edge": st.edge, "sides": st.sides, "cell": [st.cell.x, st.cell.y]})
        })
        .collect();
    let classes: Vec<serde_json::Value> = by
        .iter()
        .map(|((a, b, e), (n, m, st))| {
            let at = st.map(|st| {
                let o = st.cell.origin();
                let (lon, lat) = crate::lon_lat(GlobalPoint { x: o.x + st.q[0] as i64, y: o.y + st.q[1] as i64 });
                json!([(lat * 1e6).round() / 1e6, (lon * 1e6).round() / 1e6])
            });
            json!({"sides": [a, b], "edge": e, "count": n, "length_m": *n as f64 * grid_m, "max_m": (m * 1000.0).round() / 1000.0, "worst_at": at})
        })
        .collect();
    // Borders: the same global sample from both cells.
    let mut map: BTreeMap<(u8, i64, i64), Vec<(u8, f64, u8)>> = BTreeMap::new();
    for s in &scans {
        for &((axis, at, along, side), (h, kind)) in &s.border {
            map.entry((axis, at, along)).or_default().push((side, h, kind));
        }
    }
    let (mut pairs, mut max_dh, mut over) = (0u64, 0.0f64, 0u64);
    for v in map.values() {
        if v.len() == 2 {
            pairs += 1;
            let d = (v[0].1 - v[1].1).abs();
            max_dh = max_dh.max(d);
            if d > 0.005 {
                over += 1;
            }
        }
    }
    let q = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        if v.is_empty() {
            return json!(null);
        }
        let at = |f: f64| v[((v.len() - 1) as f64 * f).round() as usize];
        json!({"median": at(0.5), "p95": at(0.95), "max": at(1.0)})
    };
    let built: Vec<&CellScan> = scans.iter().filter(|s| s.vertices > 0.0).collect();
    let cost = json!({
        "vertices": q(built.iter().map(|s| s.vertices).collect()),
        "ns_paved": q(built.iter().map(|s| s.ns_paved).filter(|v| *v > 0.0).collect()),
        "ns_slope": q(built.iter().map(|s| s.ns_slope).filter(|v| *v > 0.0).collect()),
        "mesh_ms": q(built.iter().map(|s| s.mesh_ms).collect()),
    });
    let mut ns: Vec<f64> = scans.iter().map(|s| s.eval_ns).collect();
    ns.sort_by(f64::total_cmp);
    let report = json!({
        "cells": scans.len(), "grid_m": grid_m,
        "points": scans.iter().map(|s| s.points).sum::<u64>(),
        "evals_not_terrain": scans.iter().map(|s| s.evals).sum::<u64>(),
        "ns_per_eval_median": ns.get(ns.len() / 2), "ns_per_eval_max": ns.last(),
        "cost": cost,
        "steps": all.len(), "steps_on_edges": all.iter().filter(|s| s.edge).count(),
        "steps_inside": all.iter().filter(|s| !s.edge).count(),
        "border": {"pairs": pairs, "max_dh_m": max_dh, "over_5mm": over},
        "classes": classes, "worst": worst,
    });
    std::fs::write(out.join("fixture-steps.json"), serde_json::to_string_pretty(&report).unwrap())
        .map_err(|e| e.to_string())?;
    println!("{}", serde_json::to_string_pretty(&json!({"cost": report["cost"], "steps": report["steps"], "edge": report["steps_on_edges"], "inside": report["steps_inside"], "border": report["border"], "ns": report["ns_per_eval_median"]})).unwrap());
    Ok(())
}

/// The Rust height_at against a GPU probe file of the shapes agent (rows: cell, q, label, t, h, kind)
/// over the model file that the GPU read.
fn gpu_check(model: &Path, probe: &Path) -> Result<(), String> {
    let bytes = std::fs::read(model).map_err(|e| e.to_string())?;
    let file: Vec<u32> = bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let (x0, y0, w) = (file[1], file[2], file[3]);
    let rows: serde_json::Value =
        serde_json::from_slice(&std::fs::read(probe).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let rows = rows.as_array().cloned().or_else(|| rows["rows"].as_array().cloned()).ok_or("probe rows")?;
    let (mut n, mut max_d, mut kinds_differ, mut sum) = (0u64, 0.0f64, 0u64, 0.0f64);
    let mut worst = Vec::new();
    for r in &rows {
        let (cx, cy) = (r["cell"][0].as_u64().unwrap() as u32, r["cell"][1].as_u64().unwrap() as u32);
        let at = file[5 + ((cy - y0) * w + (cx - x0)) as usize] as usize;
        if at == 0 {
            continue;
        }
        let words = &file[at..];
        let q = [r["q"][0].as_f64().unwrap(), r["q"][1].as_f64().unwrap()];
        let label = r["label"].as_u64().map(|v| v as u32);
        let (t, gh) = (r["t"].as_f64().unwrap(), r["h"].as_f64().unwrap());
        let h = height_at(words, q, label, t);
        let kind = match h.kind {
            Kind::Terrain => 0,
            Kind::Slope => 1,
            Kind::Paved => 2,
        };
        if r["kind"].as_f64().map(|k| k as i64) != Some(kind) {
            kinds_differ += 1;
        }
        let d = (h.h - gh).abs();
        n += 1;
        sum += d;
        if d > max_d {
            max_d = d;
        }
        if d > 0.002 && worst.len() < 20 {
            worst.push(json!({"cell": [cx, cy], "q": q, "rust": h.h, "gpu": gh, "kind": kind}));
        }
    }
    println!("{}", serde_json::to_string_pretty(&json!({"rows": n, "max_abs_m": max_d, "mean_abs_m": sum / n.max(1) as f64, "kinds_differ": kinds_differ, "over_2mm": worst})).unwrap());
    Ok(())
}

/// Junctions of the fixture that test the hard cases: five arms or more, a narrow V, two junctions
/// close on one link, a junction on a cell line.
fn find_cases(out: &Path) -> Result<(), String> {
    use obc_ground_plan::frame::CELL_UNITS;
    let dir = fixture::default_dir();
    let mut fix = Fixture::open(&dir);
    let cells = crate::graph::fixture_cells(&fix, &dir);
    let rules = Rules::embedded();
    let found = fix.map(&cells, |key, input| {
        let g = graph::build(key, &input, &rules.sections);
        let o = key.origin();
        let mpu = metres_per_unit(o.y + CELL_UNITS / 2);
        let ll = |p: [i32; 2]| crate::lon_lat(GlobalPoint { x: o.x + i64::from(p[0]), y: o.y + i64::from(p[1]) });
        let mut v = Vec::new();
        let js: Vec<_> = g.junctions.iter().filter(|j| j.owned).collect();
        for j in &js {
            let (lon, lat) = ll(j.pos);
            let n = j.arms.len();
            let mut min_gap = 360.0f64;
            for k in 0..n {
                let a = j.arms[k].angle;
                let b = j.arms[(k + 1) % n].angle;
                let mut d = (b - a).to_degrees();
                while d < 0.0 {
                    d += 360.0;
                }
                if n > 1 {
                    min_gap = min_gap.min(d);
                }
            }
            let border = [j.pos[0], j.pos[1], CELL_UNITS as i32 - j.pos[0], CELL_UNITS as i32 - j.pos[1]].into_iter().min().unwrap() as f64 * mpu;
            let close = js.iter().filter(|k| k.pos != j.pos).map(|k| (f64::from(k.pos[0] - j.pos[0]).hypot(f64::from(k.pos[1] - j.pos[1]))) * mpu).fold(f64::INFINITY, f64::min);
            if n >= 5 || min_gap < 20.0 || close < 15.0 || border < 3.0 {
                v.push(json!({"lat": lat, "lon": lon, "arms": n, "min_gap_deg": (min_gap * 10.0).round() / 10.0, "nearest_junction_m": (close * 10.0).round() / 10.0, "to_cell_line_m": (border * 100.0).round() / 100.0}));
            }
        }
        v
    });
    let all: Vec<serde_json::Value> = found.into_iter().flatten().collect();
    std::fs::write(out.join("cases.json"), serde_json::to_string_pretty(&all).unwrap()).map_err(|e| e.to_string())?;
    println!("{} candidates", all.len());
    Ok(())
}

/// Ways on the ground whose bake height lies more than 2 m off the plain terrain at their own position:
/// per way its tags that could explain it, the metres off and how far along.
fn offsets(out: &Path) -> Result<(), String> {
    use obc_ground_plan::frame::CELL_UNITS;
    use obc_ground_plan::graph::{Family, Level};
    let dir = fixture::default_dir();
    let mut fix = Fixture::open(&dir);
    let cells = crate::graph::fixture_cells(&fix, &dir);
    let rules = Rules::embedded();
    const KEYS: [&str; 14] = [
        "highway", "railway", "tunnel", "covered", "indoor", "layer", "level", "location", "bridge", "building",
        "man_made", "area", "name", "access",
    ];
    let found = fix.map(&cells, |key, input| {
        let g = graph::build(key, &input, &rules.sections);
        let o = key.origin();
        let mut v: Vec<serde_json::Value> = Vec::new();
        let mut stats = [0.0f64; 2];
        for way in &g.ways {
            if way.level != Level::Ground || way.family == Family::Water {
                continue;
            }
            let part = &g.parts[way.part as usize];
            let f = &input.features[part.feature.0 as usize];
            let samples = &g.samples[way.samples.start as usize..way.samples.end as usize];
            let (mut worst, mut off_m, mut len_m, mut at) = (0.0f64, 0.0, 0.0, [0, 0]);
            for w in samples.windows(2) {
                let p = w[0].pos;
                if p[0] < 0 || p[1] < 0 || p[0] >= CELL_UNITS as i32 || p[1] >= CELL_UNITS as i32 {
                    continue;
                }
                let Some(t) = input.terrain.height(GlobalPoint { x: o.x + i64::from(p[0]), y: o.y + i64::from(p[1]) }) else { continue };
                let d = w[0].z_m - t;
                let seg = (w[1].s_m - w[0].s_m).abs();
                len_m += seg;
                if d.abs() > 2.0 {
                    off_m += seg;
                }
                if d.abs() > worst.abs() {
                    worst = d;
                    at = p;
                }
            }
            stats[0] += len_m;
            stats[1] += off_m;
            if off_m > 0.0 {
                let (lon, lat) = crate::lon_lat(GlobalPoint { x: o.x + i64::from(at[0]), y: o.y + i64::from(at[1]) });
                let tags: serde_json::Map<String, serde_json::Value> =
                    f.tags.iter().filter(|(k, _)| KEYS.contains(&k.as_str())).map(|(k, v)| (k.clone(), json!(v))).collect();
                v.push(json!({"kind": f.kind, "worst_m": (worst * 100.0).round() / 100.0, "off_m": (off_m * 10.0).round() / 10.0, "len_m": (len_m * 10.0).round() / 10.0, "lat": lat, "lon": lon, "tags": tags}));
            }
        }
        (v, stats)
    });
    let mut all = Vec::new();
    let (mut total, mut off) = (0.0, 0.0);
    for (v, s) in found {
        all.extend(v);
        total += s[0];
        off += s[1];
    }
    all.sort_by(|a, b| b["off_m"].as_f64().unwrap().total_cmp(&a["off_m"].as_f64().unwrap()));
    std::fs::write(
        out.join("offsets.json"),
        serde_json::to_string_pretty(&json!({"ground_way_m": total, "off_more_than_2m_m": off, "ways": all})).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!("ground ways {total:.0} m, more than 2 m off the terrain {off:.0} m, {} ways", all.len());
    Ok(())
}

pub fn run() -> Result<(), String> {
    if std::env::args().nth(2).as_deref() == Some("--offsets") {
        let a: Vec<String> = std::env::args().collect();
        return offsets(Path::new(&a[3]));
    }
    if std::env::args().nth(2).as_deref() == Some("--find-cases") {
        let a: Vec<String> = std::env::args().collect();
        return find_cases(Path::new(&a[3]));
    }
    if std::env::args().nth(2).as_deref() == Some("--gpu-check") {
        let a: Vec<String> = std::env::args().collect();
        return gpu_check(Path::new(&a[3]), Path::new(&a[4]));
    }
    let (mut out, mut at, mut size_m, mut px, mut sections) =
        (None::<PathBuf>, Vec::new(), 64.0f64, 1280u32, vec![0.0, 45.0, 90.0, 135.0]);
    let mut names: Vec<String> = Vec::new();
    let mut fixture_grid: Option<f64> = None;
    let mut args = std::env::args().skip(2);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--out" => out = Some(value()?.into()),
            "--size-m" => size_m = value()?.parse().map_err(|_| "--size-m: a number")?,
            "--px" => px = value()?.parse().map_err(|_| "--px: a number")?,
            "--sections" => sections = value()?.split(',').map(|s| s.parse().unwrap_or(0.0)).collect(),
            "--places" => names = value()?.split(',').map(String::from).collect(),
            "--fixture-steps" => {
                fixture_grid = Some(value()?.parse::<f64>().map_err(|_| "--fixture-steps: grid metres")?)
            }
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
    if let Some(g) = fixture_grid {
        return fixture_steps(&out, g);
    }
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
