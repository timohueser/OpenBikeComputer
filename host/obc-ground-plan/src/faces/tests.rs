use super::check::partition;
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
    /// Per vertex, the sides of the segment from it on with a sidewalk: 1 left, 2 right.
    walk: Vec<u8>,
}

fn line(kind: &'static str, tags: &[(&'static str, &'static str)], points: &[[f64; 2]]) -> Line {
    let n = points.len();
    Line { kind, tags: tags.to_vec(), points: points.to_vec(), z: vec![500.0; n], walk: vec![0; n] }
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
        walk: l.walk.clone(),
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
    let plan = super::build(&graph, &input, rules, key, 0, true);
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

/// The surface at a point in metres from the cell's centre.
fn surface_at(p: &FacePlan, m: [f64; 2]) -> Surface {
    let g = at(m);
    let o = cell().origin();
    let q = [2 * (g.x - o.x), 2 * (g.y - o.y)];
    let face = p.faces.iter().find(|f| {
        let rings: Vec<Vec<[i32; 2]>> =
            f.rings.iter().map(|r| r.iter().map(|&v| p.vertices[v as usize]).collect()).collect();
        geom::winding(&rings, q) != 0
    });
    face.map_or(Surface::Terrain, |f| f.surface)
}

#[test]
fn the_land_between_hairpin_legs_stays_land_past_the_turns_corner_zone() {
    // Residential: half-width 2.5 m, inner radius 3 m. Legs 3 m apart (under 2r) and 8 m apart.
    let h = 2.5;
    for gap in [3.0, 8.0] {
        let r = gap / 2.0 + h;
        let mut points = vec![[-r, 80.0], [-r, 0.0]];
        for k in 1..12 {
            let a = core::f64::consts::PI * f64::from(k) / 12.0;
            points.push([-r * libm::cos(a), -r * libm::sin(a)]);
        }
        points.extend([[r, 0.0], [r, 80.0]]);
        let p = plan(&[road(&points)]);
        for y in [20.0, 30.0, 45.0, 60.0] {
            assert_ne!(surface_at(&p, [0.0, y]), Surface::Carriageway, "legs {gap} m apart, {y} m from the turn");
        }
        assert_eq!(surface_at(&p, [0.0, -r]), Surface::Carriageway, "legs {gap} m apart: the turn");
    }
}

#[test]
fn a_junction_corner_takes_the_lesser_class_radius_and_the_sidewalk_one_on_a_sidewalk_side() {
    // A residential road runs south from a tertiary one, with a sidewalk on its left: the east.
    let host = line("road", &[("highway", "tertiary")], &[[-80.0, 0.0], [0.0, 0.0], [80.0, 0.0]]);
    let mut guest = road(&[[0.0, 0.0], [0.0, 80.0]]);
    guest.walk[0] = 1;
    let p = plan(&[host, guest]);
    let sections = &Rules::embedded().sections;
    let class = |name: &str| sections.way("road", &[("highway".into(), name.into())].into(), false);
    let (tertiary, residential) = (class("tertiary"), class("residential"));
    let o = cell().origin();
    let centre = at([0.0, 0.0]);
    let radius = |east: bool| {
        let c = p.corners.iter().find(|c| (o.x + i64::from(c.centre[0]) > centre.x) == east && c.radius_m > 0.0);
        c.map(|c| c.radius_m).unwrap()
    };
    let east = residential.corner_radius_walk_m.min(tertiary.corner_radius_walk_m);
    let west = residential.corner_radius_m.min(tertiary.corner_radius_m);
    assert!(east < west);
    assert!((radius(true) - east).abs() < 0.01, "{} m east", radius(true));
    assert!((radius(false) - west).abs() < 0.01, "{} m west", radius(false));
}

#[test]
fn a_round_has_its_feet_on_its_circle_and_on_the_edges_it_joins() {
    // A T of two ways of one class: a primary one (9 m rounds) and a residential one (4 m).
    let sections = &Rules::embedded().sections;
    for name in ["primary", "residential"] {
        let host = line("road", &[("highway", name)], &[[-80.0, 0.0], [0.0, 0.0], [80.0, 0.0]]);
        let guest = line("road", &[("highway", name)], &[[0.0, 0.0], [0.0, 80.0]]);
        let p = plan(&[host, guest]);
        let class = sections.way("road", &[("highway".into(), name.into())].into(), false);
        let (o, centre) = (cell().origin(), at([0.0, 0.0]));
        let (cx, cy) = ((centre.x - o.x) as i32, (centre.y - o.y) as i32);
        let h = class.half_m / mpu();
        let rounds: Vec<&CornerArc> = p.corners.iter().filter(|c| c.radius_m > 0.0).collect();
        assert_eq!(rounds.len(), 2, "{name}");
        for c in rounds {
            assert!((c.radius_m - class.corner_radius_m).abs() < 0.01, "{name}: a {} m round", c.radius_m);
            let r = c.radius_m / mpu();
            let off = |q: &[i32; 2]| libm::hypot(f64::from(q[0] - c.centre[0]), f64::from(q[1] - c.centre[1])) - r;
            for q in c.arc.iter().chain([&c.t_a, &c.t_b]) {
                assert!(off(q).abs() < 2.0, "{name}: a point {} units off its circle", off(q));
            }
            // One foot on the host's edge, the other on the guest's edge on the round's side.
            let side = (c.centre[0] - cx).signum();
            let on_host = |q: &[i32; 2]| (f64::from((q[1] - cy).abs()) - h).abs() < 2.0;
            let on_guest =
                |q: &[i32; 2]| (q[0] - cx).signum() == side && (f64::from((q[0] - cx).abs()) - h).abs() < 2.0;
            let [a, b] = [c.t_a, c.t_b];
            assert!((on_host(&a) && on_guest(&b)) || (on_host(&b) && on_guest(&a)), "{name}: feet {a:?} {b:?}");
        }
    }
}

#[test]
fn two_cells_round_a_point_alike_where_its_arm_ends_at_a_point_past_the_window() {
    // A primary road runs east over the cell's east border. A guest leaves it 40 m past the border
    // and another one 10 m on, 50 m past it: the first junction's east arm ends at the second, so its
    // corner there takes a round under the class's 9 m.
    let x = CELL_UNITS as f64 / 2.0 * mpu();
    let primary = |points: &[[f64; 2]]| line("road", &[("highway", "primary")], points);
    let lines = [
        primary(&[[x - 30.0, 0.0], [x + 40.0, 0.0], [x + 50.0, 0.0], [x + 100.0, 0.0]]),
        primary(&[[x + 40.0, 0.0], [x + 40.0, 60.0]]),
        primary(&[[x + 50.0, 0.0], [x + 50.0, -60.0]]),
    ];
    let p = at([x + 40.0, 0.0]);
    let rounds = |key: CellKey| {
        let (plan, graph) = plan_of(key, &lines);
        let o = key.origin();
        let d = |q: [i32; 2]| (o.x + i64::from(q[0]) - p.x).pow(2) + (o.y + i64::from(q[1]) - p.y).pow(2);
        let j = (0..graph.junctions.len()).min_by_key(|&j| d(graph.junctions[j].pos)).unwrap() as u32;
        let mut out: Vec<([i64; 2], i64)> = (plan.corners.iter())
            .filter(|c| c.owner == Owner::Junction(j))
            .map(|c| {
                ([o.x + i64::from(c.centre[0]), o.y + i64::from(c.centre[1])], libm::round(100.0 * c.radius_m) as i64)
            })
            .collect();
        out.sort_unstable();
        out
    };
    let (west, east) = (rounds(cell()), rounds(CellKey { x: cell().x + 1, y: cell().y }));
    assert_eq!(west, east);
    assert!(west.iter().any(|&(_, r)| r > 0 && r < 900), "{west:?}");
}

#[test]
fn two_cells_round_a_point_alike_where_its_round_meets_a_way_far_past_the_window() {
    // A primary road runs east over the cell's east border, and a guest leaves it 45 m past the
    // border to the north. A residential road starts 14 m north of the host, 67 m past the border,
    // past the carriageways that the west cell builds: the junction's 9 m round there meets its
    // carriageway, so it is smaller in both cells.
    let x = CELL_UNITS as f64 / 2.0 * mpu();
    let primary = |points: &[[f64; 2]]| line("road", &[("highway", "primary")], points);
    let lines = [
        primary(&[[x - 30.0, 0.0], [x + 45.0, 0.0], [x + 100.0, 0.0]]),
        primary(&[[x + 45.0, 0.0], [x + 45.0, -60.0]]),
        road(&[[x + 67.0, -14.0], [x + 67.0, -80.0]]),
    ];
    let p = at([x + 45.0, 0.0]);
    let rounds = |key: CellKey| {
        let (plan, graph) = plan_of(key, &lines);
        let o = key.origin();
        let d = |q: [i32; 2]| (o.x + i64::from(q[0]) - p.x).pow(2) + (o.y + i64::from(q[1]) - p.y).pow(2);
        let j = (0..graph.junctions.len()).min_by_key(|&j| d(graph.junctions[j].pos)).unwrap() as u32;
        let mut out: Vec<([i64; 2], i64)> = (plan.corners.iter())
            .filter(|c| c.owner == Owner::Junction(j))
            .map(|c| {
                ([o.x + i64::from(c.centre[0]), o.y + i64::from(c.centre[1])], libm::round(100.0 * c.radius_m) as i64)
            })
            .collect();
        out.sort_unstable();
        out
    };
    let (west, east) = (rounds(cell()), rounds(CellKey { x: cell().x + 1, y: cell().y }));
    assert_eq!(west, east);
    assert!(west.iter().any(|&(_, r)| r > 0 && r < 900), "{west:?}");
}

#[test]
fn a_face_that_the_precision_cuts_in_two_keeps_its_hole_in_the_part_that_holds_it() {
    // Two squares of one carriageway joined by a neck 3 units wide, which collapses on the grid, and
    // a hole of terrain in the east square.
    let win = window(cell(), 0.0);
    let dumbbell = vec![
        [1000, 1000],
        [2000, 1000],
        [2000, 1498],
        [3000, 1498],
        [3000, 1000],
        [4000, 1000],
        [4000, 2000],
        [3000, 2000],
        [3000, 1501],
        [2000, 1501],
        [2000, 2000],
        [1000, 2000],
    ];
    let hole = vec![[3400, 1400], [3600, 1400], [3600, 1600], [3400, 1600]];
    let (patches, _) = slice::slice(win, &[dumbbell.clone(), hole.clone()], &[]);
    let way = Label { owner: Owner::Way(0), surface: Surface::Carriageway, ..TERRAIN };
    let labels: Vec<Label> = (patches.iter())
        .map(|p| {
            let q = inner_point(p).unwrap();
            let paved = geom::winding(std::slice::from_ref(&dumbbell), q) != 0;
            if paved && geom::winding(std::slice::from_ref(&hole), q) == 0 {
                way
            } else {
                TERRAIN
            }
        })
        .collect();
    let plan = output(cell(), 0, Map::new(snap_round(&patches)), &labels, FaceStats::default());
    assert!(partition(&plan, win).is_empty());
    let parts: Vec<&Face> = plan.faces.iter().filter(|f| f.surface == Surface::Carriageway).collect();
    assert_eq!(parts.len(), 2, "the collapsed neck splits the face");
    for f in parts {
        let east = plan.vertices[f.rings[0][0] as usize][0] >= 3000;
        assert_eq!(f.rings.len(), if east { 2 } else { 1 }, "east part: {east}");
    }
}

#[test]
fn a_wide_way_tapers_into_a_narrow_one_at_their_joint() {
    // A residential road (half-width 2.5 m) runs on as a footway (0.6 m, its finish's cap): the
    // road narrows by `taper_rate` per metre toward the joint, over 1.9 m / 0.15 = 12.7 m, and
    // leaves no step. Its shoulder, which the footway lacks, tapers out with it.
    let p =
        plan(&[road(&[[-80.0, 0.0], [0.0, 0.0]]), line("path", &[("highway", "footway")], &[[0.0, 0.0], [80.0, 0.0]])]);
    let rate = Rules::embedded().plan.taper_rate;
    for x in [0.5, 4.0, 8.0, 12.0] {
        let h = 0.6 + rate * x;
        assert_eq!(surface_at(&p, [-x, h - 0.1]), Surface::Carriageway, "{x} m from the joint, inside");
        assert_ne!(surface_at(&p, [-x, h + 0.1]), Surface::Carriageway, "{x} m from the joint, outside");
        assert_ne!(surface_at(&p, [-x, h + 0.1]), Surface::Shoulder, "{x} m: the footway has no shoulder");
    }
    assert_eq!(surface_at(&p, [-14.0, 2.4]), Surface::Carriageway, "past the taper");
    assert_eq!(surface_at(&p, [-16.0, 2.7]), Surface::Shoulder, "the shoulder past its taper");
    assert!(check::short_features(&p).is_empty(), "{:?}", check::short_features(&p));
}

#[test]
fn a_road_over_a_rail_bed_is_a_crossing_only_where_the_rail_runs_through_it() {
    // A rail along y = 0 (bed half-width 1.9 m); one road crosses it, another runs beside it with
    // its edge 0.9 m over the bed. Only the first makes a crossing; beside the rail the bed wins.
    let rail = line("railway", &[("railway", "rail")], &[[-150.0, 0.0], [150.0, 0.0]]);
    let across = road(&[[-60.0, -80.0], [-60.0, 80.0]]);
    let beside = road(&[[20.0, 3.5], [150.0, 3.5]]);
    let p = plan(&[rail, across, beside]);
    assert_eq!(surface_at(&p, [-60.0, 0.5]), Surface::Crossing);
    assert_eq!(surface_at(&p, [80.0, 1.4]), Surface::RailBed, "the overlap beside the rail");
    assert_eq!(surface_at(&p, [80.0, 2.5]), Surface::Carriageway, "the road past the bed");
}

#[test]
fn a_platform_line_is_a_platform_face_that_beats_the_rail_bed() {
    // A platform 4 m wide along a rail: its centre line 3 m off the track, so it covers the bed's
    // edge from 1 m to 1.9 m.
    let rail = line("railway", &[("railway", "rail")], &[[-150.0, 0.0], [150.0, 0.0]]);
    let platform = line("platform-line", &[("railway", "platform"), ("width", "4")], &[[-40.0, 3.0], [40.0, 3.0]]);
    let p = plan(&[rail, platform]);
    assert_eq!(surface_at(&p, [0.0, 3.0]), Surface::Platform);
    assert_eq!(surface_at(&p, [0.0, 1.4]), Surface::Platform, "over the bed");
    assert_eq!(surface_at(&p, [0.0, 0.5]), Surface::RailBed);
    assert_ne!(surface_at(&p, [0.0, 5.3]), Surface::Platform, "past its half-width");
    assert!(p.faces.iter().any(|f| matches!(f.owner, Owner::Platform(1)) && f.reach_m == 0.0));
}

#[test]
fn a_junction_of_one_class_takes_a_host_arms_material() {
    // A dirt track ends on an asphalt track: one class, so the host's material wins whatever the ids.
    let guest = line("road", &[("highway", "track"), ("surface", "dirt")], &[[0.0, 0.0], [0.0, 80.0]]);
    let host = line("road", &[("highway", "track"), ("surface", "asphalt")], &[[-80.0, 0.0], [0.0, 0.0], [80.0, 0.0]]);
    for lines in [[guest.clone(), host.clone()], [host, guest]] {
        let (p, g) = plan_of(cell(), &lines);
        let j = g.junctions.iter().find(|j| j.arms.len() == 3).unwrap();
        let host_way = j.arms.iter().find(|a| a.host).unwrap().way;
        let face = p.faces.iter().find(|f| matches!(f.owner, Owner::Junction(_))).unwrap();
        assert_eq!(face.material, Some(host_way));
    }
}

#[test]
fn outline_rounds_depend_on_no_order_of_rings_or_ring_starts() {
    // A primary road's edge runs from a square inward corner X of a paved outline, past a vertex at
    // the foot of X's round (so two corners), to a corner Y 0.8 m on that turns 14°: Y lies on X's
    // round (within 0.1 m), X does not lie on Y's. X is sharper, so X's round alone is made, whatever
    // ring and vertex come first.
    let input = input_of(cell(), &[line("road", &[("highway", "primary")], &[[-100.0, 0.0], [100.0, 0.0]])]);
    let rules = Rules::embedded();
    let graph = build(cell(), &input, &rules.sections);
    let lines = level_lines(&graph, &input, rules, cell(), 0);
    let k = 1.0 / mpu();
    let (x0, edge) = ((CELL_UNITS / 2) as i32, f64::from(lines[0].p[0][1]) - lines[0].h[0]);
    // Metres along the edge and off it into land.
    let at = |u: f64, w: f64| [x0 + libm::round(u * k) as i32, libm::round(edge - w * k) as i32];
    let (c, s) = (14f64.to_radians().cos(), 14f64.to_radians().sin());
    let q = [-5.8 - 20.0 * c, 20.0 * s];
    let mut ring: Vec<P> =
        [[0.0, 30.0], [0.0, 0.0], [-5.0, 0.0], [-5.8, 0.0], q, [q[0], -30.0], [30.0, -30.0], [30.0, 30.0]]
            .iter()
            .map(|&[u, w]| at(u, w))
            .collect();
    // The paved side on the left: outer rings turn counter-clockwise.
    if (0..ring.len()).map(|i| geom::cross([0, 0], ring[i], ring[(i + 1) % ring.len()])).sum::<i128>() < 0 {
        ring.reverse();
    }
    let other: Vec<P> =
        [[200.0, -30.0], [260.0, -30.0], [260.0, 30.0], [200.0, 30.0]].iter().map(|&[u, w]| at(u, w)).collect();
    let fills = |paved: &Rings| {
        let fills = outline::rounds(paved, &lines, &[], &|_| 0.0, &|m, _| m * k, 10.0, 5.0);
        let mut out: Vec<(P, i64, Vec<P>, [i64; 2])> = fills
            .iter()
            .map(|f| (f.circle.centre, libm::round(f.circle.radius) as i64, f.ring.clone(), f.inside))
            .collect();
        out.sort();
        out
    };
    let first = fills(&vec![ring.clone(), other.clone()]);
    assert_eq!(first.len(), 1, "{first:?}");
    for start in 0..ring.len() {
        let mut turned = ring.clone();
        turned.rotate_left(start);
        assert_eq!(fills(&vec![other.clone(), turned]), first, "the ring from vertex {start}");
    }
}
