use std::collections::BTreeMap;

use super::mesh::WORDS;
use super::tube::Net;
use super::*;

/// The test place: world units near 47° N, and metres east and north from it.
const X: f64 = 5.5e8;
const Y: f64 = 3.6e8;

fn world(east: f64, north: f64) -> [f64; 2] {
    let m = metres_per_unit(Y);
    [X + east / m, Y - north / m]
}

fn tags(pairs: &[(&str, String)]) -> Tags {
    pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect::<BTreeMap<_, _>>()
}

/// An arch end at metres east, north, floor z, heading in radians of world units, with its collar.
fn end(east: f64, north: f64, z: f64, heading: f64, collar: f64) -> End {
    let t = tags(&[
        ("part", "arch".into()),
        ("heading", heading.to_string()),
        ("section", "round".into()),
        ("half", "4.5".into()),
        ("height", "5".into()),
        ("look", "concrete".into()),
        ("collar", collar.to_string()),
        ("face", "7.1".into()),
        ("left", "6 4 2".into()),
        ("right", "3".into()),
        ("foot", "-2".into()),
        ("way", "0 0".into()),
        ("shift", "0".into()),
    ]);
    End::from_tags(world(east, north), z, &t, Rules::embedded()).unwrap()
}

/// A chord of a road tube from a to b, metres east, north and floor z, of `height`.
fn chord(a: [f64; 3], b: [f64; 3], height: f64, along: f64) -> Chord {
    let t = tags(&[
        ("section", "round".into()),
        ("half", "4.5".into()),
        ("height", height.to_string()),
        ("look", "concrete".into()),
        ("class", "primary".into()),
        ("road", "3.5".into()),
        ("along", along.to_string()),
        ("ends", "arch arch".into()),
    ]);
    let (pa, pb) = (world(a[0], a[1]), world(b[0], b[1]));
    let mut c = Chord::from_tags([pa[0], pa[1], a[2]], [pb[0], pb[1], b[2]], &t, Rules::embedded()).unwrap();
    c.code = 2;
    c
}

/// A tube 40 m east from an opening at the origin: its floor dips and rises, a lower chord where a way lies on top,
/// and a taller one further in; its two arch ends.
fn tube() -> (Vec<End>, Vec<Chord>) {
    let floor = [(0.0, 500.0), (4.0, 499.9), (8.0, 500.1), (12.0, 500.4), (16.0, 500.6), (24.0, 501.0), (40.0, 501.5)];
    let heights = [5.0, 5.0, 4.6, 5.0, 5.4, 5.0];
    let chords = (0..heights.len())
        .map(|i| {
            let ((sa, za), (sb, zb)) = (floor[i], floor[i + 1]);
            chord([sa, 0.0, za], [sb, 0.0, zb], heights[i], 100.0 + sa)
        })
        .collect();
    (vec![end(0.0, 0.0, 500.0, 0.0, 6.0), end(40.0, 0.0, 501.5, std::f64::consts::PI, 3.0)], chords)
}

/// Metres east and north of a world point from the test place.
fn metric(p: [f64; 2]) -> [f64; 2] {
    let m = metres_per_unit(Y);
    [(p[0] - X) * m, -(p[1] - Y) * m]
}

#[test]
fn the_box_of_an_arch_holds_its_section_and_clears_the_crown_at_each_chord_vertex() {
    let (ends, chords) = tube();
    let rules = Rules::embedded();
    let square = [X - 1.0, Y - 1.0, X + 1.0, Y + 1.0];
    let boxes = volumes(square, &ends[..1], &chords, rules).boxes;
    assert_eq!(boxes.len(), BOX);
    let [x, y, ux, uy, depth, half, roof, floor, rise] = boxes[..] else { unreachable!() };
    // The box starts at the front of the head wall, as the footprint does.
    let recess = rules.design.recess_m;
    assert_eq!([x, y, ux, uy], [world(-recess, 0.0)[0], world(-recess, 0.0)[1], 1.0, 0.0]);
    let shell = rules.ladder.bake.shell_m;
    assert!(depth >= ends[0].collar + 2.0 && depth < 40.0, "depth {depth}");
    assert!(half >= 4.5 + shell);
    let mut tested = 0;
    for c in &chords {
        for v in [c.a, c.b] {
            let s = metric([v[0], v[1]])[0] + recess;
            if s > depth {
                continue;
            }
            tested += 1;
            assert!(roof + rise * s >= v[2] + c.height + shell - 1e-9, "the roof clears the crown {s} m in");
            assert!(
                floor + rise * s <= v[2] + rules.design.lift_m + 1e-9,
                "the box's foot lies under the floor {s} m in"
            );
        }
    }
    assert!(tested >= 5);
}

#[test]
fn two_chords_at_a_bend_share_their_ring() {
    let rules = Rules::embedded();
    // A bend of 30° at (10, 0); the second chord runs against the way, as a way drawn the other way does.
    let bend = [10.0, 0.0, 500.5];
    let (c, s) = (libm::cos(30f64.to_radians()), libm::sin(30f64.to_radians()));
    let far = [10.0 + 8.0 * c, 8.0 * s, 501.0];
    let chords = vec![chord([0.0, 0.0, 500.0], bend, 5.0, 0.0), chord(far, bend, 5.0, 20.0)];
    let net = Net::new(&[], &chords, rules);
    let (r0, r1) = (net.ring(0, 1), net.ring(1, 1));
    let corners = tube::outline_of(&net, 0);
    let points = |r: &mesh::Station| {
        let mut p: Vec<[u64; 3]> = corners.iter().map(|&k| r.at(k).map(f64::to_bits)).collect();
        p.sort();
        p
    };
    assert_eq!(points(&r0), points(&r1));
    // The ring lies on the bisector, wider than the section by the bend.
    let (a, b) = (metric([r0.x + r0.ax, r0.y + r0.ay]), metric([r0.x, r0.y]));
    let across = [a[0] - b[0], a[1] - b[1]];
    let wide = libm::hypot(across[0], across[1]);
    assert!((wide - 1.0 / libm::cos(15f64.to_radians())).abs() < 1e-6, "{wide}");
    assert!((libm::atan2(across[1], across[0]).to_degrees() - 105.0).abs() < 1e-6);
}

#[test]
fn the_daylight_is_one_on_the_frame_line_and_none_from_daylight_m_in() {
    let (ends, chords) = tube();
    let rules = Rules::embedded();
    let net = Net::new(&ends[..1], &chords, rules);
    let at = |east: f64, north: f64| {
        let p = world(east, north);
        net.daylight([p[0], p[1], 500.0])
    };
    assert_eq!(at(0.0, 0.0), 1.0);
    assert_eq!(at(0.0, 4.5), 1.0);
    assert!((at(rules.design.daylight_m / 2.0, 0.0) - 0.5).abs() < 1e-9);
    assert_eq!(at(rules.design.daylight_m, 0.0), 0.0);
    assert_eq!(at(5.0, 4.5 + rules.ladder.bake.shell_m + 1.5), 0.0, "beside the opening's line");
    // Behind the head wall it is dark: in a second tunnel 50 m behind this exit, and 300 m behind and 40 m under it.
    assert_eq!(at(-70.0, 0.0), 0.0);
    let deep = world(-300.0, 0.0);
    assert_eq!(net.daylight([deep[0], deep[1], 460.0]), 0.0);
    // Under the floor of the opening, past its foot, it is dark too.
    let under = world(5.0, 0.0);
    assert_eq!(net.daylight([under[0], under[1], 497.0]), 0.0);
    // In a tube that is level for a metre and then rises at 10 %, the crown 8 m in has the light of the floor there.
    let steep =
        [chord([0.0, 0.0, 500.0], [1.0, 0.0, 500.0], 5.0, 0.0), chord([1.0, 0.0, 500.0], [21.0, 0.0, 502.0], 5.0, 1.0)];
    let ends = [end(0.0, 0.0, 500.0, 0.0, 6.0)];
    let net = Net::new(&ends, &steep, rules);
    let at8 = world(8.0, 0.0);
    let (floor, crown) = (net.daylight([at8[0], at8[1], 500.7]), net.daylight([at8[0], at8[1], 505.69]));
    assert!(floor > 0.0 && crown == floor, "floor {floor}, crown {crown}");
    // The mesh: the vertices on the frame line have the whole daylight.
    let mesh = meshes([X - 1.0, Y - 1.0, X + 1.0, Y + 1.0], &ends[..1], &chords, rules);
    let lit = mesh.vertices.chunks(WORDS).filter(|v| f32::from_bits(v[6]) == 1.0).count();
    assert!(lit > 20);
}

#[test]
fn each_face_draws_once_over_four_squares_about_an_end() {
    let (ends, chords) = tube();
    let rules = Rules::embedded();
    // The four squares meet 6 m in from the opening and 1 m to the left of the axis.
    let [cx, cy] = world(6.0, 1.0);
    let wide = 1e5;
    let quarters = [
        [cx - wide, cy - wide, cx, cy],
        [cx, cy - wide, cx + wide, cy],
        [cx - wide, cy, cx, cy + wide],
        [cx, cy, cx + wide, cy + wide],
    ];
    let triangles = |m: &Mesh| {
        let v = |i: u32| m.vertices[i as usize * WORDS..(i as usize + 1) * WORDS].to_vec();
        m.indices.chunks(3).map(|t| [v(t[0]), v(t[1]), v(t[2])]).collect::<Vec<_>>()
    };
    let meshes_of = |q: [f64; 4]| meshes(q, &ends, &chords, rules);
    assert!(
        quarters.iter().filter(|q| !meshes_of(**q).indices.is_empty()).count() >= 2,
        "the faces split over the squares"
    );
    let mut parts: Vec<_> = quarters.iter().flat_map(|q| triangles(&meshes_of(*q))).collect();
    let mut whole = triangles(&meshes_of([cx - wide, cy - wide, cx + wide, cy + wide]));
    parts.sort();
    whole.sort();
    assert_eq!(parts.len(), whole.len());
    assert!(parts == whole);
}

#[test]
fn a_footprint_holds_the_head_wall_and_the_collar() {
    let (ends, chords) = tube();
    let rules = Rules::embedded();
    let rings = footprints(&ends, &chords, rules);
    assert_eq!(rings.len(), 2);
    let ring: Vec<[f64; 2]> = rings[0].iter().map(|&p| metric(p)).collect();
    let inside = |p: [f64; 2]| {
        let mut odd = false;
        for (i, a) in ring.iter().enumerate() {
            let b = ring[(i + 1) % ring.len()];
            if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]) {
                odd = !odd;
            }
        }
        odd
    };
    let (d, edge) = (&rules.design, 4.5 + rules.ladder.bake.shell_m);
    // Heading east, the left is north: the wings reach 3 m to the left and 1 m to the right; the collar is 6 m.
    for p in [
        [-d.recess_m + 0.01, edge + 2.9],
        [d.face_m - d.recess_m - 0.01, -edge - 0.9],
        [5.9, edge - 0.01],
        [5.9, -edge + 0.01],
    ] {
        assert!(inside(p), "{p:?} inside");
    }
    for p in [[6.1, 0.0], [-d.recess_m - 0.01, 0.0], [3.0, edge + 0.01], [0.0, edge + 3.1]] {
        assert!(!inside(p), "{p:?} outside");
    }
}

/// The edges that one face of a mesh has alone, and those that more than two faces share; the void and the lamps
/// left out. Two faces share an edge where they have its two corners: the same words of position.
fn open_edges(mesh: &Mesh) -> (Vec<[[f64; 3]; 2]>, usize) {
    let key = |i: u32| {
        let w = &mesh.vertices[i as usize * WORDS..];
        [w[0], w[1], w[2]]
    };
    let solid = |i: u32| (mesh.vertices[i as usize * WORDS + 3] >> 24) & (mesh::VOID | mesh::LAMP) == 0;
    let mut count: BTreeMap<([u32; 3], [u32; 3]), usize> = BTreeMap::new();
    for t in mesh.indices.chunks(3).filter(|t| solid(t[0])) {
        for k in 0..3 {
            let (p, q) = (key(t[k]), key(t[(k + 1) % 3]));
            *count.entry(if p < q { (p, q) } else { (q, p) }).or_default() += 1;
        }
    }
    let point = |w: [u32; 3]| [w[0] as i32 as f64, w[1] as i32 as f64, f32::from_bits(w[2]) as f64];
    let open = count.iter().filter(|(_, &n)| n == 1).map(|((p, q), _)| [point(*p), point(*q)]).collect();
    (open, count.values().filter(|&&n| n > 2).count())
}

/// The open edges of the parts of a tube that lie neither at the foot of an end nor on the last ring of its collar,
/// and the edges that more than two faces share.
fn not_closed(ends: &[End], chords: &[Chord]) -> (Vec<[[f64; 3]; 2]>, usize) {
    let rules = Rules::embedded();
    let net = Net::new(ends, chords, rules);
    let back = rules.design.face_m - rules.design.recess_m;
    // Per end: its foot, and the last station of its collar's shell.
    let limits: Vec<(f64, mesh::Station)> = (0..ends.len())
        .map(|e| {
            let last = arch::collar(&net, e).last().map_or(net.frame(e, back), |p| p.1);
            (ends[e].z + ends[e].foot, last)
        })
        .collect();
    let allowed = |p: [f64; 3]| {
        limits.iter().any(|(foot, t)| {
            let m = metres_per_unit(t.y);
            let l = libm::hypot(t.ax, t.ay);
            let (dx, dy) = ((p[0] - t.x) * m, (p[1] - t.y) * m);
            let (along, beside) = ((dx * t.ay - dy * t.ax) / l, (dx * t.ax + dy * t.ay) / l);
            // Within the rounding of a vertex to whole world units.
            (p[2] - foot).abs() < 1e-3 || (along.abs() < 0.03 && beside.abs() < 10.0)
        })
    };
    let mesh = meshes([0.0, 0.0, 1.07e9, 1.07e9], ends, chords, rules);
    let (open, crowded) = open_edges(&mesh);
    (open.into_iter().filter(|e| !(allowed(e[0]) && allowed(e[1]))).collect(), crowded)
}

/// A tube that runs at `turn` degrees north of east from the origin, bends `bend` degrees to the left 4 m in, inside
/// the collar of its first end, and goes on 36 m: a lower chord in the collar and one further in, the floor rising.
fn bent(turn: f64, bend: f64) -> (Vec<End>, Vec<Chord>) {
    let (a0, a1) = (turn.to_radians(), (turn + bend).to_radians());
    let mut points = vec![([0.0, 0.0], 500.0), ([4.0 * libm::cos(a0), 4.0 * libm::sin(a0)], 500.1)];
    for k in 1..=6 {
        let (p, l) = (points[1].0, 6.0 * k as f64);
        points.push(([p[0] + l * libm::cos(a1), p[1] + l * libm::sin(a1)], 500.1 + 0.03 * l));
    }
    let heights = [5.0, 4.6, 5.0, 5.4, 4.8, 5.0, 5.0];
    let chords = (points.windows(2).zip(heights))
        .map(|(w, h)| chord([w[0].0[0], w[0].0[1], w[0].1], [w[1].0[0], w[1].0[1], w[1].1], h, 0.0))
        .collect();
    let (last, pi) = (points[points.len() - 1], std::f64::consts::PI);
    (vec![end(0.0, 0.0, 500.0, -a0, 6.0), end(last.0[0], last.0[1], last.1, -(a1 + pi), 3.0)], chords)
}

#[test]
fn a_part_is_closed_but_at_its_foot_and_at_the_end_of_its_collar() {
    let (ends, mut rock) = bent(30.0, 20.0);
    rock.iter_mut().for_each(|c| c.class = "footway".into());
    for (name, (ends, chords)) in [("straight", tube()), ("turned and bent", bent(30.0, 20.0)), ("rock", (ends, rock))]
    {
        let (open, crowded) = not_closed(&ends, &chords);
        assert!(open.is_empty(), "{name}: {} open edges, the first {:?}", open.len(), &open[..open.len().min(4)]);
        assert_eq!(crowded, 0, "{name}: edges of more than two faces");
    }
}
