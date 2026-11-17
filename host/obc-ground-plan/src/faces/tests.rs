use super::check::partition;
use super::records::{self, check as records_check};
use super::*;
use crate::frame::{from_lon_lat, metres_per_unit, GlobalPoint};
use crate::graph::{build, WayGraph};
use crate::terrain::{StoredPage, Terrain, PAGE};
use crate::tile::{self, Alignment, Feature, Geometry, Tags};

pub(crate) fn cell() -> CellKey {
    CellKey::of(from_lon_lat(8.40, 46.82))
}

pub(crate) fn mpu() -> f64 {
    metres_per_unit(cell().origin().y + CELL_UNITS / 2)
}

/// A point at metres east and south of the cell's centre.
pub(crate) fn at([x, y]: [f64; 2]) -> GlobalPoint {
    let o = cell().origin();
    let half = CELL_UNITS / 2;
    GlobalPoint { x: o.x + half + libm::round(x / mpu()) as i64, y: o.y + half + libm::round(y / mpu()) as i64 }
}

#[derive(Clone)]
pub(crate) struct Line {
    kind: &'static str,
    tags: Vec<(&'static str, &'static str)>,
    points: Vec<[f64; 2]>,
    pub z: Vec<f64>,
    /// Per vertex, the sides of the segment from it on with a sidewalk: 1 left, 2 right.
    pub walk: Vec<u8>,
}

pub(crate) fn line(kind: &'static str, tags: &[(&'static str, &'static str)], points: &[[f64; 2]]) -> Line {
    let n = points.len();
    Line { kind, tags: tags.to_vec(), points: points.to_vec(), z: vec![500.0; n], walk: vec![0; n] }
}

pub(crate) fn road(points: &[[f64; 2]]) -> Line {
    line("road", &[("highway", "residential")], points)
}

/// A road of a wall class (`leg_wall`).
pub(crate) fn main_road(points: &[[f64; 2]]) -> Line {
    line("road", &[("highway", "tertiary")], points)
}

/// A way of a class without walls.
pub(crate) fn track(points: &[[f64; 2]]) -> Line {
    line("road", &[("highway", "track")], points)
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

pub(crate) fn input_of(key: CellKey, lines: &[Line]) -> CellInput {
    let tile = cell().feature_tiles()[0];
    CellInput {
        key,
        features: lines.iter().zip(0..).map(|(l, index)| feature(l, tile, index)).collect(),
        terrain: Terrain::new(key, Vec::new()),
    }
}

/// The terrain of a cell on every page that it reads: z of metres east and south of the centre of
/// `cell()`.
pub(crate) fn plane(key: CellKey, z: &dyn Fn(f64, f64) -> f64) -> Terrain {
    let centre = at([0.0, 0.0]);
    let pages = Terrain::wanted(key)
        .into_iter()
        .map(|k| {
            let s = crate::frame::FRAME_BITS - 8 - u32::from(k.level);
            let texel = |i: usize, base: u32| ((i64::from(base) << 8) + i as i64) << s | 1 << (s - 1);
            let h: Vec<f64> = (0..PAGE * PAGE)
                .map(|t| {
                    let (x, y) = (texel(t % PAGE, k.x), texel(t / PAGE, k.y));
                    z((x - centre.x) as f64 * mpu(), (y - centre.y) as f64 * mpu())
                })
                .collect();
            let (min, max) = h.iter().fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
            let scale = ((max - min) / 65534.0).max(1e-4);
            let codes = h.iter().map(|v| ((v - min) / scale).round() as u16).collect();
            StoredPage { level: k.level, x: k.x, y: k.y, offset: min as f32, scale: scale as f32, codes }
        })
        .collect();
    Terrain::new(key, pages)
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
fn a_junction_corner_takes_the_lesser_class_radius_and_the_one_walk_radius_where_one_side_walks() {
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
    let east = Rules::embedded().plan.walk.radius_one_walk_m;
    let west = residential.corner_radius_m.min(tertiary.corner_radius_m);
    assert!(east < west);
    assert!((radius(true) - east).abs() < 0.01, "{} m east", radius(true));
    assert!((radius(false) - west).abs() < 0.01, "{} m west", radius(false));
}

#[test]
fn a_one_walk_round_shapes_the_kerb_inside_the_seams_of_its_class_corner_round() {
    // The T of the test above, with and without the sidewalk: the 0.8 m round at the east corner changes the paving
    // there only. The junction's face reaches as far along each arm as with the class's round, and meets the arms in
    // straight seams.
    let host = line("road", &[("highway", "tertiary")], &[[-80.0, 0.0], [0.0, 0.0], [80.0, 0.0]]);
    let guest = road(&[[0.0, 0.0], [0.0, 80.0]]);
    let mut walking = guest.clone();
    walking.walk[0] = 1;
    let reach = |lines: &[Line]| {
        let (p, graph) = plan_of(cell(), lines);
        let bent: Vec<_> =
            (check::artefacts(&p, &graph).into_iter()).filter(|a| a.kind == check::ArtefactKind::BentBorder).collect();
        assert!(bent.is_empty(), "{bent:?}");
        let junction = p.faces.iter().filter(|f| matches!(f.owner, Owner::Junction(_)));
        let pts: Vec<[i32; 2]> =
            junction.flat_map(|f| f.rings.iter().flatten().map(|&v| p.vertices[v as usize])).collect();
        let x = pts.iter().map(|q| q[0]);
        [x.clone().min().unwrap(), x.max().unwrap(), pts.iter().map(|q| q[1]).max().unwrap()]
    };
    let (plain, walk) = (reach(&[host.clone(), guest]), reach(&[host, walking]));
    for k in 0..3 {
        assert!((plain[k] - walk[k]).abs() <= 2 * slice::PRECISION, "{plain:?} {walk:?}");
    }
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
    let plan = output(cell(), 0, &Map::new(snap_round(&patches)), &labels, None, FaceStats::default());
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
fn a_guest_wider_than_its_host_tapers_into_the_host_at_the_junction() {
    // A primary (half-width 4 m) ends at 30° on a residential host (2.5 m) that runs through. Square
    // at full width, the guest's end corner would lie 4 cos 30° = 3.5 m off the host's centre line,
    // past its far edge; tapered to 2.5 m it stays inside the host.
    let (c, s) = (libm::cos(std::f64::consts::FRAC_PI_6), libm::sin(std::f64::consts::FRAC_PI_6));
    let p = plan(&[
        road(&[[-80.0, 0.0], [0.0, 0.0], [80.0, 0.0]]),
        line("road", &[("highway", "primary")], &[[0.0, 0.0], [-80.0 * c, -80.0 * s]]),
    ]);
    for x in -8..=8 {
        let q = [f64::from(x) * 0.5, 2.7];
        assert_ne!(surface_at(&p, q), Surface::Carriageway, "{q:?}: past the host's far edge");
    }
    let along = |d: f64, off: f64| [-d * c + off * s, -d * s - off * c];
    assert_eq!(surface_at(&p, along(30.0, 3.8)), Surface::Carriageway, "the guest past its taper");
    assert_eq!(surface_at(&p, along(30.0, -3.8)), Surface::Carriageway, "the guest past its taper");
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
    let (c, s) = (libm::cos(14f64.to_radians()), libm::sin(14f64.to_radians()));
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

#[test]
fn the_paved_outline_holds_no_water() {
    // A stream that runs on past the window crosses a road under it: no round fills a corner between the road's
    // edge and the water, on either side of either.
    let mut stream = line("stream", &[("waterway", "stream")], &[[0.0, -300.0], [0.0, 300.0]]);
    stream.z = vec![499.0; 2];
    let p = plan(&[road(&[[-300.0, 0.0], [300.0, 0.0]]), stream]);
    assert!(p.corners.is_empty(), "rounds at {:?}", p.corners.iter().map(|c| c.centre).collect::<Vec<_>>());
}

/// A cell's records with its graph and feature names, for the records' checks.
fn records_of(key: CellKey, lines: &[Line]) -> (Vec<u32>, WayGraph, Vec<[u32; 3]>) {
    let input = input_of(key, lines);
    let rules = Rules::embedded();
    let graph = build(key, &input, &rules.sections);
    let words = records::build(&graph, &input, rules, key, 0, true).near.words;
    (words, graph, check::feature_names(&input))
}

#[test]
fn the_records_give_each_point_its_face_label_and_the_far_level_keeps_every_other_label() {
    // A junction of a primary road and a residential one, a rail that a road crosses, and a road
    // beside the rail over its bed's edge.
    let primary = line("road", &[("highway", "primary")], &[[-20.0, -200.0], [0.0, -38.0], [30.0, 200.0]]);
    let rail = line("railway", &[("railway", "rail")], &[[-150.0, 60.0], [150.0, 60.0]]);
    let lines =
        [road(&[[-200.0, -40.0], [0.0, -38.0], [180.0, -60.0]]), primary, rail, road(&[[60.0, 63.5], [150.0, 63.5]])];
    let (key, input, rules) = (cell(), input_of(cell(), &lines), Rules::embedded());
    let graph = build(key, &input, &rules.sections);
    let built = records::build(&graph, &input, rules, key, 0, true);
    let (plan, near) = (built.faces, built.near.words);
    let a =
        records_check::agreement(&near, &plan, &records_check::Overlay::new(&graph, &input, rules, key, true), 6, 8.0);
    assert!(a.points > 10_000, "{} points", a.points);
    assert!(a.area.is_empty(), "{} of {} points: {:?}", a.area.len(), a.points, &a.area[..a.area.len().min(4)]);
    let far = built.far_records.words;
    let [points, differ] = records_check::far_agreement(&near, &far, 6);
    assert!(points > 10_000, "{points} points");
    assert_eq!(differ, 0);
}

#[test]
fn the_far_block_check_finds_a_way_in_reach_that_the_block_leaves_out() {
    let lines = [road(&[[-200.0, -40.0], [0.0, -38.0], [180.0, -60.0]]), road(&[[-20.0, -200.0], [30.0, 200.0]])];
    let (key, input, rules) = (cell(), input_of(cell(), &lines), Rules::embedded());
    let graph = build(key, &input, &rules.sections);
    let built = records::build(&graph, &input, rules, key, 0, true);
    let near = built.near.words;
    let (faults, counts) = records_check::far_matches_near(&near, &built.far_block.unwrap(), key);
    assert!(faults.is_empty() && counts["lines"] >= 2, "{faults:?} {counts:?}");
    // A block without the first way, and an empty block.
    let mut all = record_lines(&graph, &input, rules, key);
    all.remove(0);
    for lines in [&all[..], &[]] {
        let block = records::far::encode(key, &graph, &input, rules, lines).unwrap();
        let faults = records_check::far_matches_near(&near, &block, key).0;
        assert!(faults.iter().any(|f| f.ends_with("segments in reach, no far line")), "{faults:?}");
    }
}

#[test]
fn two_cells_give_one_label_on_their_line_where_an_edge_and_vertices_lie_on_it() {
    // A road runs north with its carriageway's east edge on the east cell line (a vertex within 2
    // units moves onto it); another one crosses the line, so its rings have vertices on it, and meets
    // a third one 4 m past the line, so both cells read that junction's region.
    let x = CELL_UNITS as f64 / 2.0 * mpu();
    let lines = [
        road(&[[x - 2.5, -250.0], [x - 2.5, -50.0]]),
        road(&[[x - 100.0, 150.0], [x + 4.0, 151.0], [x + 100.0, 152.0]]),
        road(&[[x + 4.0, 151.0], [x + 4.0, 300.0]]),
    ];
    let (a, b) = (records_of(cell(), &lines), records_of(CellKey { x: cell().x + 1, y: cell().y }, &lines));
    let c = CELL_UNITS as i32;
    let (vertices, edges) = records_check::on_line(&a.0, true, c);
    assert!(edges > 0 && vertices.len() > 4, "{edges} edges and {} vertices on the line", vertices.len());
    let (points, blend, differ) = records_check::border((&a.0, &a.1, &a.2), (&b.0, &b.1, &b.2), true, 37);
    assert!(points > 7_000 && blend > 100, "{points} points, {blend} with a blend edge near");
    assert!(differ.is_empty(), "{} differ: {:?}", differ.len(), &differ[..differ.len().min(4)]);
}

#[test]
fn a_junction_label_names_its_record_with_the_arms_ranges_flags_and_directions() {
    // A residential road runs east, and a dirt path leaves it to the north and stops there.
    let lines = [
        road(&[[-80.0, 0.0], [0.0, 0.0], [80.0, 0.0]]),
        line("path", &[("highway", "path"), ("surface", "dirt")], &[[0.0, 0.0], [0.0, -30.0], [0.0, -80.0]]),
    ];
    let (words, graph, _) = records_of(cell(), &lines);
    let ids: Vec<u32> = (0..words[2]).filter(|&id| records::junction(&words, id).is_some()).collect();
    assert!(!ids.is_empty());
    let jr = records::junction(&words, ids[0]).unwrap();
    for &id in &ids {
        assert_eq!(records::junction(&words, id), Some(jr), "every label of the junction names one record");
        assert_eq!(records::owner(&words, id), jr[2] >> 16);
    }
    let j = &graph.junctions[(jr[2] >> 16) as usize];
    assert_eq!([jr[0] as i32, jr[1] as i32], j.pos);
    assert_eq!(jr[2] & 0xff, 3);
    let sample = |line: u32, i: u32| {
        let first = words[words[6] as usize + records::LINE * line as usize + 2];
        let s = &words[words[7] as usize + records::SAMPLE * (first + i) as usize..];
        [f64::from(s[0] as i32 - j.pos[0]), f64::from(s[1] as i32 - j.pos[1])]
    };
    // Per arm: its flags and its direction at the node, east, west or north (frame y runs south).
    let mut arms: Vec<(u32, [i32; 2])> = Vec::new();
    for a in jr[records::JR..].chunks(records::JR_ARM) {
        let (line, node, last) = (a[0] & 0xffff, a[1] & 0xffff, a[1] >> 16);
        let d = [f64::from((a[2] & 0xffff) as u16 as i16), f64::from((a[2] >> 16) as u16 as i16)].map(|v| v / 32767.0);
        assert_eq!(sample(line, node), [0.0, 0.0], "the node sample lies on the node");
        // The arm runs from the node in its direction to the first sample past its seam, some metres on.
        let p = sample(line, last);
        let ahead = (p[0] * d[0] + p[1] * d[1]) * mpu();
        assert!(ahead > 3.0 && ahead < 40.0, "the last sample lies {ahead} m ahead");
        arms.push((a[0] >> 16, [libm::round(d[0]) as i32, libm::round(d[1]) as i32]));
    }
    arms.sort_unstable();
    let (host, stop) = (records::ARM_HOST >> 16, records::ARM_STOP >> 16);
    assert_eq!(arms, vec![(host, [-1, 0]), (host, [1, 0]), (stop, [0, -1])]);
}

/// Whether q (metres east and south of the junction's node) keeps the route in the stop mouth of junction record `jr`,
/// as `terrain/mouth.wgsl` decides it (a port of it); none outside a sector with a stop arm.
fn keeps_route(words: &[u32], jr: &[u32], q: [f64; 2]) -> Option<bool> {
    let m = mpu();
    let node = [f64::from(jr[0] as i32), f64::from(jr[1] as i32)];
    let v = [q[0] / m, q[1] / m];
    let p = [node[0] + v[0], node[1] + v[1]];
    let sample = |line: u32, i: u32| {
        let first = words[words[6] as usize + records::LINE * line as usize + 2];
        let s = &words[words[7] as usize + records::SAMPLE * (first + i) as usize..];
        ([f64::from(s[0] as i32), f64::from(s[1] as i32)], f64::from(f32::from_bits(s[2])))
    };
    let arms: Vec<&[u32]> = jr[records::JR..].chunks(records::JR_ARM).collect();
    let dir = |a: &[u32]| [a[2] & 0xffff, a[2] >> 16].map(|x| f64::from(x as u16 as i16) / 32767.0);
    let stop = |a: &[u32]| a[0] & records::ARM_STOP != 0;
    let wrap = |x: f64| x.rem_euclid(std::f64::consts::TAU);
    let angle = |u: [f64; 2]| libm::atan2(u[1], u[0]);
    let aq = angle(v);
    let route: Vec<&&[u32]> = arms.iter().filter(|a| !stop(a)).collect();
    let a = route.iter().min_by(|x, y| wrap(aq - angle(dir(x))).total_cmp(&wrap(aq - angle(dir(y)))))?;
    let b = route.iter().min_by(|x, y| wrap(angle(dir(x)) - aq).total_cmp(&wrap(angle(dir(y)) - aq)))?;
    let span = wrap(aq - angle(dir(a))) + wrap(angle(dir(b)) - aq);
    if !arms.iter().any(|s| stop(s) && wrap(angle(dir(s)) - angle(dir(a))) < span) {
        return None;
    }
    let (ua, ub) = (dir(a), dir(b));
    let (ha, hb) = (sample(a[0] & 0xffff, a[1] & 0xffff).1, sample(b[0] & 0xffff, b[1] & 0xffff).1);
    let (na, nb) = ([-ua[1], ua[0]], [ub[1], -ub[0]]);
    // Where one facing edge corner lies past the other's edge line, the other arm widens to it at the node.
    let two = if a != b { 1.0 } else { 0.0 };
    let dot = |x: [f64; 2], y: [f64; 2]| x[0] * y[0] + x[1] * y[1];
    let (wa, wb) = (two * (dot(nb, na) * hb - ha).max(0.0), two * (dot(na, nb) * ha - hb).max(0.0));
    // The strokes, flat at the node, and the mitre.
    let mut d = f64::INFINITY;
    for r in &route {
        let (n, last) = (r[1] & 0xffff, r[1] >> 16);
        let (side, widen) = if r == a {
            (two, wa)
        } else if r == b {
            (-two, wb)
        } else {
            (0.0, 0.0)
        };
        let step: i64 = if last > n { 1 } else { -1 };
        let at = |k: i64| sample(r[0] & 0xffff, (i64::from(n) + k * step) as u32);
        let segments = (i64::from(last) - i64::from(n)).abs();
        let total: f64 =
            (0..segments).map(|k| libm::hypot(at(k + 1).0[0] - at(k).0[0], at(k + 1).0[1] - at(k).0[1])).sum();
        let mut along = 0.0;
        for k in 0..segments {
            let ((s0, h0), (s1, h1)) = (at(k), at(k + 1));
            let u = [s1[0] - s0[0], s1[1] - s0[1]];
            let len = libm::hypot(u[0], u[1]);
            let t = ((p[0] - s0[0]) * u[0] + (p[1] - s0[1]) * u[1]) / (len * len);
            let c = t.clamp(0.0, 1.0);
            let cross = u[0] * (p[1] - s0[1]) - u[1] * (p[0] - s0[0]);
            let wider = if side * cross > 0.0 { widen * (1.0 - (along + c * len) / total) } else { 0.0 };
            d = d.min(if k == 0 && t < 0.0 {
                (cross.abs() / len - h0 - wider).max(-t * len)
            } else {
                libm::hypot(p[0] - s0[0] - c * u[0], p[1] - s0[1] - c * u[1]) - (h0 + c * (h1 - h0)) - wider
            });
            along += len;
        }
    }
    if a != b {
        let edges = (dot(v, na) - ha - wa).max(dot(v, nb) - hb - wb);
        let behind = dot(v, ua).max(dot(v, ub));
        d = d.min(edges.max(libm::hypot(v[0], v[1]) - 4.0 * ha.max(hb)).max(behind));
    }
    Some(d * m <= 0.02)
}

#[test]
fn a_stop_mouth_keeps_the_routes_own_edge_and_its_mitre_at_a_bend() {
    let dirt = |points: &[[f64; 2]]| line("path", &[("highway", "path"), ("surface", "dirt")], points);
    // A road turns north at the node and a dirt path leaves the outer side of the bend to the east.
    let lines = [road(&[[-80.0, 0.0], [0.0, 0.0], [0.0, -80.0]]), dirt(&[[0.0, 0.0], [40.0, 10.0], [80.0, 10.0]])];
    let (words, _, _) = records_of(cell(), &lines);
    let id = (0..words[2]).find(|&id| records::junction(&words, id).is_some()).unwrap();
    let jr = records::junction(&words, id).unwrap();
    // The route arms that bound the outer sector, as the record has them: a turns counter-clockwise to b through it.
    let arm = |k: usize| &jr[records::JR + records::JR_ARM * k..][..records::JR_ARM];
    let dir = |a: &[u32]| [a[2] & 0xffff, a[2] >> 16].map(|x| f64::from(x as u16 as i16) / 32767.0);
    let route: Vec<&[u32]> = (0..(jr[2] & 0xff) as usize).map(arm).filter(|a| a[0] & records::ARM_STOP == 0).collect();
    let south = |a: &[u32]| dir(a)[1] > 0.0;
    let (a, b) = if south(route[0]) { (route[1], route[0]) } else { (route[0], route[1]) };
    let half = |a: &[u32]| {
        let line = &words[words[6] as usize + records::LINE * (a[0] & 0xffff) as usize..];
        f64::from(f32::from_bits(words[words[7] as usize + records::SAMPLE * (line[2] + (a[1] & 0xffff)) as usize + 2]))
            * mpu()
    };
    let (ua, ub, ha, hb) = (dir(a), dir(b), half(a), half(b));
    let (na, nb) = ([-ua[1], ua[0]], [ub[1], -ub[0]]);
    // Their facing edges meet at the mitre's corner m behind the node.
    let det = na[0] * nb[1] - na[1] * nb[0];
    let m = [(ha * nb[1] - hb * na[1]) / det, (na[0] * hb - nb[0] * ha) / det];
    let at = |p: [f64; 2], n: [f64; 2], s: f64| [p[0] + s * n[0], p[1] + s * n[1]];
    assert_eq!(keeps_route(&words, jr, at(m, m, -0.1)), Some(true), "inside the mitre's corner");
    assert_eq!(keeps_route(&words, jr, at(at(m, ua, 0.3), na, 0.3)), Some(false), "past a's edge, straight on");
    assert_eq!(keeps_route(&words, jr, at(at(m, ub, 0.3), nb, 0.3)), Some(false), "past b's edge, straight on");
    // Beside each arm the edge is the arm's own: no tab beside the mouth.
    for (u, n, h) in [(ua, na, ha), (ub, nb, hb)] {
        assert_eq!(keeps_route(&words, jr, at(at([0.0; 2], u, 1.0), n, h - 0.3)), Some(true));
        assert_eq!(keeps_route(&words, jr, at(at([0.0; 2], u, 1.0), n, h + 0.3)), Some(false));
    }
}

#[test]
fn a_stop_mouth_between_two_widths_widens_the_narrow_arm_without_a_step() {
    let dirt = |points: &[[f64; 2]]| line("path", &[("highway", "path"), ("surface", "dirt")], points);
    // A wide road runs on as a narrow one through the node, and a dirt path leaves it to the north.
    let lines = [
        line("road", &[("highway", "primary")], &[[-80.0, 0.0], [0.0, 0.0]]),
        road(&[[0.0, 0.0], [80.0, 0.0]]),
        dirt(&[[0.0, 0.0], [0.0, -40.0], [0.0, -80.0]]),
    ];
    let (words, _, _) = records_of(cell(), &lines);
    let id = (0..words[2]).find(|&id| records::junction(&words, id).is_some()).unwrap();
    let jr = records::junction(&words, id).unwrap();
    let half = |west: bool| {
        let a = jr[records::JR..].chunks(records::JR_ARM).find(|a| {
            let x = f64::from((a[2] & 0xffff) as u16 as i16);
            a[0] & records::ARM_STOP == 0 && (x < 0.0) == west
        });
        let a = a.unwrap();
        let line = &words[words[6] as usize + records::LINE * (a[0] & 0xffff) as usize..];
        f64::from(f32::from_bits(words[words[7] as usize + records::SAMPLE * (line[2] + (a[1] & 0xffff)) as usize + 2]))
            * mpu()
    };
    let (wide, narrow) = (half(true), half(false));
    assert!(wide > narrow + 0.5, "{wide} m and {narrow} m");
    // Just east of the node the narrow arm is as wide as the wide one on the path's side: no step.
    assert_eq!(keeps_route(&words, jr, [0.2, -(wide - 0.1)]), Some(true));
    assert_eq!(keeps_route(&words, jr, [0.2, -(wide + 0.1)]), Some(false));
    // The other side has no stop arm: the junction keeps its own look there.
    assert_eq!(keeps_route(&words, jr, [0.2, narrow + 0.1]), None);
}

#[test]
fn two_cells_give_one_side_to_their_line_where_an_edge_passes_a_tile_corner_on_it() {
    // A ring's edge leaves the cell line x = C at y = t + 27 or t + 15 and crosses the tile line y = t
    // 0.53 or 0.30 units off the cell line, beside the tile corner (C, t); or it crosses the cell line
    // at that corner. Each also mirrored and reversed. Per window, the segments on the cell line after
    // the snap rounding, each with whether its patch lies in the ring, which goes through `prepare`
    // twice (as `split_legs` does) and then gets its tile-line points, as `candidates` gives them.
    let (c, t) = (CELL_UNITS as i32, 7 * slice::TILE as i32);
    let sides = |window: [i32; 4], ring: &[P]| {
        let rings: Rings =
            prepare(&prepare(&vec![ring.to_vec()])).iter().map(|r| geom::with_grid_lines(r, slice::TILE)).collect();
        let (patches, _) = slice(window, &rings, &[]);
        let snapped = snap_round(&patches);
        let at = |i: u32| snapped.verts[i as usize];
        let mut on: Vec<([P; 2], bool)> = (patches.iter().zip(&snapped.patches))
            .flat_map(|(patch, segs)| {
                let inside = inner_point(patch).is_some_and(|q| geom::winding(&rings, q) != 0);
                segs.iter().map(move |&[a, b]| ([at(a), at(b)], inside))
            })
            .filter(|([a, b], _)| a[0] == c && b[0] == c)
            .map(|([a, b], inside)| (if a < b { [a, b] } else { [b, a] }, inside))
            .collect();
        on.sort_unstable();
        on
    };
    let wedge = |on: [P; 2], low: P| {
        vec![[c - 40, t + 300], on[0], on[1], low, [c + 300, t - 379], [c + 300, t + 300], [c, t + 300]]
    };
    let mut rings = Vec::new();
    for ring in [
        wedge([[c - 20, t + 200], [c, t + 27]], [c + 8, t - 379]),
        wedge([[c - 20, t + 200], [c, t + 15]], [c + 8, t - 379]),
        wedge([[c - 8, t + 8], [c, t]], [c + 8, t - 8]),
    ] {
        rings.push(ring.iter().map(|p| [2 * c - p[0], p[1]]).collect::<Vec<P>>());
        rings.push(ring.iter().rev().copied().collect());
        rings.push(ring);
    }
    for ring in rings {
        let west = sides([c - 400, t - 400, c, t + 400], &ring);
        let east = sides([c, t - 400, c + 400, t + 400], &ring);
        assert!(west.len() >= 3, "{west:?}");
        assert_eq!(west, east, "ring {ring:?}");
    }
}

/// A residential road through a bore of 120 m under ground 3.1 m over the road: storey 0 and the floor are one map,
/// the bore is the prism under storey 0's triangles, and the cover holds the least room over the floor (its lift lies
/// within `lift_max_m`).
#[cfg(feature = "bores")]
#[test]
fn a_bore_is_the_prism_under_storey_0_and_its_cover_holds_the_least_room() {
    use crate::mesh::shell::{closure, mesh_storeys_with, FLOOR, ROOF};
    use crate::mesh::{mesh_cell, Reads, NO_LABEL};
    let key = cell();
    let levels = ("levels", "O 40 P B 160 P O");
    let bore = line("tunnel", &[("highway", "residential"), ("tunnel", "yes"), levels], &[[-100.0, 0.0], [100.0, 0.0]]);
    let mut input = input_of(key, &[bore]);
    input.terrain = plane(key, &|_, _| 503.1);
    let rules = Rules::embedded();
    let b = crate::cell::build_input(&input, rules);
    let m0 = mesh_cell(&b.faces, &b.records, &Reads::of(&b.faces, &[]), &mut || 0.0).unwrap();
    let m = mesh_storeys_with(key, &b.storeys, &b.records, &rules.structures, Some(&m0)).unwrap();
    assert!(b.storeys.plans.iter().any(|p| p.level < 0), "a storey under the ground");
    assert!(!b.storeys.portals.is_empty(), "pieces on the portal lines");
    assert!(closure(&m).is_empty(), "{:?}", closure(&m));
    // No deck: every triangle and slot is the bore's own.
    assert_eq!(m.parts[..FLOOR].iter().sum::<u32>(), 0, "a bore's triangles in the decks' parts");
    assert_eq!(m.slot_kinds[..2].iter().sum::<u32>(), 0, "a bore's slots among the decks' kinds");
    let t = |p: P| {
        input.terrain.height(GlobalPoint { x: key.origin().x + i64::from(p[0]), y: key.origin().y + i64::from(p[1]) })
    };
    let h0 = |k: usize| {
        let (p, l, terrain) = m0.corner(k);
        let t = t(p).unwrap();
        if terrain || l == NO_LABEL {
            t
        } else {
            heights::height_at(&b.records, [f64::from(p[0]), f64::from(p[1])], Some(l), t).h
        }
    };
    // The floor's triangles by their points, and the lids: storey 0's corners of the triangles on the same points.
    let points = |t: &[u32; 3], c: &dyn Fn(u32) -> P| {
        let mut p = t.map(c);
        p.sort_unstable();
        p
    };
    let floors: std::collections::BTreeSet<[P; 3]> =
        m.triangles[m.part(FLOOR)].iter().map(|t| points(t, &|k| m.corner(k as usize).0)).collect();
    let mut ground: BTreeMap<P, Vec<f64>> = BTreeMap::new();
    for t in &m0.triangles {
        if floors.contains(&points(t, &|k| m0.corner(k as usize).0)) {
            for &k in t {
                ground.entry(m0.corner(k as usize).0).or_default().push(h0(k as usize));
            }
        }
    }
    let sec = rules.sections.way("road", &[("highway".to_string(), "residential".to_string())].into(), false);
    // The floor's and the roof's heights by point.
    let by_point = |part: std::ops::Range<usize>| {
        let mut out: BTreeMap<P, f64> = BTreeMap::new();
        for &k in m.triangles[part].iter().flatten() {
            let h = m.corner_height(k as usize, &b.records, &input.terrain);
            let e = out.entry(m.corner(k as usize).0).or_insert(h);
            *e = e.min(h);
        }
        out
    };
    let (floor, roof) = (by_point(m.part(FLOOR)), by_point(m.part(ROOF)));
    let mut corners = 0;
    for (p, &hf) in &floor {
        // One map: every floor triangle is a triangle of storey 0's mesh.
        let lid = ground.get(p).expect("a floor point is a point of storey 0's triangles over the floor");
        // The prism: the roof lies its plate under storey 0 at the point, and the cover holds the least room.
        let (least, hr) = (lid.iter().copied().fold(f64::INFINITY, f64::min), roof[p]);
        assert!(hr <= least - sec.roof_least_m + 1e-3, "{p:?}: roof {hr} under storey 0 {least}");
        assert!(hr - hf >= sec.bore_least_m - 1e-3, "{p:?}: room {} under the cover", hr - hf);
        let lift = lid.iter().copied().fold(f64::NEG_INFINITY, f64::max) - t(*p).unwrap();
        assert!(lift <= rules.structures.lift_max_m + 1e-3, "{p:?}: the cover {lift} m over the terrain");
        corners += 1;
    }
    assert!(corners > 100, "{corners} floor corners");
    // The way's cut ends square on the portal line (x −60): in front of it the ground beside the way lies in the
    // cut, behind it the ground beside the cover is the terrain.
    let ground_at = |x: f64, y: f64| {
        let o = key.origin();
        let g = at([x, y]);
        let q = [(g.x - o.x) as f64, (g.y - o.y) as f64];
        heights::height_at(&b.records, q, records::find(&b.records, q), 503.1).h
    };
    for y in [-6.0, 6.0] {
        assert!(ground_at(-62.0, y) < 503.1 - 0.5, "in front of the portal line at {y}: {}", ground_at(-62.0, y));
        assert!(ground_at(-58.0, y) >= 503.1 - 1e-3, "behind the portal line at {y}: {}", ground_at(-58.0, y));
    }
}

/// The slant of the ways into the hill (`entrance`): storey 0's corners on a portal line lie off it by a few frame
/// units.
#[cfg(feature = "bores")]
const SLANT: [f64; 2] = [0.94, 0.341];

/// A hill over metres along the slant: level at 500 m, rising from 80 m before the cell's centre to 506 m.
#[cfg(feature = "bores")]
fn hill(along: f64) -> f64 {
    500.0 + ((along + 80.0) * 0.3).clamp(0.0, 6.0)
}

/// A residential tunnel way at 500 m along the slant, `off` metres to its left, from 100 m before the cell's centre to
/// 100 m past it, with its `levels`.
#[cfg(feature = "bores")]
fn tube(off: f64, levels: &'static str) -> Line {
    let (u, n) = (SLANT, [-SLANT[1], SLANT[0]]);
    let ends = [-100.0, 100.0].map(|a| [a * u[0] + off * n[0], a * u[1] + off * n[1]]);
    line("tunnel", &[("highway", "residential"), ("tunnel", "yes"), ("levels", levels)], &ends)
}

/// Storey 0's seams at the portals of a cell.
#[cfg(feature = "bores")]
struct Seams {
    /// The largest step off the portal lines (within 10 m of one) between the cover and a ground face beside it.
    step: f64,
    /// The count of those slots.
    sides: usize,
    /// The largest difference between a ground corner on a portal line (within 2 cm) and its own face 3 cm to its side:
    /// behind the line for the cover, in front of it for every other face.
    side: f64,
    /// The least height over the terrain of the ground within 12 m behind the first way's portal line and 14 m beside
    /// its centre line.
    under: f64,
}

/// A cell built from `ways` over the ground of `profile` (metres along the slant), storey 0's mesh and its seams at the
/// portals.
#[cfg(feature = "bores")]
fn entrance(
    ways: &[Line],
    profile: &dyn Fn(f64) -> f64,
) -> (crate::cell::CellBuild, Terrain, crate::mesh::CellMesh, Seams) {
    use crate::mesh::{mesh_cell, Reads, NO_LABEL};
    let key = cell();
    let mut input = input_of(key, ways);
    let (centre, u) = (at([0.0, 0.0]), SLANT);
    input.terrain = plane(key, &|x, y| profile(x * u[0] + y * u[1]));
    let b = crate::cell::build_input(&input, Rules::embedded());
    let m0 = mesh_cell(&b.faces, &b.records, &Reads::of(&b.faces, &[]), &mut || 0.0).unwrap();
    let o = key.origin();
    // Metres along the slant and to its left of a point from a point.
    let rel = |p: [f64; 2], from: [f64; 2]| {
        let v = [(p[0] - from[0]) * mpu(), (p[1] - from[1]) * mpu()];
        [v[0] * u[0] + v[1] * u[1], v[1] * u[0] - v[0] * u[1]]
    };
    let f = |p: P| [f64::from(p[0]), f64::from(p[1])];
    // The portal points (the joints of a ground way and a bore), each with the sign of the slant into its bore: toward
    // the cell's centre, which every bore holds.
    let mid = [(centre.x - o.x) as f64, (centre.y - o.y) as f64];
    let level = |a: &crate::graph::Arm| b.graph.ways[a.way as usize].level;
    let portals: Vec<([f64; 2], f64)> = (b.graph.joints.iter())
        .filter(|j| {
            j.arms.iter().any(|a| level(a) == Level::Tunnel) && j.arms.iter().any(|a| level(a) == Level::Ground)
        })
        .map(|j| (f(j.pos), rel(mid, f(j.pos))[0].signum()))
        .collect();
    // Metres past the nearest portal line within its way's reach (negative in front of it), and the slant into its bore.
    let past = |p: P| {
        let near = portals.iter().map(|&(x, into)| (rel(f(p), x), into)).filter(|(r, _)| r[1].abs() < 13.0);
        let nearest = near.min_by(|a, c| a.0[0].abs().total_cmp(&c.0[0].abs()));
        nearest.map_or((f64::INFINITY, 1.0), |(r, into)| (r[0] * into, into))
    };
    let t = |p: P| input.terrain.height(GlobalPoint { x: o.x + i64::from(p[0]), y: o.y + i64::from(p[1]) }).unwrap();
    let surface = |l: u32| (l != NO_LABEL).then(|| records::label(&b.records, l)[0] & 0xff);
    let ground = |l: u32| {
        surface(l).is_none_or(|s| [Surface::Terrain, Surface::Slope, Surface::Cover].map(|x| x as u32).contains(&s))
    };
    let cover = |l: u32| surface(l) == Some(Surface::Cover as u32);
    let at_label = |q: [f64; 2], l: u32, t: f64| heights::height_at(&b.records, q, Some(l), t).h;
    let h0 = |k: usize| {
        let (p, l, terrain) = m0.corner(k);
        if terrain || l == NO_LABEL {
            t(p)
        } else {
            at_label(f(p), l, t(p))
        }
    };
    let mut seams = Seams { step: 0.0, sides: 0, side: 0.0, under: f64::INFINITY };
    // A slot is [a left, b left, b right, a right].
    for s in &m0.steps {
        let [a, b2, c, d] = s.map(|k| k as usize);
        let (la, ld) = (m0.corner(a).1, m0.corner(d).1);
        let (ma, mb) = (past(m0.corner(a).0).0, past(m0.corner(b2).0).0);
        let on_line = ma.abs() < 0.05 && mb.abs() < 0.05;
        if cover(la) == cover(ld) || !ground(la) || !ground(ld) || on_line || ma.abs().min(mb.abs()) > 10.0 {
            continue;
        }
        seams.step = seams.step.max((h0(a) - h0(d)).abs().max((h0(b2) - h0(c)).abs()));
        seams.sides += 1;
    }
    // The first way's first portal: on its centre line, the nearer to the start.
    let ours = portals.iter().map(|x| x.0).filter(|&x| rel(x, mid)[1].abs() < 1.0);
    let first = ours.min_by(|a, c| rel(*a, mid)[0].total_cmp(&rel(*c, mid)[0]));
    for k in 0..m0.corners.len() {
        let (p, l, terrain) = m0.corner(k);
        if terrain || l == NO_LABEL || !ground(l) {
            continue;
        }
        let (m, into) = past(p);
        if m.abs() < 0.02 {
            // Its face's side: behind the line for the cover, in front for every other face.
            let side = if cover(l) { 0.03 } else { -0.03 } * into / mpu();
            let q = [f(p)[0] + side * u[0], f(p)[1] + side * u[1]];
            seams.side = seams.side.max((h0(k) - at_label(q, l, t(p))).abs());
        }
        let r = first.map_or([f64::NAN; 2], |x| rel(f(p), x));
        if (0.02..12.0).contains(&r[0]) && r[1].abs() < 14.0 {
            seams.under = seams.under.min(h0(k) - t(p));
        }
    }
    (b, input.terrain, m0, seams)
}

/// Twin bores side by side into a hill at a slant, their portals 10 m apart along it: the cover's end parts each way's
/// cut in front of its portal line from the ground behind it, and the ground behind it, also the other way's slope
/// band beside the first bore, meets the cover with no step off the line; a corner on a portal line reads its face's
/// side; the mouth keeps the least room.
#[cfg(feature = "bores")]
#[test]
fn the_ground_beside_the_cover_meets_it_with_no_step() {
    use crate::mesh::shell::{mesh_storeys_with, FLOOR, ROOF};
    let (b, terrain, m0, seams) = entrance(&[tube(0.0, "O 40 P B 160 P O"), tube(9.0, "O 50 P B 150 P O")], &hill);
    assert!(seams.sides > 0, "the cover meets the ground off the portal lines");
    assert!(seams.step < 0.05, "a step of {} m between the cover and the ground beside it", seams.step);
    assert!(seams.side < 0.1, "a corner on a portal line {} m off its face's side", seams.side);
    // The mouth: the floor's points within 3 m behind the first line keep the least room under the roof.
    let rules = Rules::embedded();
    let m = mesh_storeys_with(b.key, &b.storeys, &b.records, &rules.structures, Some(&m0)).unwrap();
    let sec = rules.sections.way("road", &[("highway".to_string(), "residential".to_string())].into(), false);
    let by_point = |part: std::ops::Range<usize>| {
        let mut out: BTreeMap<P, f64> = BTreeMap::new();
        for &k in m.triangles[part].iter().flatten() {
            let h = m.corner_height(k as usize, &b.records, &terrain);
            let e = out.entry(m.corner(k as usize).0).or_insert(h);
            *e = e.min(h);
        }
        out
    };
    let behind = |p: P| {
        let (g, o) = (at([0.0, 0.0]), b.key.origin());
        let v = [(o.x + i64::from(p[0]) - g.x) as f64 * mpu(), (o.y + i64::from(p[1]) - g.y) as f64 * mpu()];
        v[0] * SLANT[0] + v[1] * SLANT[1] + 60.0
    };
    let (floor, roof) = (by_point(m.part(FLOOR)), by_point(m.part(ROOF)));
    let mouth: Vec<(&P, &f64)> = floor.iter().filter(|(p, _)| (0.0..3.0).contains(&behind(**p))).collect();
    assert!(!mouth.is_empty(), "floor points in the mouth");
    for (p, &hf) in mouth {
        assert!(roof[p] - hf >= sec.bore_least_m - 1e-3, "{p:?}: room {} in the mouth", roof[p] - hf);
    }
}

/// Storey 0's end faces beside the cover on the portal lines of a cell (`EdgeKind::Headwall`), by its mesh and the
/// Rust heights.
#[cfg(feature = "bores")]
struct EndWalls {
    /// The end faces on the portal lines (`CellMesh::headwalls`); those of them on no portal line or not beside the
    /// cover; the steps between the cover and another face with both ends on one portal line that are no end face; end
    /// face corners in no triangle (a hole at the top or the foot).
    faces: usize,
    off_line: usize,
    steps_on_line: usize,
    loose: usize,
    /// The highest end face, and the least rise of the cover over the other side at an end face's corner.
    high: f64,
    least: f64,
    /// Per portal line with an opening: its point, and where the end faces and the opening's pieces join in one run
    /// along the line that starts and ends with an end face, the steps at its two ends.
    runs: Vec<([f64; 2], Option<[f64; 2]>)>,
}

#[cfg(feature = "bores")]
fn end_walls(b: &crate::cell::CellBuild, terrain: &Terrain, m0: &crate::mesh::CellMesh) -> EndWalls {
    use crate::mesh::NO_LABEL;
    let o = b.key.origin();
    let f = |p: P| [f64::from(p[0]), f64::from(p[1])];
    let t = |p: P| terrain.height(GlobalPoint { x: o.x + i64::from(p[0]), y: o.y + i64::from(p[1]) }).unwrap();
    let h = |k: u32| {
        let (p, l, vertex) = m0.corner(k as usize);
        if vertex || l == NO_LABEL {
            t(p)
        } else {
            heights::height_at(&b.records, f(p), Some(l), t(p)).h
        }
    };
    let cover = |k: u32| {
        let l = m0.corner(k as usize).1;
        l != NO_LABEL && records::label(&b.records, l)[0] & 0xff == Surface::Cover as u32
    };
    let lines = heights::check::portal_lines(&b.records);
    // A slot's ends are its edge's points, rounded to whole units: one unit more than the faces' test.
    let on = |p: P, &(at, n): &([f64; 2], u32)| heights::behind_end(f(p), at, n).abs() <= heights::ON_LINE_UNITS + 1.0;
    let corner = |k: u32| m0.corner(k as usize).0;
    let line_of = |s: &[u32; 4]| lines.iter().position(|e| on(corner(s[0]), e) && on(corner(s[1]), e));
    let in_triangle: std::collections::BTreeSet<u32> = m0.triangles.iter().flatten().copied().collect();
    let (first, walls) = (m0.steps.len() - m0.end_faces as usize, m0.steps.len() - m0.headwalls as usize);
    let (mut faces, mut off_line, mut steps_on_line, mut loose, mut high, mut least) =
        (0, 0, 0, 0, 0.0f64, f64::INFINITY);
    let mut pieces: BTreeMap<usize, Vec<(P, P, bool)>> = BTreeMap::new();
    for (k, s) in m0.steps.iter().enumerate() {
        let left = cover(s[0]);
        let (beside, line) = (left != cover(s[3]), line_of(s));
        if k < first {
            steps_on_line += usize::from(beside && line.is_some());
            continue;
        }
        if k < walls {
            continue;
        }
        faces += 1;
        if !beside {
            off_line += 1;
            continue;
        }
        loose += s.iter().filter(|c| !in_triangle.contains(c)).count();
        let rise = [h(s[0]) - h(s[3]), h(s[1]) - h(s[2])].map(|r| if left { r } else { -r });
        high = high.max(rise[0].abs()).max(rise[1].abs());
        least = least.min(rise[0]).min(rise[1]);
        match line {
            Some(l) => pieces.entry(l).or_default().push((corner(s[0]), corner(s[1]), false)),
            None => off_line += 1,
        }
    }
    let mut runs = Vec::new();
    for (l, &(at, n)) in lines.iter().enumerate() {
        let mut on_line = pieces.remove(&l).unwrap_or_default();
        on_line.extend(
            b.storeys.portals.iter().filter(|x| on(x.a, &lines[l]) && on(x.b, &lines[l])).map(|x| (x.a, x.b, true)),
        );
        // Metres along the line.
        let u = [f64::from(n as u16 as i16), f64::from((n >> 16) as u16 as i16)];
        let along = |p: P| (u[0] * (f(p)[1] - at[1]) - u[1] * (f(p)[0] - at[0])) / libm::hypot(u[0], u[1]) * mpu();
        let mut spans: Vec<(f64, f64, bool)> =
            on_line.iter().map(|&(a, c, open)| (along(a).min(along(c)), along(a).max(along(c)), open)).collect();
        spans.sort_by(|x, y| x.0.total_cmp(&y.0));
        let Some(open) = spans.iter().position(|x| x.2) else { continue };
        let (mut lo, mut hi) = (open, open);
        while lo > 0 && (spans[lo].0 - spans[lo - 1].1).abs() < 0.01 {
            lo -= 1;
        }
        while hi + 1 < spans.len() && (spans[hi + 1].0 - spans[hi].1).abs() < 0.01 {
            hi += 1;
        }
        // The step of the end face with a corner at metres m along.
        let step_at = |m: f64| {
            let near = |c: u32| (along(corner(c)) - m).abs() < 0.01;
            let s = m0.steps[walls..].iter().find(|s| near(s[0]) || near(s[1]))?;
            let [a, c] = if near(s[0]) { [s[0], s[3]] } else { [s[1], s[2]] };
            Some((h(a) - h(c)).abs())
        };
        let bounded = !spans[lo].2 && !spans[hi].2;
        runs.push((at, bounded.then(|| step_at(spans[lo].0).zip(step_at(spans[hi].1)).map(|(a, c)| [a, c])).flatten()));
    }
    EndWalls { faces, off_line, steps_on_line, loose, high, least, runs }
}

/// Twin bores into a hill (the made case above): on each portal line the cover meets the ground in front in end faces
/// beside the opening. Every step of the cover on a portal line is one, and none lies off a line; their corners are
/// storey 0's, the cover's above and the ground's below; the cover is their higher side; and beside each way's opening
/// they join it in one run that tapers to nothing at both ends. A single bore into the hill: beside its opening the cut
/// stands as high as the cover on the line, so its end faces have no height.
#[cfg(feature = "bores")]
#[test]
fn the_end_wall_spans_the_portal_line_beside_the_opening() {
    let (b, terrain, m0, _) = entrance(&[tube(0.0, "O 40 P B 160 P O"), tube(9.0, "O 50 P B 150 P O")], &hill);
    let w = end_walls(&b, &terrain, &m0);
    assert!(w.faces > 0 && w.high > 3.0, "{} end faces, the highest {} m", w.faces, w.high);
    assert_eq!((w.steps_on_line, w.off_line, w.loose), (0, 0, 0), "steps on a line, end faces off one, loose corners");
    assert!(w.least > -0.01, "the cover {} m under the ground at an end face", -w.least);
    // The lines that the ways enter by: before the cell's centre along the slant.
    let (centre, o) = (at([0.0, 0.0]), b.key.origin());
    let before =
        |p: [f64; 2]| ((o.x - centre.x) as f64 + p[0]) * SLANT[0] + ((o.y - centre.y) as f64 + p[1]) * SLANT[1] < 0.0;
    let entered: Vec<_> = w.runs.iter().filter(|r| before(r.0)).collect();
    assert_eq!(entered.len(), 2, "two portal lines entered");
    for (at, run) in entered {
        let ends = run.unwrap_or_else(|| panic!("{at:?}: no run of end faces about the opening"));
        assert!(ends.iter().all(|&e| e < 0.05), "{at:?}: the run ends in steps of {ends:?} m");
    }
    let (b, terrain, m0, _) = entrance(&[tube(0.0, "O 40 P B 160 P O")], &hill);
    let w = end_walls(&b, &terrain, &m0);
    assert!(w.faces > 0 && w.high < 0.01, "{} end faces, the highest {} m", w.faces, w.high);
}

/// A bore into a hill at a slant beside a road on the terrain, whose slope band reaches behind the portal line: the cut
/// of the way that enters ends on the line, so the ground behind it stands at the terrain.
#[cfg(feature = "bores")]
#[test]
fn the_cut_of_the_way_that_enters_ends_on_its_portal_line() {
    let (u, n) = (SLANT, [-SLANT[1], SLANT[0]]);
    let along: Vec<f64> = (0..=40).map(|i| -100.0 + 5.0 * f64::from(i)).collect();
    let mut beside = road(&along.iter().map(|a| [a * u[0] + 12.0 * n[0], a * u[1] + 12.0 * n[1]]).collect::<Vec<_>>());
    beside.z = along.iter().map(|&a| hill(a)).collect();
    let (.., seams) = entrance(&[tube(0.0, "O 40 P B 160 P O"), beside], &hill);
    assert!(seams.under > -0.1, "the ground {} m under the terrain behind the portal line", seams.under);
    assert!(seams.step < 0.05, "a step of {} m between the cover and the ground beside it", seams.step);
    assert!(seams.side < 0.1, "a corner on a portal line {} m off its face's side", seams.side);
}

/// A road that passes over its own bore 4 m up, as a way on top, before it descends into it (a spiral): an opening
/// lies only on the bore's end lines, not under the road on top.
#[cfg(feature = "bores")]
#[test]
fn a_way_over_its_own_bore_opens_it_only_at_its_ends() {
    let key = cell();
    let points = [[20.0, 30.0], [20.0, -30.0], [-40.0, -30.0], [-40.0, 0.0], [100.0, 0.0], [120.0, 0.0]];
    let levels = ("levels", "O 140 P B 290 P O");
    let mut way = line("tunnel", &[("highway", "residential"), ("tunnel", "yes"), levels], &points);
    way.z = vec![504.0, 504.0, 500.0, 500.0, 500.0, 500.0];
    let mut input = input_of(key, &[way]);
    input.terrain = plane(key, &|_, _| 503.1);
    let b = crate::cell::build_input(&input, Rules::embedded());
    assert!(!b.storeys.portals.is_empty(), "pieces on the portal lines");
    // The end lines: the joints of a ground way and a bore.
    let level = |a: &crate::graph::Arm| b.graph.ways[a.way as usize].level;
    let ends: Vec<[i32; 2]> = (b.graph.joints.iter())
        .filter(|j| {
            j.arms.iter().any(|a| level(a) == Level::Tunnel) && j.arms.iter().any(|a| level(a) == Level::Ground)
        })
        .map(|j| j.pos)
        .collect();
    assert_eq!(ends.len(), 2, "two portals");
    for piece in &b.storeys.portals {
        for p in [piece.a, piece.b] {
            let near = |e: &[i32; 2]| libm::hypot(f64::from(p[0] - e[0]), f64::from(p[1] - e[1])) * mpu() < 10.0;
            assert!(ends.iter().any(near), "a portal piece at {p:?}, off the end lines");
        }
    }
}
