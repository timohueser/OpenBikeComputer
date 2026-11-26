//! The built tunnel ends of a cell's input (`obc-trailview-parts`): the openings that its ground reaches.

use obc_trailview_parts::{mouths, Chord, End, Rules};

use crate::frame::GlobalPoint;
use crate::graph::OpenEnd;
use crate::tile::Geometry;
use crate::CellInput;

/// Frame units per world unit of the viewer (2^30 per side).
const FRAME: f64 = 16.0;

/// A way that enters a built head: its point on the frame line in frame units, its unit way into the tube, the
/// metres from its point to the jamb lines of its hole on each side (to the right negative), the end vertex of the way
/// and the metres along the way from it to the opening (`OpenEnd`), whether its front is clear (`graph::forecourt`),
/// its room, which the lanes of one hole share, and the room's floor at the frame line and at the front of the head
/// wall, `recess_m` out (m).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mouth {
    pub at: [f64; 2],
    pub into: [f64; 2],
    pub jamb_m: [f64; 2],
    pub way: GlobalPoint,
    pub shift_m: f64,
    pub clear: bool,
    pub room: usize,
    pub floor_m: [f64; 2],
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
        let way = GlobalPoint { x: m.way[0], y: m.way[1] };
        out.push(Mouth {
            at: m.at.map(|v| v * FRAME),
            into: m.into,
            jamb_m: m.jamb,
            way,
            shift_m: m.shift,
            clear: m.clear,
            room: m.room,
            floor_m: m.floor,
        });
        if (x0..=x1).contains(&m.way[0]) && (y0..=y1).contains(&m.way[1]) {
            open.push(OpenEnd { way, shift_m: m.shift });
        }
    }
    Built { open, mouths: out }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::faces::records;
    use crate::faces::tests::{at, cell, input_of, line, mpu, plane, track, Line};
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
        input_beside(ends, (0.0, 0.0), [-150.0, 0.0], ("road", "residential"), &[])
    }

    /// `input` with the way (kind, highway) from `from` to its vertex `vertex` metres east and the tunnel way on from
    /// there, `y` metres beside the records' axis, and the ways `side` first.
    fn input_beside(
        ends: Option<&str>,
        (y, vertex): (f64, f64),
        from: [f64; 2],
        (kind, highway): (&'static str, &'static str),
        side: &[Line],
    ) -> CellInput {
        let street = [("highway", highway)];
        let tunnel = [("highway", highway), ("tunnel", "yes")];
        let mut lines = side.to_vec();
        lines.push(line(kind, &street, &[from, [vertex, y]]));
        lines.push(line("tunnel", &tunnel, &[[vertex, y], [50.0, y], [100.0, y]]));
        let mut input = input_of(cell(), &lines);
        input.terrain = plane(cell(), &|x, _| 500.0 + (x + 10.0).max(0.0));
        let Some(ends) = ends else { return input };
        let way = at([vertex, y]);
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
            ("shift", format!("{}", 6.0 - vertex)),
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
    fn a_clear_front_is_the_forecourt_s_floor_across_the_reveal() {
        // The reveal is the arch's inner half, 4.5 m, and the frame, 0.45 m. The terrain rises 16 m over the 16 m
        // before the opening, so every slope there stands high over the road. The way meets its opening on its axis,
        // 1.5 m beside it, as a lane of a room meets the bake's lane, and at 60 degrees, as a way that turns there; a
        // footway, whose class is far narrower than the reveal; a road whose vertex is the opening (shift 0), into a
        // room whose floor rises 0.1 m per metre, with a track from that vertex that falls 4 m over its 14 m; a road
        // that ends at a T on a street that passes its vertex, where no way carries it on (`on` false); and that T at
        // the opening, into the rising floor, where the lane has no ground way, on main roads, which wear lines.
        let (road, footway, main) = (("road", "residential"), ("path", "footway"), ("main-road", "secondary"));
        let mut side = track(&[[6.0, 0.0], [-4.0, 10.0]]);
        side.z = vec![500.0, 496.0];
        let north = |x: f64, (kind, highway): (&'static str, &'static str)| {
            line(kind, &[("highway", highway)], &[[x, 0.0], [x, 10.0]])
        };
        let (north, at_opening) = (north(0.0, road), north(6.0, main));
        let design = &obc_trailview_parts::Rules::embedded().design;
        let (head, reveal) = (6.0 - design.recess_m, 4.5 + design.frame_m);
        for (beside, vertex, from, way, side, grade, on) in [
            (0.0, 0.0, [-150.0, 0.0], road, &[][..], 0.0, true),
            (1.5, 0.0, [-150.0, 1.5], road, &[], 0.0, true),
            (0.0, 0.0, [-10.0, 17.32], road, &[], 0.0, true),
            (0.0, 0.0, [-150.0, 0.0], footway, &[], 0.0, true),
            (0.0, 6.0, [-150.0, 0.0], road, std::slice::from_ref(&side), 0.1, true),
            (0.0, 0.0, [0.0, -10.0], road, std::slice::from_ref(&north), 0.0, false),
            (0.0, 6.0, [6.0, -10.0], main, std::slice::from_ref(&at_opening), 0.1, false),
        ] {
            // The case with its records `drop` metres lower.
            let built = |drop: f64| {
                let mut input = input_beside(Some("arch arch"), (beside, vertex), from, way, side);
                for f in input.features.iter_mut().filter(|f| f.kind.starts_with("tunnel-")) {
                    let east = |p: &GlobalPoint| (p.x - at([0.0, 0.0]).x) as f64 * mpu();
                    let part = &mut f.parts[0];
                    part.heights =
                        Some(part.points.iter().map(|p| Some(500.0 - drop + grade * (east(p) - 6.0))).collect());
                    if f.kind == "tunnel-end" {
                        f.tags.insert("front".into(), "1".into());
                    }
                }
                input
            };
            let (built, lower) = (built(0.0), built(0.5));
            let rules = Rules::embedded();
            let (graph, mesh) = build(&built);
            assert!(crate::mesh::check::validity(&mesh).is_empty(), "{:?}", crate::mesh::check::validity(&mesh));
            // The recess is a face of its own: no triangle over the reveal spans the front of the head wall, so the
            // drawn ground over the recess is the floor at its corners, which the opening's box keeps.
            for t in &mesh.triangles {
                let p = t.map(|k| centroid(&mesh, &[k; 3]));
                let spans = p.iter().any(|q| q[0] < head - 0.01) && p.iter().any(|q| q[0] > head + 0.01);
                assert!(!(spans && p.iter().all(|q| q[1].abs() <= reveal + 0.01)), "{from:?} {way:?}: {p:?}");
            }
            let floor = graph.mouths[0].floor_m;
            assert!((floor[0] - floor[1] - grade * design.recess_m).abs() < 1e-3, "{floor:?}");
            let near = records::near_under(&graph, &built, rules, built.key).1;
            let lowered =
                records::near_under(&crate::cell::cell_graph(lower.key, &lower, rules), &lower, rules, lower.key);
            let o = cell().origin();
            let height = |words: &[u32], x: f64, y: f64| {
                let q = at([x, y]);
                let q = [(q.x - o.x) as f64, (q.y - o.y) as f64];
                crate::faces::heights::check::at(words, &built.terrain, built.key, q).expect("a height").0
            };
            // The floor across the reveal on the frame line and at the front of the head wall.
            for y in (-19..=19).map(|k| f64::from(k) * 0.25) {
                for (x, z) in [(6.0 - 0.01, floor[0]), (head + 0.01, floor[1]), (head - 0.01, floor[1])] {
                    let h = height(&near.words, x, y);
                    assert_eq!(h.kind, crate::faces::heights::Kind::Paved, "{from:?} {x} {y}: {h:?}");
                    assert!((h.h - z).abs() < 0.005, "{from:?} {x} {y}: {} over the floor {z}", h.h - z);
                }
            }
            // Its lanes' ground ways are the tunnel way's open stretch, where the opening lies past the vertex, and the
            // road where it carries the lane on, never the side way: their frames its paint takes, the road's look (else
            // the open stretch's) and heights it keeps outward. With neither, it frames its own line with the tunnel
            // way's look without lines, and its heights run the floor on at its grade.
            let f = &graph.forecourts[0];
            let feature = |w: u32| graph.parts[graph.ways[w as usize].part as usize].feature.0 as usize;
            let mut lanes: Vec<usize> = f.frames.iter().map(|&w| feature(w)).collect();
            lanes.sort_unstable();
            let (street, open) = (side.len(), vertex < 6.0);
            let want = [(on, street), (open, street + 1)].into_iter().filter(|w| w.0).map(|w| w.1);
            let part = &graph.parts[graph.ways[f.way as usize].part as usize];
            assert_eq!(part.feature.0 as usize, if on { street } else { street + 1 }, "{from:?}");
            if open || on {
                assert_eq!(lanes, want.collect::<Vec<_>>(), "{from:?} {f:?}");
                assert!(
                    part.vertices[3..].iter().all(|v| (v.z_m - 500.0).abs() < 1e-9),
                    "{from:?} {:?}",
                    part.vertices
                );
            } else {
                assert_eq!(f.frames, [f.way], "{from:?}");
                let run_on = |d: f64| floor[0] + (floor[1] - floor[0]) / design.recess_m * d;
                let samples = graph.ways[f.way as usize].samples.clone();
                let off: Vec<f64> =
                    samples.map(|k| &graph.samples[k as usize]).map(|s| s.z_m - run_on(s.along_m)).collect();
                assert!(off.iter().all(|z| z.abs() < 1e-9), "{from:?} {off:?}");
            }
            // Its records end square on its start: behind it, a point that its outline's cones do not hold keeps its
            // height when the floor drops.
            let way = |line: u32| near.words[near.words[6] as usize + records::LINE * line as usize];
            let mut held = 0;
            for x in [6.01, 6.5, 7.0, 8.0] {
                for y in (-16..=16).map(|k| f64::from(k) * 0.5) {
                    let h = height(&near.words, x, y);
                    if h.owner == u32::MAX || way(h.owner) != f.way {
                        assert_eq!(h, height(&lowered.1.words, x, y), "{from:?} {x} {y}");
                        held += 1;
                    }
                }
            }
            assert!(held > 0, "{from:?}");
            // Its paint takes its frames from those ways, never from its own line, which its heights follow.
            let q = at([3.0, 0.0]);
            let id = records::find(&near.words, [(q.x - o.x) as f64, (q.y - o.y) as f64]).expect("a label");
            let label = records::label(&near.words, id);
            let mut hosts: Vec<u32> = label[4..8].iter().filter(|&&l| l != u32::MAX).map(|&l| way(l)).collect();
            hosts.sort_unstable();
            let mut frames = f.frames.clone();
            frames.sort_unstable();
            assert_eq!((way(label[2]), &hosts), (f.way, &frames), "{from:?}");
            if !(open || on) {
                let finish = near.words[near.words[6] as usize + records::LINE * label[2] as usize + 1];
                assert_eq!(finish & records::MARKED, 0, "{from:?}");
            }
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
