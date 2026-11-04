use super::check::{neighbours, partition};
use super::*;
use crate::frame::{from_lon_lat, metres_per_unit, GlobalPoint};
use crate::graph::{build, WayGraph};
use crate::terrain::Terrain;
use crate::tile::{self, Alignment, Feature, Geometry, Tags};

fn cell() -> CellKey {
    CellKey::of(from_lon_lat(8.40, 46.82))
}

fn mpu() -> f64 {
    metres_per_unit(cell().origin().y + CELL_UNITS / 2)
}

/// A point at metres east and south of the cell's centre.
fn at([x, y]: [f64; 2]) -> GlobalPoint {
    let o = cell().origin();
    let half = CELL_UNITS / 2;
    GlobalPoint { x: o.x + half + libm::round(x / mpu()) as i64, y: o.y + half + libm::round(y / mpu()) as i64 }
}

#[derive(Clone)]
struct Line {
    kind: &'static str,
    tags: Vec<(&'static str, &'static str)>,
    points: Vec<[f64; 2]>,
    z: Vec<f64>,
}

fn line(kind: &'static str, tags: &[(&'static str, &'static str)], points: &[[f64; 2]]) -> Line {
    Line { kind, tags: tags.to_vec(), points: points.to_vec(), z: vec![500.0; points.len()] }
}

fn road(points: &[[f64; 2]]) -> Line {
    line("road", &[("highway", "residential")], points)
}

fn feature(l: &Line, tile: (u32, u32), index: u32) -> Feature {
    let points: Vec<GlobalPoint> = l.points.iter().map(|&p| at(p)).collect();
    let n = points.len();
    let chord = |i: usize, j: usize| {
        libm::atan2((points[j].y - points[i].y) as f64, (points[j].x - points[i].x) as f64)
            .rem_euclid(core::f64::consts::TAU)
    };
    let tangent: Vec<f64> = (0..n)
        .map(|i| match i {
            0 => chord(0, 1),
            i if i == n - 1 => chord(n - 2, n - 1),
            i => chord(i - 1, i + 1),
        })
        .collect();
    let plan = Some(Alignment {
        along_m: 0.0,
        backward: false,
        tangent,
        grade: vec![0.0; n],
        bank: vec![0.0; n],
        widen: vec![0.0; n],
        side: vec![0; n],
        walk: vec![0; n],
    });
    let tags: Tags = l.tags.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let heights = Some(l.z.iter().map(|&z| Some(z)).collect());
    let part = tile::Part { outer: false, tile, own: vec![true; n - 1], points, heights, plan };
    Feature { index, kind: l.kind.into(), tags, geometry: Geometry::Line, parts: vec![part] }
}

fn input_of(key: CellKey, lines: &[Line]) -> CellInput {
    let tile = cell().feature_tiles()[0];
    CellInput {
        key,
        features: lines.iter().zip(0..).map(|(l, index)| feature(l, tile, index)).collect(),
        terrain: Terrain::new(key, Vec::new()),
    }
}

/// The plan of a cell from lines, with check 1 at 0 faults.
fn plan_of(key: CellKey, lines: &[Line]) -> (FacePlan, WayGraph) {
    let input = input_of(key, lines);
    let rules = Rules::embedded();
    let graph = build(key, &input, &rules.sections);
    let plan = build_faces(&graph, &input, rules, key, 0, true);
    let faults = partition(&plan, window(key, 0.0));
    assert!(faults.is_empty(), "{} faults: {:?}", faults.len(), &faults[..faults.len().min(10)]);
    (plan, graph)
}

fn plan(lines: &[Line]) -> FacePlan {
    plan_of(cell(), lines).0
}

fn count(plan: &FacePlan, surface: Surface) -> usize {
    plan.faces.iter().filter(|f| f.surface == surface).count()
}

#[test]
fn an_empty_cell_is_one_terrain_face() {
    let p = plan(&[]);
    assert!(p.faces.iter().all(|f| f.surface == Surface::Terrain));
    assert_eq!(p.faces.len(), 1);
}

#[test]
fn a_straight_road_gives_a_carriageway_shoulders_and_slopes() {
    let p = plan(&[road(&[[-300.0, 10.0], [300.0, 12.0]])]);
    assert!(count(&p, Surface::Carriageway) >= 1);
    assert!(count(&p, Surface::Shoulder) >= 1);
    assert!(count(&p, Surface::Slope) >= 1);
}
