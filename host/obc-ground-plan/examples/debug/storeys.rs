//! `--storeys`: per place the storeys over the ground from above (`<place>-storeys.png`: the sheets' faces, their
//! free outline, the open lines, the parapets, railings and piers) and seen obliquely (`<place>-oblique.png`: the
//! cell meshes of storey 0 with every storey's mesh and shell, heights by the crate's height function, flat-shaded,
//! the storeys' triangles culled from behind), with `--gltf` the same meshes as `<place>-storeys.glb`.

use std::collections::BTreeMap;

use obc_ground_plan::cell::{build_storeys, CellBuild};
use obc_ground_plan::faces::heights::height_at;
use obc_ground_plan::faces::records;
use obc_ground_plan::faces::{Rules, Surface};
use obc_ground_plan::frame::{metres_per_unit, CellKey, GlobalPoint};
use obc_ground_plan::mesh::params::{CELLS, COARSE};
use obc_ground_plan::mesh::shell::{closure, mesh_storeys, on_paved, StoreyMesh, PARTS};
use obc_ground_plan::mesh::{mesh_cell, CellMesh, Reads, Rim, NO_LABEL};
use obc_ground_plan::tile::TileCache;
use obc_ground_plan::CellInput;
use serde_json::{json, Value};
use tiny_skia::{FillRule, Pixmap, Transform};

use crate::draw::{self, cell_square, Canvas};
use crate::fixture::Fixture;
use crate::gltf;

pub struct Cell {
    pub build: CellBuild,
    pub mesh: Option<CellMesh>,
    pub storeys: StoreyMesh,
}

/// The cells with their storeys, storey 0's mesh (with the rims of the cells about them) and the storeys' meshes.
pub fn cells(fix: &mut Fixture, keys: &[CellKey], rules: &Rules) -> Result<BTreeMap<CellKey, Cell>, String> {
    let mut cache = TileCache::new(16);
    let mut built = BTreeMap::new();
    for &key in keys {
        let b = build_storeys(fix.request(key), &mut cache, rules, &mut || 0.0)?;
        built.insert(key, b);
    }
    let rims: BTreeMap<CellKey, Rim> = built.iter().map(|(&k, b)| (k, Rim::of(&b.faces))).collect();
    let mut out = BTreeMap::new();
    for (key, b) in built {
        let around: Vec<Rim> = (-1..=1i64)
            .flat_map(|dy| {
                (-1..=1i64).map(move |dx| CellKey { x: (key.x as i64 + dx) as u32, y: (key.y as i64 + dy) as u32 })
            })
            .filter_map(|k| rims.get(&k).cloned())
            .collect();
        let mesh = mesh_cell(&b.faces, &b.records, &Reads::of(&b.faces, &around), &mut || 0.0).ok();
        let storeys = mesh_storeys(key, &b.storeys, &b.records, &rules.structures).map_err(|e| format!("{e:?}"))?;
        out.insert(key, Cell { build: b, mesh, storeys });
    }
    Ok(out)
}

/// The counts and checks of the place's cells; the triangles and corners of the storeys' meshes in the square
/// [x0, y0, x1, y1] (global frame units, every corner inside).
pub fn stats(cells: &BTreeMap<CellKey, Cell>, square: [f64; 4]) -> Value {
    let inside = |key: CellKey, p: [i32; 2]| {
        let g = global(key, p);
        (square[0]..=square[2]).contains(&(g.x as f64)) && (square[1]..=square[3]).contains(&(g.y as f64))
    };
    let mut near = [0u32; PARTS.len()];
    let mut near_corners = std::collections::BTreeSet::new();
    for (&key, c) in cells {
        let m = &c.storeys;
        for (k, n) in near.iter_mut().enumerate() {
            for t in &m.triangles[m.part(k)] {
                if t.iter().all(|&i| inside(key, m.corner(i as usize).0)) {
                    *n += 1;
                    near_corners.extend(t.iter().map(|&i| (key, i)));
                }
            }
        }
    }
    let mut parts = [0u32; PARTS.len()];
    let (mut closure_faults, mut paved, mut bytes, mut piers, mut left_out) = (Vec::new(), Vec::new(), 0, 0, 0);
    for c in cells.values() {
        for (k, n) in c.storeys.parts.iter().enumerate() {
            parts[k] += n;
        }
        closure_faults.extend(closure(&c.storeys));
        paved.extend(on_paved(&c.storeys, &c.build.storeys, &c.build.records));
        bytes += if c.storeys.corners.is_empty() { 0 } else { c.storeys.to_bytes().len() };
        piers += c.storeys.stats.piers;
        left_out += c.storeys.stats.piers_left_out;
    }
    let by_part = |v: [u32; PARTS.len()]| {
        PARTS.iter().zip(v).map(|(n, k)| (n.to_string(), json!(k))).collect::<serde_json::Map<_, _>>()
    };
    json!({"triangles": by_part(parts), "in_square": {"triangles": by_part(near), "corners": near_corners.len()},
        "piers": piers, "piers_left_out": left_out, "storey_mesh_bytes": bytes,
        "closure_faults": closure_faults, "on_paved": paved})
}

fn global(key: CellKey, p: [i32; 2]) -> GlobalPoint {
    let o = key.origin();
    GlobalPoint { x: o.x + i64::from(p[0]), y: o.y + i64::from(p[1]) }
}

/// The sheets from above: faces by surface, the free outline dark blue, the open lines green, parapets red,
/// railings orange, piers black.
pub fn top(canvas: &mut Canvas, cells: &BTreeMap<CellKey, Cell>) {
    for (&key, c) in cells {
        let mask = canvas.mask(cell_square(key));
        for plan in &c.build.storeys.plans {
            for f in plan.faces.iter().filter(|f| f.surface != Surface::Terrain) {
                let Some((r, g, b, _)) = obc_ground_plan::debug::face_colour(f.surface, false) else { continue };
                for ring in f.rings.iter().take(1) {
                    let pts: Vec<GlobalPoint> = ring.iter().map(|&v| global(key, plan.vertices[v as usize])).collect();
                    if let Some(p) = draw::path(canvas, &pts, true) {
                        canvas.pixmap.fill_path(
                            &p,
                            &draw::paint(r, g, b, 230),
                            FillRule::EvenOdd,
                            Transform::identity(),
                            mask.as_ref(),
                        );
                    }
                }
            }
        }
        let m = &c.storeys;
        let fill = |canvas: &mut Canvas, part: usize, rgb: (u8, u8, u8)| {
            for t in &m.triangles[m.part(part)] {
                let pts: Vec<GlobalPoint> = t.iter().map(|&k| global(key, m.corner(k as usize).0)).collect();
                if let Some(p) = draw::path(canvas, &pts, true) {
                    canvas.pixmap.fill_path(
                        &p,
                        &draw::paint(rgb.0, rgb.1, rgb.2, 255),
                        FillRule::Winding,
                        Transform::identity(),
                        mask.as_ref(),
                    );
                }
            }
        };
        fill(canvas, 3, (210, 40, 40));
        fill(canvas, 4, (240, 150, 0));
        fill(canvas, 5, (240, 150, 0));
        fill(canvas, 6, (20, 20, 20));
        for t in &m.triangles[m.part(2)] {
            let pts: Vec<GlobalPoint> = t.iter().map(|&k| global(key, m.corner(k as usize).0)).collect();
            if let Some(p) = draw::path(canvas, &pts, true) {
                canvas.pixmap.stroke_path(
                    &p,
                    &draw::paint(20, 40, 160, 255),
                    &draw::stroke(2.0, false),
                    Transform::identity(),
                    mask.as_ref(),
                );
            }
        }
        for o in &m.open {
            if let Some(p) = draw::path(canvas, &[global(key, o[0]), global(key, o[1])], false) {
                canvas.pixmap.stroke_path(
                    &p,
                    &draw::paint(0, 170, 60, 255),
                    &draw::stroke(3.0, false),
                    Transform::identity(),
                    mask.as_ref(),
                );
            }
        }
    }
}

/// A triangle in metres about the centre (east, north, up), its colour, and whether it shows from behind.
type Tri = ([[f64; 3]; 3], [f32; 3], bool);

fn rgb(c: (u8, u8, u8)) -> [f32; 3] {
    [f32::from(c.0) / 255.0, f32::from(c.1) / 255.0, f32::from(c.2) / 255.0]
}

/// Every triangle of the place's meshes with its heights.
fn triangles(
    cells: &BTreeMap<CellKey, Cell>,
    inputs: &BTreeMap<CellKey, CellInput>,
    centre: GlobalPoint,
    base: f64,
    half_m: f64,
) -> Vec<Tri> {
    let mpu = metres_per_unit(centre.y);
    let at = |key: CellKey, p: [i32; 2], z: f64| {
        let g = global(key, p);
        [(g.x - centre.x) as f64 * mpu, -((g.y - centre.y) as f64) * mpu, z - base]
    };
    let mut out = Vec::new();
    // The terrain under everything, a hair low: the GPU sinks it where a cell's mesh draws.
    let step = (half_m / 60.0).max(0.25);
    let n = (2.0 * half_m / step) as i64;
    let ground = |i: i64, j: i64| {
        let (e, s) = (-half_m + i as f64 * step, -half_m + j as f64 * step);
        let g = GlobalPoint { x: centre.x + (e / mpu) as i64, y: centre.y + (s / mpu) as i64 };
        inputs.get(&CellKey::of(g)).and_then(|c| c.terrain.height(g)).map(|h| [e, -s, h - base - 0.05])
    };
    // A point in R of its cell's mesh: the terrain is sunk there.
    let in_r = |e: f64, n: f64| {
        let g = GlobalPoint { x: centre.x + (e / mpu) as i64, y: centre.y - (n / mpu) as i64 };
        let key = CellKey::of(g);
        let Some(m) = cells.get(&key).and_then(|c| c.mesh.as_ref()) else { return false };
        let o = key.origin();
        let (cx, cy) = ((g.x - o.x) / i64::from(COARSE), (g.y - o.y) / i64::from(COARSE));
        let bit = ((cy + 1) * i64::from(CELLS + 2) + cx + 1) as usize;
        m.region.get(bit / 32).is_some_and(|w| w >> (bit % 32) & 1 == 1)
    };
    for j in 0..n {
        for i in 0..n {
            if let (Some(a), Some(b), Some(c), Some(d)) =
                (ground(i, j), ground(i + 1, j), ground(i, j + 1), ground(i + 1, j + 1))
            {
                if in_r((a[0] + d[0]) / 2.0, (a[1] + d[1]) / 2.0) {
                    continue;
                }
                out.push(([a, c, b], rgb((150, 175, 120)), true));
                out.push(([b, c, d], rgb((150, 175, 120)), true));
            }
        }
    }
    for (&key, c) in cells {
        let words = &c.build.records;
        let terrain = |p: [i32; 2]| inputs.get(&key).and_then(|i| i.terrain.height(global(key, p))).unwrap_or(base);
        let near = |p: [f64; 3]| p[0].abs() <= half_m && p[1].abs() <= half_m;
        if let Some(m) = &c.mesh {
            let z = |k: u32| {
                let (p, l, flag) = m.corner(k as usize);
                let t = terrain(p);
                let h = if flag || l == NO_LABEL {
                    t
                } else {
                    height_at(words, [f64::from(p[0]), f64::from(p[1])], Some(l), t).h
                };
                (at(key, p, h), l)
            };
            let colour = |l: u32| match if l == NO_LABEL { 0 } else { records::label(words, l)[0] & 0xff } {
                2 | 6 => (120, 120, 125),
                3 => (175, 170, 150),
                7 => (130, 105, 85),
                9 => (60, 110, 200),
                _ => (130, 165, 100),
            };
            let strip = m.triangles.len() - m.strip_triangles as usize;
            for (i, t) in m.triangles.iter().enumerate() {
                let v = t.map(z);
                if v.iter().all(|x| near(x.0)) {
                    let col = if i >= strip { (130, 165, 100) } else { colour(v[0].1) };
                    out.push((v.map(|x| x.0), rgb(col), true));
                }
            }
            for s in &m.steps {
                let v = s.map(z);
                if v.iter().all(|x| near(x.0)) {
                    out.push(([v[0].0, v[1].0, v[2].0], rgb((150, 140, 125)), true));
                    out.push(([v[0].0, v[2].0, v[3].0], rgb((150, 140, 125)), true));
                }
            }
        }
        let m = &c.storeys;
        let z = |k: u32| {
            let (p, l, off) = m.corner(k as usize);
            let t = terrain(p);
            let h = if l == NO_LABEL { t } else { height_at(words, [f64::from(p[0]), f64::from(p[1])], Some(l), t).h };
            at(key, p, h + off)
        };
        let colours = [
            (150, 150, 152),
            (95, 93, 88),
            (165, 162, 152),
            (200, 196, 186),
            (70, 74, 78),
            (70, 74, 78),
            (170, 166, 156),
        ];
        // The top in its surface's colour: carriageway grey, rail bed brown, shoulder pale, band blue-grey.
        let top = |l: u32| match records::label(words, l)[0] & 0xff {
            2 | 6 => (115, 115, 120),
            3 => (185, 180, 160),
            7 => (125, 100, 80),
            12 => (150, 170, 190),
            _ => (150, 150, 152),
        };
        for (k, colour) in colours.iter().enumerate() {
            for t in &m.triangles[m.part(k)] {
                let v = t.map(z);
                let colour = if k == 0 { top(m.corner(t[0] as usize).1) } else { *colour };
                if v.iter().all(|&x| near(x)) {
                    out.push((v, rgb(colour), false));
                }
            }
        }
        for (i, s) in m.slots.iter().enumerate() {
            let v = s.map(z);
            let col = rgb(if (i as u32) < m.top_slots { (140, 140, 140) } else { (95, 93, 88) });
            if v.iter().all(|&x| near(x)) {
                out.push(([v[0], v[1], v[2]], col, true));
                out.push(([v[0], v[2], v[3]], col, true));
            }
        }
    }
    out
}

/// The place seen obliquely from `heading` (degrees from north) and `pitch` (degrees down), orthographic, about the
/// height `base`: painter's order, flat shading from the north-west.
pub fn oblique(
    path: &std::path::Path,
    cells: &BTreeMap<CellKey, Cell>,
    inputs: &BTreeMap<CellKey, CellInput>,
    centre: GlobalPoint,
    size_m: f64,
    (heading, pitch): (f64, f64),
) -> Result<(), String> {
    let base = inputs.get(&CellKey::of(centre)).and_then(|i| i.terrain.height(centre)).unwrap_or(0.0);
    let tris = triangles(cells, inputs, centre, base, size_m / 2.0);
    let (h, p) = (heading.to_radians(), pitch.to_radians());
    let fwd = [libm::sin(h), libm::cos(h)];
    let right = [libm::cos(h), -libm::sin(h)];
    let view = [fwd[0] * libm::cos(p), fwd[1] * libm::cos(p), -libm::sin(p)];
    let up = [fwd[0] * libm::sin(p), fwd[1] * libm::sin(p), libm::cos(p)];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let px = 1600u32;
    let scale = f64::from(px) / size_m;
    let screen = |v: [f64; 3]| {
        let x = v[0] * right[0] + v[1] * right[1];
        ((x * scale + f64::from(px) / 2.0) as f32, (f64::from(px) / 2.0 - dot(v, up) * scale) as f32)
    };
    let light = {
        let l = [-0.5, 0.6, 0.65];
        let n = libm::sqrt(dot(l, l));
        [l[0] / n, l[1] / n, l[2] / n]
    };
    let mut items: Vec<(f64, &Tri)> =
        tris.iter().map(|t| ((dot(t.0[0], view) + dot(t.0[1], view) + dot(t.0[2], view)) / 3.0, t)).collect();
    items.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut pixmap = Pixmap::new(px, px).ok_or("a picture")?;
    pixmap.fill(tiny_skia::Color::from_rgba8(205, 220, 235, 255));
    for (_, (v, colour, two_sided)) in items {
        let (a, b) = (
            [v[1][0] - v[0][0], v[1][1] - v[0][1], v[1][2] - v[0][2]],
            [v[2][0] - v[0][0], v[2][1] - v[0][1], v[2][2] - v[0][2]],
        );
        let mut n = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let len = libm::sqrt(dot(n, n));
        if len <= 1e-12 {
            continue;
        }
        n = [n[0] / len, n[1] / len, n[2] / len];
        if dot(n, view) > 0.0 {
            if !*two_sided {
                continue;
            }
            n = [-n[0], -n[1], -n[2]];
        }
        let shade = (0.45 + 0.55 * dot(n, light).max(0.0)) as f32;
        let c = colour.map(|x| (x * shade * 255.0).clamp(0.0, 255.0) as u8);
        let mut pb = tiny_skia::PathBuilder::new();
        let s = v.map(screen);
        pb.move_to(s[0].0, s[0].1);
        pb.line_to(s[1].0, s[1].1);
        pb.line_to(s[2].0, s[2].1);
        pb.close();
        if let Some(path) = pb.finish() {
            let mut paint = draw::paint(c[0], c[1], c[2], 255);
            paint.anti_alias = false;
            pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
            pixmap.stroke_path(&path, &paint, &draw::stroke(0.8, false), Transform::identity(), None);
        }
    }
    save(&pixmap, path)
}

/// A picture's pixels: its background is opaque, so the premultiplied pixels are the straight ones.
pub fn save(pixmap: &Pixmap, path: &std::path::Path) -> Result<(), String> {
    image::save_buffer(path, pixmap.data(), pixmap.width(), pixmap.height(), image::ColorType::Rgba8)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// The place's meshes as a glTF mesh per kind: storey 0, the storeys' tops and their shells.
pub fn gltf_meshes(
    cells: &BTreeMap<CellKey, Cell>,
    inputs: &BTreeMap<CellKey, CellInput>,
    centre: GlobalPoint,
    size_m: f64,
) -> Vec<gltf::Mesh> {
    let base = inputs.get(&CellKey::of(centre)).and_then(|i| i.terrain.height(centre)).unwrap_or(0.0);
    let mut ground =
        gltf::Mesh { name: "storey 0".into(), positions: Vec::new(), colours: Vec::new(), indices: Vec::new() };
    let mut shell = gltf::Mesh {
        name: "storeys and shells".into(),
        positions: Vec::new(),
        colours: Vec::new(),
        indices: Vec::new(),
    };
    for (v, colour, two_sided) in triangles(cells, inputs, centre, base, size_m / 2.0) {
        let m = if two_sided { &mut ground } else { &mut shell };
        for p in v {
            m.indices.push(m.positions.len() as u32);
            // glTF: x east, y up, z south.
            m.positions.push([p[0] as f32, p[2] as f32, -p[1] as f32]);
            m.colours.push(colour);
        }
    }
    vec![ground, shell]
}
