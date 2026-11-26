//! The built tunnel ends of a cell's input (`obc-trailview-parts`): the openings that its ground reaches.

use obc_trailview_parts::{mouths, Chord, End, Rules};

use crate::frame::GlobalPoint;
use crate::graph::OpenEnd;
use crate::tile::Geometry;
use crate::CellInput;

/// Frame units per world unit of the viewer (2^30 per side).
const FRAME: f64 = 16.0;

/// A way that enters a built head: its point on the frame line in frame units, its unit way into the tube, and the
/// metres from its point to the jamb lines of its hole on each side (to the right negative).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mouth {
    pub at: [f64; 2],
    pub into: [f64; 2],
    pub jamb_m: [f64; 2],
}

/// The built ends of a cell's input: their openings on the ground (`open`), where the end vertex of the opening's way
/// lies in the cell grown by its margin, as the bake measures them; and the mouth of each.
pub struct Built {
    pub open: Vec<OpenEnd>,
    pub mouths: Vec<Mouth>,
}

pub fn built(input: &CellInput) -> Built {
    let rules = Rules::embedded();
    let world = |p: GlobalPoint| [p.x as f64 / FRAME, p.y as f64 / FRAME];
    let (mut ends, mut chords) = (Vec::new(), Vec::new());
    for f in &input.features {
        let Some(part) = f.parts.first() else { continue };
        let z = |k: usize| part.heights.as_ref().and_then(|h| h.get(k).copied().flatten()).unwrap_or(0.0);
        match (f.kind.as_str(), f.geometry, part.points.as_slice()) {
            ("tunnel-end", Geometry::Point, &[p]) => {
                if let Ok(end) = End::from_tags(world(p), z(0), &f.tags, rules) {
                    ends.push(end);
                }
            }
            ("tunnel-tube", Geometry::Line, &[a, b]) => {
                let (a, b) = (world(a), world(b));
                if let Ok(c) = Chord::from_tags([a[0], a[1], z(0)], [b[0], b[1], z(1)], &f.tags, rules) {
                    chords.push(c);
                }
            }
            _ => {}
        }
    }
    let [x0, y0, x1, y1] = input.key.grown();
    let (mut open, mut out) = (Vec::new(), Vec::new());
    for m in mouths(&ends, &chords, rules) {
        out.push(Mouth { at: m.at.map(|v| v * FRAME), into: m.into, jamb_m: m.jamb });
        if (x0..=x1).contains(&m.way[0]) && (y0..=y1).contains(&m.way[1]) {
            open.push(OpenEnd { way: GlobalPoint { x: m.way[0], y: m.way[1] }, shift_m: m.shift });
        }
    }
    Built { open, mouths: out }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::faces::records;
    use crate::faces::tests::{at, cell, input_of, line, plane};
    use crate::faces::{Rules, Surface};
    use crate::graph::Level;
    use crate::mesh::{mesh_cell, CellMesh, Reads};
    use crate::tile::{Feature, Part, Tags};

    /// A record of a part (`tunnel-end` at one point, `tunnel-tube` from a to b) at metres east of the cell's centre.
    fn record(kind: &str, xs: &[f64], tags: &[(&str, String)]) -> Feature {
        let points: Vec<GlobalPoint> = xs.iter().map(|&x| at([x, 0.0])).collect();
        let n = points.len();
        let own = vec![true; n.max(2) - 1];
        let part = Part {
            outer: false,
            tile: cell().feature_tiles()[0],
            own,
            points,
            heights: Some(vec![Some(500.0); n]),
            plan: None,
        };
        let tags: Tags = tags.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        let geometry = if n == 1 { Geometry::Point } else { Geometry::Line };
        Feature { index: 0, kind: kind.into(), tags, geometry, parts: vec![part] }
    }

    /// A residential road from the west into a tunnel at the cell's centre that runs 100 m east, under a hill; with
    /// `ends`, its chain's part records: an arch 6 m in.
    fn input(ends: Option<&str>) -> CellInput {
        let street = [("highway", "residential")];
        let tunnel = [("highway", "residential"), ("tunnel", "yes")];
        let lines = [
            line("road", &street, &[[-150.0, 0.0], [0.0, 0.0]]),
            line("tunnel", &tunnel, &[[0.0, 0.0], [50.0, 0.0], [100.0, 0.0]]),
        ];
        let mut input = input_of(cell(), &lines);
        input.terrain = plane(cell(), &|x, _| 500.0 + (x + 10.0).max(0.0));
        let Some(ends) = ends else { return input };
        let way = at([0.0, 0.0]);
        let s = |v: &str| v.to_string();
        let end = [
            ("part", s("arch")),
            ("heading", s("0")),
            ("section", s("round")),
            ("half", s("4.5")),
            ("height", s("5")),
            ("look", s("concrete")),
            ("collar", s("6")),
            ("face", s("7.1")),
            ("left", s("6 4 2")),
            ("right", s("3")),
            ("foot", s("-2")),
            ("way", format!("{} {}", way.x, way.y)),
            ("shift", s("6")),
        ];
        input.features.push(record("tunnel-end", &[6.0], &end));
        for (a, b) in [(6.0, 50.0), (50.0, 100.0)] {
            let tube = [
                ("section", s("round")),
                ("half", s("4.5")),
                ("height", s("5")),
                ("look", s("concrete")),
                ("class", s("residential")),
                ("road", s("3")),
                ("along", format!("{a}")),
                ("ends", s(ends)),
            ];
            input.features.push(record("tunnel-tube", &[a, b], &tube));
        }
        for (k, f) in input.features.iter_mut().enumerate() {
            f.index = k as u32;
        }
        input
    }

    /// The cell's graph and its near mesh, as the device builds them.
    fn build(input: &CellInput) -> (crate::graph::WayGraph, CellMesh) {
        let rules = Rules::embedded();
        let graph = crate::cell::cell_graph(input.key, input, rules);
        let (faces, near, _, _) = records::near_under(&graph, input, rules, input.key);
        let mesh = mesh_cell(&faces, &near.words, &Reads::of(&faces, &[]), &mut || 0.0).unwrap();
        (graph, mesh)
    }

    /// A triangle's centroid in metres east and south of the cell's centre.
    fn centroid(mesh: &CellMesh, t: &[u32; 3]) -> [f64; 2] {
        let (o, c) = (cell().origin(), at([0.0, 0.0]));
        let m = crate::faces::tests::mpu();
        let p = t.map(|k| mesh.corner(k as usize).0);
        [0, 1].map(|i| {
            let mean = p.iter().map(|q| f64::from(q[i])).sum::<f64>() / 3.0;
            (mean + [o.x, o.y][i] as f64 - [c.x, c.y][i] as f64) * m
        })
    }

    #[test]
    fn the_ground_reaches_a_built_opening() {
        let (graph, mesh) = build(&input(Some("arch arch")));
        let (_, develop) = build(&input(None));
        // The tunnel way lies on the ground up to the opening, 6 m in.
        let mut ways: Vec<(Level, f64)> = (graph.ways.iter())
            .filter(|w| graph.parts[w.part as usize].feature.0 == 1)
            .map(|w| (w.level, graph.samples[w.samples.start as usize].along_m))
            .collect();
        ways.sort_by(|a, b| a.1.total_cmp(&b.1));
        assert_eq!(ways.iter().map(|w| w.0).collect::<Vec<_>>(), [Level::Ground, Level::Tunnel], "{ways:?}");
        assert!((graph.samples.iter().any(|s| (s.along_m - 6.0).abs() < 1e-6)), "a sample at the opening");
        assert!(crate::mesh::check::validity(&mesh).is_empty(), "{:?}", crate::mesh::check::validity(&mesh));
        // Away from the opening the cell is as without the parts; the road's edges run 6 m longer, so their equal
        // pieces move along them.
        let far = |m: &CellMesh| {
            let mut p: Vec<[f64; 2]> =
                m.triangles.iter().map(|t| centroid(m, t)).filter(|p| p[0] < -40.0 && p[1].abs() > 7.0).collect();
            p.sort_by(|a, b| a.partial_cmp(b).unwrap());
            p
        };
        assert!(far(&mesh).len() > 20, "{}", far(&mesh).len());
        assert_eq!(far(&mesh), far(&develop));
    }

    #[test]
    fn the_way_widens_to_the_jamb_lines_at_a_built_opening_and_back_over_its_approach() {
        // The arch's inner half is 4.5 m, the road's core about 3.4 m: 4.2 m to each side lies in the road's core
        // just before the opening, and on the ground 4 m before it, where its own way ends at the street.
        let surface = |input: &CellInput, x: f64, y: f64| {
            let rules = Rules::embedded();
            let graph = crate::cell::cell_graph(input.key, input, rules);
            let (plan, _, _, _) = records::near_under(&graph, input, rules, input.key);
            let (o, q) = (cell().origin(), at([x, y]));
            let q = [(q.x - o.x) as f64, (q.y - o.y) as f64];
            let holds = |r: &Vec<u32>| {
                let p: Vec<[f64; 2]> = r.iter().map(|&v| plan.vertices[v as usize].map(f64::from)).collect();
                (0..p.len())
                    .filter(|&k| {
                        let (a, b) = (p[k], p[(k + 1) % p.len()]);
                        (a[1] > q[1]) != (b[1] > q[1]) && q[0] < a[0] + (q[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1])
                    })
                    .count()
                    % 2
                    == 1
            };
            let face = plan.faces.iter().find(|f| f.level == 0 && f.rings.iter().filter(|r| holds(r)).count() % 2 == 1);
            face.map(|f| f.surface)
        };
        let core = |s: Option<Surface>| matches!(s, Some(Surface::Carriageway | Surface::Shoulder | Surface::Verge));
        let (built, develop) = (input(Some("arch arch")), input(None));
        for y in [-4.2, 4.2] {
            assert!(core(surface(&built, 5.5, y)), "{:?}", surface(&built, 5.5, y));
            assert!(!core(surface(&develop, 5.5, y)), "{:?}", surface(&develop, 5.5, y));
            assert!(!core(surface(&built, 2.0, y)), "{:?}", surface(&built, 2.0, y));
        }
    }

    #[test]
    fn a_chain_with_an_end_that_is_not_built_keeps_its_tunnel_way() {
        let (graph, mesh) = build(&input(Some("arch none")));
        let (develop_graph, develop) = build(&input(None));
        assert_eq!((&graph.ways, &graph.samples), (&develop_graph.ways, &develop_graph.samples));
        assert_eq!(mesh.to_bytes(), develop.to_bytes());
    }
}
