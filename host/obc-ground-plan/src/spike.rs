//! Spike 2 (never merges): the cost of `height_at` over every corner of a cell's mesh, the same loop
//! natively (`examples/spike_heights.rs`) and in WASM (the bridge's `HeightBench`).

use serde_json::{json, Value};

use crate::faces::heights::{
    height_at, height_at_shared, height_at_sides, owner_slope, spike_label, spike_rules, Height,
};
use crate::faces::records::label as label_words;
use crate::frame::{metres_per_unit_at, GlobalPoint, CELL_UNITS};
use crate::mesh::{CellMesh, NO_LABEL};
use crate::terrain::Terrain;

/// As `NORMAL_STEP_M` of `plan/corners.wgsl`.
const NORMAL_STEP_M: f64 = 0.1;
pub const RULE_NAMES: [&str; 5] = ["terrain", "slope", "owner", "blend", "kerb"];

/// One corner: point (frame units from the cell's origin), label, terrain-vertex flag.
struct Corner {
    q: [f64; 2],
    label: Option<u32>,
    terrain_vertex: bool,
}

/// A label's face may take a look other than the ground's (`may_pave` of corners.wgsl): the
/// corners that the GPU gives the four normal taps.
fn may_pave(words: &[u32], label: Option<u32>) -> bool {
    let Some(l) = label else { return false };
    let s = label_words(words, l)[0] & 0xff;
    // Carriageway, crossing, rail bed (m_paved); shoulder, corner fill, platform.
    matches!(s, 2 | 6 | 7 | 3 | 5 | 8)
}

fn corners(mesh: &CellMesh) -> Vec<Corner> {
    (0..mesh.corners.len())
        .map(|k| {
            let (p, l, tv) = mesh.corner(k);
            Corner { q: [f64::from(p[0]), f64::from(p[1])], label: (l != NO_LABEL).then_some(l), terrain_vertex: tv }
        })
        .collect()
}

fn angle_deg(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).clamp(-1.0, 1.0);
    libm::acos(d).to_degrees()
}

fn unit3(v: [f64; 3]) -> [f64; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if n > 0.0 {
        [v[0] / n, v[1] / n, v[2] / n]
    } else {
        [0.0, 0.0, 1.0]
    }
}

fn quantiles(mut v: Vec<f64>) -> Value {
    if v.is_empty() {
        return Value::Null;
    }
    v.sort_by(f64::total_cmp);
    let q = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    let over5 = v.iter().filter(|&&x| x > 5.0).count() as f64 / v.len() as f64;
    json!({"p50": q(0.5), "p95": q(0.95), "p99": q(0.99), "max": q(1.0), "n": v.len(), "share_over_5": over5})
}

/// One run of every variant over the corners of a mesh, in milliseconds; with `full` also the
/// normals' comparisons and the checks (they do not depend on the run).
pub fn run(mesh: &CellMesh, words: &[u32], terrain: &Terrain, now: &mut dyn FnMut() -> f64, full: bool) -> Value {
    let cs = corners(mesh);
    let o = mesh.cell.origin();
    let n = cs.len();
    // The plain terrain t at each corner.
    let s = now();
    let ts: Vec<Option<f64>> =
        cs.iter().map(|c| terrain.height(GlobalPoint { x: o.x + c.q[0] as i64, y: o.y + c.q[1] as i64 })).collect();
    let terrain_ms = now() - s;
    let rule = |c: &Corner| spike_label(words, c.label).0 as usize;
    // A: every corner in mesh order, one call each.
    let mut hs: Vec<f64> = vec![f64::NAN; n];
    let mut owners: Vec<u32> = vec![u32::MAX; n];
    let s = now();
    for (k, c) in cs.iter().enumerate() {
        if let Some(t) = ts[k] {
            let h: Height = height_at(words, c.q, c.label, t);
            hs[k] = h.h;
            owners[k] = h.owner;
        }
    }
    let all_ms = now() - s;
    // By rule: the corners of each rule in mesh order, timed per rule.
    let mut by_rule: [Vec<usize>; 5] = Default::default();
    for (k, c) in cs.iter().enumerate() {
        if ts[k].is_some() {
            by_rule[rule(c).min(4)].push(k);
        }
    }
    let mut rule_ms = [0.0; 5];
    let mut sink = 0.0;
    for (r, ks) in by_rule.iter().enumerate() {
        let s = now();
        for &k in ks {
            sink += height_at(words, cs[k].q, cs[k].label, ts[k].unwrap()).h;
        }
        rule_ms[r] = now() - s;
    }
    // B: rule-0 corners skip the call (they are the terrain).
    let s = now();
    for (k, c) in cs.iter().enumerate() {
        if let Some(t) = ts[k].filter(|_| rule(c) != 0) {
            sink += height_at(words, c.q, c.label, t).h;
        }
    }
    let skip_ms = now() - s;
    // C: B with the label's work shared: the rules once per block, rule and material once per label.
    let p = spike_rules(words);
    let labels = words[2] as usize;
    let mut cache: Vec<Option<(u32, u32)>> = vec![None; labels];
    let mut shared_diff = 0.0f64;
    let s = now();
    for (k, c) in cs.iter().enumerate() {
        let Some(t) = ts[k] else { continue };
        let (r, m) = match c.label {
            None => (0, u32::MAX),
            Some(l) => *cache[l as usize].get_or_insert_with(|| spike_label(words, c.label)),
        };
        if r == 0 {
            continue;
        }
        let h = height_at_shared(words, c.q, c.label, r, m, t, &p).h;
        shared_diff = shared_diff.max((h - hs[k]).abs());
    }
    let shared_ms = now() - s;
    // D: two labelled corners at one point share the records found there (`height_at_sides`).
    let mut pairs = 0usize;
    let mut sides_diff = 0.0f64;
    let s = now();
    let mut k = 0;
    while k < n {
        let mut e = k + 1;
        while e < n && cs[e].q == cs[k].q {
            e += 1;
        }
        let ks: Vec<usize> = (k..e).filter(|&j| ts[j].is_some() && rule(&cs[j]) != 0).collect();
        if ks.len() == 2 {
            let [a, b] = [ks[0], ks[1]];
            let h = height_at_sides(words, cs[a].q, [cs[a].label, cs[b].label], ts[a].unwrap());
            pairs += 1;
            sides_diff = sides_diff.max((h[0].h - hs[a]).abs()).max((h[1].h - hs[b]).abs());
        } else {
            for &j in &ks {
                sink += height_at(words, cs[j].q, cs[j].label, ts[j].unwrap()).h;
            }
        }
        k = e;
    }
    let sides_ms = now() - s;
    // Normals. The GPU's: four more calls at ±NORMAL_STEP_M per corner that may pave, in the cell.
    let paved: Vec<usize> = (0..n).filter(|&k| ts[k].is_some() && may_pave(words, cs[k].label)).collect();
    let cell = CELL_UNITS as f64;
    let mut taps_n: Vec<[f64; 3]> = Vec::with_capacity(paved.len());
    let s = now();
    for &k in &paved {
        let c = &cs[k];
        let mpu = metres_per_unit_at(f64::from(words[10]) * cell + c.q[1]);
        let step = (NORMAL_STEP_M / mpu).round().max(1.0);
        let at = |d: [f64; 2]| [(c.q[0] + d[0]).clamp(0.0, cell), (c.q[1] + d[1]).clamp(0.0, cell)];
        let h = |q: [f64; 2]| {
            let t = terrain.height(GlobalPoint { x: o.x + q[0] as i64, y: o.y + q[1] as i64 }).unwrap_or(f64::NAN);
            height_at(words, q, c.label, t).h
        };
        let (e, w, no, so) = (at([step, 0.0]), at([-step, 0.0]), at([0.0, -step]), at([0.0, step]));
        let east = (h(e) - h(w)) / ((e[0] - w[0]) * mpu);
        let north = (h(no) - h(so)) / ((so[1] - no[1]) * mpu);
        taps_n.push(unit3([-east, -north, 1.0]));
    }
    let taps_ms = now() - s;
    // (1) Area-weighted triangle normals per corner (a corner is one label's), no extra call.
    let s = now();
    let mut acc = vec![[0.0f64; 3]; n];
    let mpu0 = metres_per_unit_at(f64::from(words[10]) * cell + cell / 2.0);
    for t in &mesh.triangles {
        let v = t.map(|c| {
            let c = c as usize;
            let z = if hs[c].is_finite() { hs[c] } else { ts[c].unwrap_or(0.0) };
            [cs[c].q[0] * mpu0, -cs[c].q[1] * mpu0, z]
        });
        let (a, b) = (
            [v[1][0] - v[0][0], v[1][1] - v[0][1], v[1][2] - v[0][2]],
            [v[2][0] - v[0][0], v[2][1] - v[0][1], v[2][2] - v[0][2]],
        );
        let x = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        for &c in t {
            for i in 0..3 {
                acc[c as usize][i] += x[i];
            }
        }
    }
    let tri_n: Vec<[f64; 3]> = acc.iter().map(|&v| unit3(v)).collect();
    let tri_ms = now() - s;
    // (2) The analytic normal of an owner surface (owner and kerb rules) from its grade and bank.
    let owned: Vec<usize> = paved.iter().copied().filter(|&k| matches!(rule(&cs[k]), 2 | 4)).collect();
    let mut ana: Vec<Option<[f64; 3]>> = Vec::with_capacity(owned.len());
    let s = now();
    for &k in &owned {
        let line = spike_label(words, cs[k].label).1;
        ana.push(owner_slope(words, cs[k].q, line).map(|g| unit3([-g[0], g[1], 1.0])));
    }
    let analytic_ms = now() - s;
    sink += taps_n.iter().chain(tri_n.iter()).chain(ana.iter().flatten()).map(|v| v[2]).sum::<f64>();
    sink += shared_diff + sides_diff;
    let counts: Vec<usize> = by_rule.iter().map(Vec::len).collect();
    let mut out = json!({
        "corners": n,
        "terrain_vertices": cs.iter().filter(|c| c.terrain_vertex).count(),
        "unknown_t": ts.iter().filter(|t| t.is_none()).count(),
        "triangles": mesh.triangles.len(),
        "steps": mesh.steps.len(),
        "terrain_ms": terrain_ms,
        "all_ms": all_ms,
        "rule_ms": rule_ms,
        "rule_n": counts,
        "skip_terrain_ms": skip_ms,
        "shared_ms": shared_ms,
        "sides_ms": sides_ms,
        "sides_pairs": pairs,
        "paved_corners": paved.len(),
        "taps_ms": taps_ms,
        "tri_normals_ms": tri_ms,
        "owner_corners": owned.len(),
        "analytic_ms": analytic_ms,
        "analytic_found": ana.iter().filter(|a| a.is_some()).count(),
        "sink": sink.is_finite(),
    });
    if full {
        let pos: std::collections::BTreeMap<usize, usize> = paved.iter().enumerate().map(|(i, &k)| (k, i)).collect();
        let tri_err: Vec<f64> = paved.iter().enumerate().map(|(i, &k)| angle_deg(tri_n[k], taps_n[i])).collect();
        // Split: owner rule only, and corners on a face edge (a point with more than one corner).
        let mut shared_point = vec![false; n];
        for k in 1..n {
            if cs[k].q == cs[k - 1].q {
                shared_point[k] = true;
                shared_point[k - 1] = true;
            }
        }
        let tri_edge: Vec<f64> =
            paved.iter().enumerate().filter(|(_, &k)| shared_point[k]).map(|(i, _)| tri_err[i]).collect();
        let tri_inner: Vec<f64> =
            paved.iter().enumerate().filter(|(_, &k)| !shared_point[k]).map(|(i, _)| tri_err[i]).collect();
        let ana_err: Vec<f64> =
            owned.iter().zip(&ana).filter_map(|(&k, a)| a.map(|a| angle_deg(a, taps_n[pos[&k]]))).collect();
        let tri_owner: Vec<f64> = owned.iter().map(|&k| tri_err[pos[&k]]).collect();
        // The worst deviations, for the report: what kind of corner they are.
        let mut tri_count = vec![0u32; n];
        let mut min_area = vec![f64::INFINITY; n];
        for t in &mesh.triangles {
            let p = t.map(|c| cs[c as usize].q);
            let a = ((p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0])).abs() / 2.0
                * mpu0
                * mpu0;
            for &c in t {
                tri_count[c as usize] += 1;
                min_area[c as usize] = min_area[c as usize].min(a);
            }
        }
        let mut worst: Vec<(f64, usize, usize)> =
            paved.iter().enumerate().map(|(i, &k)| (tri_err[i], i, k)).filter(|w| w.0 > 5.0).collect();
        worst.sort_by(|a, b| b.0.total_cmp(&a.0));
        let slope = |v: [f64; 3]| libm::acos(v[2].clamp(-1.0, 1.0)).to_degrees();
        let over5: Vec<Value> = worst
            .iter()
            .take(12)
            .map(|&(e, i, k)| {
                let l = cs[k].label.unwrap();
                json!({"deg": e, "q": cs[k].q, "surface": label_words(words, l)[0] & 0xff, "rule": rule(&cs[k]),
                    "edge": shared_point[k], "tris": tri_count[k], "min_tri_m2": min_area[k],
                    "tri_slope_deg": slope(tri_n[k]), "taps_slope_deg": slope(taps_n[i])})
            })
            .collect();
        let big = |lim: f64| worst.iter().filter(|w| w.0 > lim).collect::<Vec<_>>();
        let tilt = |sel: &Vec<&(f64, usize, usize)>, taps: bool| {
            let v: Vec<f64> = sel.iter().map(|w| if taps { slope(taps_n[w.1]) } else { slope(tri_n[w.2]) }).collect();
            quantiles(v)
        };
        let b5 = big(5.0);
        let few = b5.iter().filter(|w| tri_count[w.2] <= 2).count();
        let small = b5.iter().filter(|w| min_area[w.2] < 0.05).count();
        let ana_none: Vec<usize> = owned.iter().zip(&ana).filter(|(_, a)| a.is_none()).map(|(&k, _)| k).collect();
        let ana_none_edge = ana_none.iter().filter(|&&k| shared_point[k]).count();
        out["normals_detail"] = json!({
            "over5": b5.len(), "over5_le2_tris": few, "over5_tri_lt_0.05m2": small,
            "over5_tri_slope_deg": tilt(&b5, false), "over5_taps_slope_deg": tilt(&b5, true),
            "worst": over5,
            "analytic_none": ana_none.len(), "analytic_none_edge": ana_none_edge,
            "analytic_none_surfaces": ana_none.iter().fold(std::collections::BTreeMap::<u32, usize>::new(), |mut m, &k| {
                *m.entry(label_words(words, cs[k].label.unwrap())[0] & 0xff).or_default() += 1; m }),
        });
        let fnv = hs.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, v| (h ^ v.to_bits()).wrapping_mul(0x100_0000_01b3));
        out["check"] = json!({
            "heights_fnv": format!("{fnv:016x}"),
            "shared_max_diff_m": shared_diff,
            "sides_max_diff_m": sides_diff,
            "owner_unknown": owners.iter().zip(&cs).filter(|(o, c)| **o == u32::MAX && matches!(rule(c), 2 | 4)).count(),
        });
        out["normals"] = json!({
            "tri_vs_taps_deg": quantiles(tri_err),
            "tri_vs_taps_inner_deg": quantiles(tri_inner),
            "tri_vs_taps_edge_deg": quantiles(tri_edge),
            "tri_vs_taps_owner_deg": quantiles(tri_owner),
            "analytic_vs_taps_owner_deg": quantiles(ana_err),
        });
    }
    out
}

/// The least of each timing over `runs` runs, the counts and checks of the first.
pub fn best_of(mesh: &CellMesh, words: &[u32], terrain: &Terrain, now: &mut dyn FnMut() -> f64, runs: usize) -> Value {
    let mut best = run(mesh, words, terrain, now, true);
    for _ in 1..runs {
        let r = run(mesh, words, terrain, now, false);
        for key in [
            "terrain_ms",
            "all_ms",
            "skip_terrain_ms",
            "shared_ms",
            "sides_ms",
            "taps_ms",
            "tri_normals_ms",
            "analytic_ms",
        ] {
            let v = best[key].as_f64().unwrap().min(r[key].as_f64().unwrap());
            best[key] = json!(v);
        }
        for i in 0..5 {
            let v = best["rule_ms"][i].as_f64().unwrap().min(r["rule_ms"][i].as_f64().unwrap());
            best["rule_ms"][i] = json!(v);
        }
    }
    best
}
