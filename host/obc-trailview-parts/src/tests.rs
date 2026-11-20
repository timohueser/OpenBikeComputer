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

/// A world point at station `st` of a chord: `across` metres to the left and `up` over its floor there.
fn point_at(st: &mesh::Station, across: f64, up: f64) -> [f64; 3] {
    st.at([across, up])
}

/// Whether a world point lies inside the outer shell of a chord as the part draws it: between the planes of its two
/// rings, and inside the outline of its shell (`tube::shell_outline`, the polygon of the faces) over the floor there,
/// from its foot `foot` (a world height) up. The floor at a point rises by its share between the rings, as the faces
/// run straight from ring to ring.
fn in_shell(net: &Net, c: usize, foot: f64, p: [f64; 3]) -> bool {
    let chord = &net.chords[c];
    let (s, t) = (net.ring(c, 0), net.ring(c, 1));
    let (d, len) = tube::direction(chord);
    let m = metres_per_unit(s.y);
    // The side of a ring's plane that faces into the chord.
    let side = |r: &mesh::Station, p: [f64; 3], into: f64| {
        let n = [r.ay, -r.ax];
        let n = if n[0] * d[0] + n[1] * d[1] >= 0.0 { n } else { [-n[0], -n[1]] };
        into * ((p[0] - r.x) * n[0] + (p[1] - r.y) * n[1]) * m >= -1e-3 * libm::hypot(n[0], n[1])
    };
    if !side(&s, p, 1.0) || !side(&t, p, -1.0) {
        return false;
    }
    let (dx, dy) = ((p[0] - s.x) * m, (p[1] - s.y) * m);
    let left = mesh::left_of(d);
    let (along, across) = (dx * d[0] + dy * d[1], dx * left[0] + dy * left[1]);
    // The shell's faces run straight from ring to ring: at `across`, the floor rises by the share between them.
    let share = boxes::share(along, across, len, [boxes::slope(d, &s), boxes::slope(d, &t)]).clamp(0.0, 1.0);
    let floor = s.z + (t.z - s.z) * share;
    let o = tube::shell_outline(net, chord.section, chord.half, chord.height, foot - floor);
    let q = [across, p[2] - floor];
    let mut odd = false;
    for (i, a) in o.iter().enumerate() {
        let b = o[(i + 1) % o.len()];
        if (a[1] > q[1]) != (b[1] > q[1]) && q[0] < a[0] + (q[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]) {
            odd = !odd;
        }
    }
    odd
}

/// The least margin of a box to the faces of its part (the spec).
const MARGIN_M: f64 = 0.11;

/// A box shrunk by `g` metres in its sides and its roof.
fn shrunk(b: &[f64; BOX], g: f64) -> [f64; BOX] {
    let mut s = *b;
    (s[5], s[6]) = (b[5] - g, b[6] - g);
    s[9] = s[9].min(s[6]);
    s
}

/// Checks the boxes of each built end of a tube, with the margin `MARGIN_M`: each point of the room (the inside of the
/// opening's hole and of each chord up to the reach of the boxes) lies in a box, and over the floor in a box shrunk by
/// the margin; each point of a tube's box, grown by the margin in its sides and its roof, lies inside the outer shell
/// of a chord as the faces draw it. Gives the count of each kind of sample.
fn boxes_fit(name: &str, ends: &[End], chords: &[Chord]) -> (usize, usize) {
    let rules = Rules::embedded();
    let net = Net::new(ends, chords, rules);
    let design = &rules.design;
    // Within the rounding of a box's words and of a sample: a millimetre.
    let tol = 1e-3;
    let (mut room, mut solid) = (0, 0);
    for e in (0..ends.len()).filter(|&e| net.built_end(e)) {
        let list = boxes::boxes(&net, e);
        let small: Vec<[f64; BOX]> = list.iter().map(|b| shrunk(b, MARGIN_M)).collect();
        let held = |boxes: &[[f64; BOX]], p: [f64; 3]| {
            boxes.iter().map(|b| outside(b, p)).fold(f64::INFINITY, f64::min) <= tol
        };
        let reach = boxes::reach(&net, e).boxes;
        let foot = boxes::foot(&net, e);
        let (c0, _) = net.opening(e).unwrap();
        let first = &net.chords[c0];
        let sp = tube::spring(&net, first.section, first.height);
        // The room: the opening's hole from the front of the head wall to the frame line.
        for k in 0..=4 {
            let st = net.frame(e, -design.recess_m * (1.0 - k as f64 / 4.0));
            for a in [-1.0, -0.5, 0.0, 0.5, 1.0] {
                let hw = first.half + design.frame_m;
                for up in [design.lift_m + 0.01, 1.0, sp, first.height] {
                    let r = if up > sp {
                        libm::sqrt(1.0 - ((up - sp) / (first.height + design.frame_m - sp)).powi(2))
                    } else {
                        1.0
                    };
                    let p = point_at(&st, a * hw * r, up);
                    assert!(held(&list, p), "{name}, end {e}: the hole at {a} across, {up} up, {k}/4 of the reveal");
                    room += 1;
                }
            }
        }
        // The room of each chord up to the reach.
        for (c, forward, start) in net.walk(e, reach) {
            let chord = &net.chords[c];
            let (_, len) = tube::direction(chord);
            let (s, t) = if forward { (net.ring(c, 0), net.ring(c, 1)) } else { (net.ring(c, 1), net.ring(c, 0)) };
            let sp = tube::spring(&net, chord.section, chord.height);
            for k in 0..=8 {
                let u = k as f64 / 8.0;
                if start + u * len > reach {
                    continue;
                }
                let st = s.mix(&t, u);
                for a in [-1.0, -0.5, 0.0, 0.5, 1.0] {
                    for up in [design.lift_m + 0.01, 1.0, sp, (sp + chord.height) / 2.0, chord.height] {
                        let r = if up > sp && chord.section == Section::Round {
                            libm::sqrt((1.0 - ((up - sp) / (chord.height - sp)).powi(2)).max(0.0))
                        } else {
                            1.0
                        };
                        let p = point_at(&st, a * chord.half * r, up);
                        assert!(held(&list, p), "{name}, end {e}, chord {c} at {u}: the room at {a} across, {up} up");
                        let over = up > design.lift_m + 0.02;
                        assert!(
                            !over || held(&small, p),
                            "{name}, end {e}, chord {c} at {u}: the room at {a} across, {up} up, within the margin"
                        );
                        room += 1;
                    }
                }
            }
        }
        // Each box of the tube, grown by the margin: points of its sides, its floor and its roof, at shares 0 to 1
        // between its end planes, inside the drawn shell.
        let m = metres_per_unit(ends[e].at[1]);
        for (i, b) in list.iter().enumerate().skip(1) {
            let lu = mesh::left_of([b[2], b[3]]);
            let (depth, half, roof, floor, spring) = (b[4], b[5] + MARGIN_M, b[6] + MARGIN_M, b[7], b[9]);
            for k in 0..=4 {
                let f = k as f64 / 4.0;
                for a in [-1.0, -0.5, 0.0, 0.5, 1.0] {
                    let left = a * half;
                    let along = b[10] * left + (depth + (b[11] - b[10]) * left) * f;
                    let top = if roof > spring && left.abs() < half {
                        spring + (roof - spring) * libm::sqrt(1.0 - (left / half).powi(2))
                    } else {
                        spring.min(roof)
                    };
                    for z in [floor, (floor + top) / 2.0, top] {
                        let q = [
                            b[0] + (b[2] * along + lu[0] * left) / m,
                            b[1] + (b[3] * along + lu[1] * left) / m,
                            z + b[8] * depth * f,
                        ];
                        let held = (0..net.chords.len()).any(|c| in_shell(&net, c, foot, q));
                        assert!(held, "{name}, end {e}, box {i}: {k}/4 in, {left} left, {z} up lies outside the shell");
                        solid += 1;
                    }
                }
            }
        }
    }
    (room, solid)
}

/// A tube with a bend of `bend` degrees, 4 m in, on a grade `grade`, of one height.
fn graded(bend: f64, grade: f64) -> (Vec<End>, Vec<Chord>) {
    let (mut ends, mut chords) = bent(-10.0, bend);
    let mut z = 500.0;
    for c in chords.iter_mut() {
        let len = tube::direction(c).1;
        (c.a[2], c.b[2], c.height, z) = (z, z + grade * len, 5.0, z + grade * len);
    }
    ends[1].z = z;
    (ends, chords)
}

#[test]
fn the_boxes_hold_the_room_and_lie_inside_the_part() {
    let rules = Rules::embedded();
    let (straight, lower) = tube();
    let (bent20, chords20) = bent(30.0, 20.0);
    let (bent60, chords60) = graded(60.0, 0.0);
    let mut boxed = lower.clone();
    boxed.iter_mut().for_each(|c| c.section = Section::Box);
    let mut cases = vec![
        ("straight with a lower chord".to_string(), straight.clone(), lower.clone()),
        ("bent 20° in the collar".to_string(), bent20, chords20),
        ("bent 60° on a level floor".to_string(), bent60, chords60),
        ("a box section".to_string(), straight.clone(), boxed),
    ];
    // Sharp bends on grades: the ends of the boxes there slant, and the boxes rise between them as the shell does.
    for (bend, grade) in [(30.0, 0.15), (45.0, 0.15), (60.0, 0.15), (60.0, 0.25)] {
        let (ends, chords) = graded(bend, grade);
        cases.push((format!("bent {bend}° on a grade of {grade}"), ends, chords));
    }
    for (name, ends, chords) in &cases {
        let (room, solid) = boxes_fit(name, ends, chords);
        assert!(room > 200 && solid > 100, "{name}: {room} room samples, {solid} box samples");
    }
    // The opening's box starts at the front of the head wall, each other box at a vertex of the axis, the first on the
    // frame line; the depth past the collar follows the start of the cover.
    let net = Net::new(&straight, &lower, rules);
    let list = boxes::boxes(&net, 0);
    let recess = rules.design.recess_m;
    assert_eq!(list[0][..4], [world(-recess, 0.0)[0], world(-recess, 0.0)[1], 1.0, 0.0]);
    assert_eq!(list[1][..2], straight[0].at);
    let reach = boxes::reach(&net, 0);
    assert!(reach.boxes >= straight[0].collar + 1.0 && reach.closed, "{reach:?}");
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

/// Edges as their two corners.
type Edges = Vec<[[f64; 3]; 2]>;

/// The edges that one face of a mesh has alone, and those that more than two faces share; the void and the lamps
/// left out. Two faces share an edge where they have its two corners: the same words of position.
fn open_edges(mesh: &Mesh) -> (Edges, Edges) {
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
    let edges = |of: &dyn Fn(usize) -> bool| {
        count.iter().filter(|(_, &n)| of(n)).map(|((p, q), _)| [point(*p), point(*q)]).collect::<Vec<_>>()
    };
    (edges(&|n| n == 1), edges(&|n| n > 2))
}

/// The open edges of the parts of a tube that do not lie at the foot of an end, and the edges that more than two faces
/// share but at the face that closes a shell: there the inside of the tube goes on past it.
fn not_closed(ends: &[End], chords: &[Chord]) -> (Edges, Edges) {
    let rules = Rules::embedded();
    let net = Net::new(ends, chords, rules);
    let feet: Vec<f64> = (0..ends.len()).filter(|&e| net.built_end(e)).map(|e| boxes::foot(&net, e)).collect();
    // Per end whose shell a face closes, the station of that face.
    let faces: Vec<mesh::Station> = (0..ends.len())
        .filter(|&e| net.built_end(e))
        .filter_map(|e| {
            let r = boxes::reach(&net, e);
            arch::collar(&net, e, r.shell).last().filter(|_| r.closed).map(|p| p.1)
        })
        .collect();
    // Within the rounding of a vertex to whole world units.
    let at_foot = |p: [f64; 3]| feet.iter().any(|f| (p[2] - f).abs() < 1e-3);
    let on_face = |p: [f64; 3]| {
        faces.iter().any(|t| {
            let m = metres_per_unit(t.y);
            let l = libm::hypot(t.ax, t.ay);
            let (dx, dy) = ((p[0] - t.x) * m, (p[1] - t.y) * m);
            ((dx * t.ay - dy * t.ax) / l).abs() < 0.03
        })
    };
    let mesh = meshes([0.0, 0.0, 1.07e9, 1.07e9], ends, chords, rules);
    let (open, crowded) = open_edges(&mesh);
    (
        open.into_iter().filter(|e| !(at_foot(e[0]) && at_foot(e[1]))).collect(),
        crowded.into_iter().filter(|e| !(on_face(e[0]) && on_face(e[1]))).collect(),
    )
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

/// A tube 18 m east, too short for the parts of its two ends: a lower chord at its middle vertex.
fn short() -> (Vec<End>, Vec<Chord>) {
    let floor = [(0.0, 500.0), (5.0, 500.2), (9.0, 500.3), (13.0, 500.4), (18.0, 500.6)];
    let heights = [5.0, 5.0, 4.6, 4.6];
    let chords = (0..heights.len())
        .map(|i| {
            let ((sa, za), (sb, zb)) = (floor[i], floor[i + 1]);
            chord([sa, 0.0, za], [sb, 0.0, zb], heights[i], sa)
        })
        .collect();
    (vec![end(0.0, 0.0, 500.0, 0.0, 6.0), end(18.0, 0.0, 500.6, std::f64::consts::PI, 3.0)], chords)
}

#[test]
fn a_part_is_closed_but_at_its_foot() {
    let (ends, mut rock) = bent(30.0, 20.0);
    rock.iter_mut().for_each(|c| c.class = "footway".into());
    for (name, (ends, chords)) in [
        ("straight", tube()),
        ("turned and bent", bent(30.0, 20.0)),
        ("rock", (ends, rock)),
        ("two shells that meet", short()),
    ] {
        let (open, crowded) = not_closed(&ends, &chords);
        assert!(open.is_empty(), "{name}: {} open edges, the first {:?}", open.len(), &open[..open.len().min(4)]);
        assert!(
            crowded.is_empty(),
            "{name}: {} edges of more than two faces, the first {:?}",
            crowded.len(),
            &crowded[..crowded.len().min(4)]
        );
    }
    // The shells of the short tube meet at its middle vertex, with no face between them; the longer tubes close theirs.
    let (ends, chords) = short();
    let net = Net::new(&ends, &chords, Rules::embedded());
    let (r0, r1) = (boxes::reach(&net, 0), boxes::reach(&net, 1));
    assert!(
        !r0.closed && !r1.closed && (r0.shell - 9.0).abs() < 1e-6 && (r1.shell - 9.0).abs() < 1e-6,
        "{r0:?} {r1:?}"
    );
    assert_eq!((r0.then.lower, r1.then.lower), (Some((4.6, Section::Round)), None));
    assert_eq!((r0.then.foot, r1.then.foot), (Some(boxes::foot(&net, 1)), Some(boxes::foot(&net, 0))));
}
