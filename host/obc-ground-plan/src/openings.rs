//! The built tunnel ends of a cell's input (`obc-trailview-parts`): the openings that its ground reaches, and the cut of
//! each end, where the bake found ground inside its part: the mesh leaves the cut open and draws no wall of its steps.

use obc_trailview_parts::{built_ends, Chord, End, Rules};

use crate::frame::{CellKey, GlobalPoint};
use crate::graph::OpenEnd;
use crate::tile::Geometry;
use crate::CellInput;

/// Frame units per world unit of the viewer (2^30 per side).
const FRAME: f64 = 16.0;

/// The cut of one end, in frame units from the cell's origin: its rings (a point lies in the cut where an odd number of
/// them hold it), and its walls: the stretches of face edges where the wall of a step passes through the part.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cut {
    pub rings: Vec<Vec<[i64; 2]>>,
    pub walls: Vec<[[i64; 2]; 2]>,
}

/// The opening of a built end: its point in frame units, its unit way into the tube, and its inner half-width in
/// metres, to the jamb lines of its head wall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mouth {
    pub at: [f64; 2],
    pub into: [f64; 2],
    pub half_m: f64,
}

/// The built ends of a cell's input: their openings on the ground (`open`), where the end vertex of the opening's way
/// lies in the cell grown by its margin, as the bake measures them; the cut of each end that has one; and the mouth of
/// each.
pub struct Built {
    pub open: Vec<OpenEnd>,
    pub cuts: Vec<Cut>,
    pub mouths: Vec<Mouth>,
}

pub fn built(input: &CellInput) -> Built {
    let rules = Rules::embedded();
    let world = |p: GlobalPoint| [p.x as f64 / FRAME, p.y as f64 / FRAME];
    let (mut ends, mut cuts, mut chords) = (Vec::new(), Vec::new(), Vec::new());
    for f in &input.features {
        let Some(part) = f.parts.first() else { continue };
        let z = |k: usize| part.heights.as_ref().and_then(|h| h.get(k).copied().flatten()).unwrap_or(0.0);
        match (f.kind.as_str(), f.geometry, part.points.as_slice()) {
            ("tunnel-end", Geometry::Point, &[p]) => {
                if let Ok(end) = End::from_tags(world(p), z(0), &f.tags, rules) {
                    ends.push(end);
                    cuts.push((f.tags.get("cut"), f.tags.get("walls"), p));
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
    let built = built_ends(&ends, &chords, rules);
    let [x0, y0, x1, y1] = input.key.grown();
    let mut open = Vec::new();
    let mut out = Vec::new();
    let mut mouths = Vec::new();
    for ((end, cut), _) in ends.iter().zip(&cuts).zip(&built).filter(|(_, &b)| b) {
        mouths.push(Mouth { at: end.at.map(|v| v * FRAME), into: end.way_in(), half_m: end.half });
        let ways = [Some((end.way, end.shift)), end.mate_way.zip(end.mate_shift)];
        for (way, shift_m) in ways.into_iter().flatten() {
            if (x0..=x1).contains(&way[0]) && (y0..=y1).contains(&way[1]) {
                open.push(OpenEnd { way: GlobalPoint { x: way[0], y: way[1] }, shift_m });
            }
        }
        let &(rings_text, walls_text, p) = cut;
        let rings = rings_text.map_or(Some(Vec::new()), |text| rings(text, p, input.key));
        let walls = walls_text.map_or(Some(Vec::new()), |text| walls(text, p, input.key));
        if let (Some(rings), Some(walls)) = (rings, walls) {
            if !rings.is_empty() || !walls.is_empty() {
                out.push(Cut { rings, walls });
            }
        }
    }
    Built { open, cuts: out, mouths }
}

/// A point `x y` of a tag of an end at frame point `p`, in frame units from the cell's origin.
fn point(q: &str, p: GlobalPoint, cell: CellKey) -> Option<[i64; 2]> {
    let (o, (x, y)) = (cell.origin(), q.split_once(' ')?);
    Some([p.x + x.parse::<i64>().ok()? - o.x, p.y + y.parse::<i64>().ok()? - o.y])
}

/// The rings of a tag `cut` of an end at frame point `p`; None where the tag does not read.
fn rings(text: &str, p: GlobalPoint, cell: CellKey) -> Option<Vec<Vec<[i64; 2]>>> {
    text.split(';').map(|ring| ring.split(',').map(|q| point(q, p, cell)).collect()).collect()
}

/// The walls of a tag `walls` of an end at frame point `p`; None where the tag does not read.
fn walls(text: &str, p: GlobalPoint, cell: CellKey) -> Option<Vec<[[i64; 2]; 2]>> {
    (text.split(';'))
        .map(|wall| {
            let (a, b) = wall.split_once(',')?;
            Some([point(a, p, cell)?, point(b, p, cell)?])
        })
        .collect()
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

    /// The tag `cut` of one ring of points in metres in and to the left from an end 6 m east of the cell's centre,
    /// heading east: frame units from its point.
    fn cut(ring: [[f64; 2]; 4]) -> String {
        let m = crate::faces::tests::mpu();
        let point = |[u, v]: [f64; 2]| format!("{} {}", libm::round(u / m), libm::round(-v / m));
        ring.map(point).join(",")
    }

    /// A residential road from the west into a tunnel at the cell's centre that runs 100 m east, under a hill; with
    /// `ends`, its chain's part records: an arch 6 m in, whose cut runs from 2 m before its opening to 3 m in and 4 m to
    /// each side, across the road.
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
            ("cut", cut([[-2.0, -4.0], [3.0, -4.0], [3.0, 4.0], [-2.0, 4.0]])),
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
        let (mut faces, near, _, _) = records::near_under(&graph, input, rules, input.key);
        faces.cuts = built(input).cuts;
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
    fn the_ground_reaches_a_built_opening_and_leaves_its_cut_open() {
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
        // No triangle in the cut, the ring's edges in the mesh, and ground round it.
        assert!(!mesh.cut.is_empty());
        let cut = |p: [f64; 2]| (4.0..9.0).contains(&p[0]) && (-4.0..4.0).contains(&p[1]);
        let centroids: Vec<[f64; 2]> = mesh.triangles.iter().map(|t| centroid(&mesh, t)).collect();
        assert!(!centroids.iter().any(|&p| cut(p)), "a triangle in the cut");
        assert!(centroids.iter().any(|&p| (3.0..4.0).contains(&p[0]) && p[1].abs() < 4.0), "the road up to the cut");
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
    fn a_wall_of_the_cut_takes_the_step_slots_on_it_and_splits_its_edge_at_its_end() {
        let input = input(Some("arch arch"));
        let mesh = |walls: Vec<[[i64; 2]; 2]>| {
            let rules = Rules::embedded();
            let graph = crate::cell::cell_graph(input.key, &input, rules);
            let (mut faces, near, _, _) = records::near_under(&graph, &input, rules, input.key);
            faces.cuts = built(&input).cuts;
            faces.cuts[0].walls = walls;
            mesh_cell(&faces, &near.words, &Reads::of(&faces, &[]), &mut || 0.0).unwrap()
        };
        let slots = |m: &CellMesh| -> Vec<[[i32; 2]; 2]> {
            let mut out: Vec<_> = (m.steps.iter())
                .map(|s| {
                    let (a, b) = (m.corner(s[0] as usize).0, m.corner(s[1] as usize).0);
                    if a < b {
                        [a, b]
                    } else {
                        [b, a]
                    }
                })
                .collect();
            out.sort_unstable();
            out
        };
        let all = slots(&mesh(Vec::new()));
        // A step slot of the road's cut into the hill, before the opening.
        let east = |p: [i32; 2]| {
            (f64::from(p[0]) + (cell().origin().x - at([0.0, 0.0]).x) as f64) * crate::faces::tests::mpu()
        };
        let &[a, b] =
            all.iter().find(|s| (-6.0..2.0).contains(&east(s[0])) && (-6.0..2.0).contains(&east(s[1]))).unwrap();
        let wide = |p: [i32; 2]| p.map(i64::from);
        let one = slots(&mesh(vec![[wide(a), wide(b)]]));
        assert_eq!(one, all.iter().copied().filter(|&s| s != [a, b]).collect::<Vec<_>>());
        // A wall to the piece's middle: the edge splits there, the half on the wall goes and the other half stays.
        let mid = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
        let half = slots(&mesh(vec![[wide(a), wide(mid)]]));
        assert_eq!(half.len(), all.len(), "{half:?}");
        assert!(half.contains(&if mid < b { [mid, b] } else { [b, mid] }) && !half.contains(&[a, b]), "{half:?}");
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
        let (graph, mesh) = build(&input(Some("arch box")));
        let (develop_graph, develop) = build(&input(None));
        assert_eq!((&graph.ways, &graph.samples), (&develop_graph.ways, &develop_graph.samples));
        assert_eq!(mesh.to_bytes(), develop.to_bytes());
    }
}
