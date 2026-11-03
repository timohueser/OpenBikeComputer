//! The glTF of a place: binary glTF 2.0 with one mesh per layer, x east, y up, z south, in metres
//! about the centre and its height.

use std::collections::BTreeMap;

use obc_ground_plan::frame::{metres_per_unit, CellKey, GlobalPoint};
use obc_ground_plan::CellInput;
use serde_json::json;

use crate::draw::shade;

/// The terrain grid's step.
const STEP_M: f64 = 0.5;

pub struct Mesh {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub colours: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

/// The terrain under the square on a grid of `STEP_M`, hillshaded; magenta at 0 m where no height is known.
pub fn terrain(cells: &BTreeMap<CellKey, CellInput>, centre: GlobalPoint, size_m: f64) -> Mesh {
    let mpu = metres_per_unit(centre.y);
    let n = (size_m / STEP_M) as usize + 1;
    let point = |i: usize, j: usize| {
        let (east, south) = (i as f64 * STEP_M - size_m / 2.0, j as f64 * STEP_M - size_m / 2.0);
        GlobalPoint { x: centre.x + (east / mpu).round() as i64, y: centre.y + (south / mpu).round() as i64 }
    };
    let height = |p: GlobalPoint| cells.get(&CellKey::of(p)).and_then(|c| c.terrain.height(p));
    let base = height(centre).unwrap_or(0.0);
    let heights: Vec<Option<f64>> = (0..n * n).map(|k| height(point(k % n, k / n))).collect();
    let at =
        |i: isize, j: isize| heights[(j.clamp(0, n as isize - 1) as usize) * n + i.clamp(0, n as isize - 1) as usize];
    let mut mesh = Mesh { name: "terrain".into(), positions: Vec::new(), colours: Vec::new(), indices: Vec::new() };
    for j in 0..n {
        for i in 0..n {
            let (ii, jj) = (i as isize, j as isize);
            let here = at(ii, jj);
            let y = here.map_or(0.0, |h| h - base);
            mesh.positions.push([
                (i as f64 * STEP_M - size_m / 2.0) as f32,
                y as f32,
                (j as f64 * STEP_M - size_m / 2.0) as f32,
            ]);
            mesh.colours.push(match here {
                None => [1.0, 0.0, 1.0],
                Some(h) => {
                    let near = |di: isize, dj: isize| at(ii + di, jj + dj).unwrap_or(h);
                    let light = 0.35
                        + 0.65
                            * shade(
                                (near(1, 0) - near(-1, 0)) / (2.0 * STEP_M),
                                (near(0, 1) - near(0, -1)) / (2.0 * STEP_M),
                            );
                    [0.8 * light as f32, 0.84 * light as f32, 0.75 * light as f32]
                }
            });
        }
    }
    for j in 0..n - 1 {
        for i in 0..n - 1 {
            // Counter-clockwise seen from above: a, its south neighbour, its east neighbour.
            let a = (j * n + i) as u32;
            let (east, south, both) = (a + 1, a + n as u32, a + n as u32 + 1);
            mesh.indices.extend([a, south, east, east, south, both]);
        }
    }
    mesh
}

/// The bytes of a binary glTF that holds the meshes.
pub fn glb(meshes: &[Mesh]) -> Vec<u8> {
    let mut bin: Vec<u8> = Vec::new();
    let (mut views, mut accessors, mut nodes, mut gltf_meshes) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut view = |bin: &mut Vec<u8>, bytes: Vec<u8>, target: u32| {
        let at = bin.len();
        bin.extend(bytes);
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        views.push(json!({"buffer": 0, "byteOffset": at, "byteLength": bin.len() - at, "target": target}));
        views.len() - 1
    };
    for (m, mesh) in meshes.iter().enumerate() {
        let floats = |v: &[[f32; 3]]| v.iter().flatten().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
        let (lo, hi) = mesh.positions.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| {
            (std::array::from_fn(|k| lo[k].min(p[k])), std::array::from_fn(|k| hi[k].max(p[k])))
        });
        let positions = view(&mut bin, floats(&mesh.positions), 34962);
        let colours = view(&mut bin, floats(&mesh.colours), 34962);
        let indices = view(&mut bin, mesh.indices.iter().flat_map(|i| i.to_le_bytes()).collect(), 34963);
        let first = accessors.len();
        accessors.push(json!({"bufferView": positions, "componentType": 5126, "count": mesh.positions.len(), "type": "VEC3", "min": lo, "max": hi}));
        accessors
            .push(json!({"bufferView": colours, "componentType": 5126, "count": mesh.colours.len(), "type": "VEC3"}));
        accessors
            .push(json!({"bufferView": indices, "componentType": 5125, "count": mesh.indices.len(), "type": "SCALAR"}));
        gltf_meshes.push(json!({"name": mesh.name, "primitives": [{"attributes": {"POSITION": first, "COLOR_0": first + 1}, "indices": first + 2, "mode": 4}]}));
        nodes.push(json!({"name": mesh.name, "mesh": m}));
    }
    let document = json!({
        "asset": {"version": "2.0", "generator": "obc-ground-plan debug"},
        "scene": 0,
        "scenes": [{"nodes": (0..nodes.len()).collect::<Vec<_>>()}],
        "nodes": nodes,
        "meshes": gltf_meshes,
        "buffers": [{"byteLength": bin.len()}],
        "bufferViews": views,
        "accessors": accessors,
    });
    let mut text = serde_json::to_vec(&document).unwrap();
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    let total = 12 + 8 + text.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend(b"glTF");
    out.extend(2u32.to_le_bytes());
    out.extend((total as u32).to_le_bytes());
    out.extend((text.len() as u32).to_le_bytes());
    out.extend(b"JSON");
    out.extend(text);
    out.extend((bin.len() as u32).to_le_bytes());
    out.extend(b"BIN\0");
    out.extend(bin);
    out
}
