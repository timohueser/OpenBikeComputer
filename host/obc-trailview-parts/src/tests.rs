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

/// A tube as an underpass of a way of `class`: `box` ends and box sections.
fn underpass((mut ends, mut chords): (Vec<End>, Vec<Chord>), class: &str) -> (Vec<End>, Vec<Chord>) {
    for e in &mut ends {
        (e.part, e.section) = (Part::Box, Section::Box);
    }
    for c in &mut chords {
        (c.section, c.ends, c.class) = (Section::Box, [Some(Part::Box); 2], class.into());
    }
    (ends, chords)
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
    for e in (0..net.ends.len()).filter(|&e| net.built_end(e)) {
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
        for k in (0..=4).filter(|_| !net.closed(e)) {
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
        let m = metres_per_unit(net.ends[e].at[1]);
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
    for (name, (ends, chords)) in [
        ("an underpass", underpass(tube(), "primary")),
        ("a path underpass", underpass(bent(30.0, 20.0), "footway")),
        ("an underpass of one chord", one_chord()),
    ] {
        cases.push((name.to_string(), ends, chords));
    }
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
fn the_volumes_near_a_square_are_those_within_reach_of_it() {
    let (ends, chords) = tube();
    let rules = Rules::embedded();
    let all = volumes(&ends, &chords, rules);
    let (m, reach) = (metres_per_unit(Y), rules.design.reach_m);
    let near = |square| near_volumes(square, &all.boxes, &all.floors, rules);
    // The westmost point of the volumes: of a box's corners and of a floor segment's ends.
    let corners_x = all.boxes.as_chunks::<BOX>().0.iter().flat_map(|b| corners(b).map(|c| c[0]));
    let west = corners_x
        .chain(all.floors.as_chunks::<FLOOR>().0.iter().flat_map(|f| [f[0], f[2]]))
        .fold(f64::INFINITY, f64::min);
    // Squares 1 km high about the test place, west of the part, whose east side lies `metres` short of its reach.
    let short_of = |metres: f64| {
        near([west - (reach + metres + 500.0) / m, Y - 500.0 / m, west - (reach + metres) / m, Y + 500.0 / m])
    };
    assert!(!all.boxes.is_empty() && !all.floors.is_empty());
    // Inside: a square over the whole part keeps all, in their order.
    assert_eq!(near([X - 500.0 / m, Y - 500.0 / m, X + 500.0 / m, Y + 500.0 / m]), all);
    // On the border: half a metre inside the reach keeps the westmost volumes, but not all; half a metre out, none.
    let edge = short_of(-0.5);
    assert!(!edge.boxes.is_empty() && edge != all, "{} of {} box words", edge.boxes.len(), all.boxes.len());
    assert_eq!(short_of(0.5), Volumes::default());
    // Empty: a square 1 km north keeps none.
    assert_eq!(near([X - 500.0 / m, Y - 1500.0 / m, X + 500.0 / m, Y - 1000.0 / m]), Volumes::default());
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
    let built = || (0..net.ends.len()).filter(|&e| net.built_end(e));
    let feet: Vec<f64> = built().map(|e| boxes::foot(&net, e)).collect();
    // Per end whose shell a face closes, the station of that face; and the front of a closed end.
    let faces: Vec<mesh::Station> = (built())
        .filter_map(|e| {
            let r = boxes::reach(&net, e);
            arch::collar(&net, e, r.shell).last().filter(|_| r.closed).map(|p| p.1)
        })
        .chain(built().filter(|&e| net.closed(e)).map(|e| net.frame(e, 0.0)))
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

/// An underpass 10 m east of one chord, under a way whose reach covers it all.
fn one_chord() -> (Vec<End>, Vec<Chord>) {
    let ends = vec![end(0.0, 0.0, 500.0, 0.0, 3.0), end(10.0, 0.0, 500.2, std::f64::consts::PI, 3.0)];
    underpass((ends, vec![chord([0.0, 0.0, 500.0], [10.0, 0.0, 500.2], 5.0, 0.0)]), "primary")
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
        ("an underpass", underpass(tube(), "primary")),
        ("a path underpass", underpass(bent(30.0, 20.0), "footway")),
        ("an underpass of one chord", one_chord()),
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
    // An underpass is one part: the parts of its two ends meet at the vertex nearest the middle, 16 m and 24 m in (of
    // two as near, the one with the least key), with their boxes; those of a tube of one chord at its middle.
    let (ends, chords) = underpass(tube(), "primary");
    let net = Net::new(&ends, &chords, Rules::embedded());
    let (r0, r1) = (boxes::reach(&net, 0), boxes::reach(&net, 1));
    assert!(!r0.closed && !r1.closed, "{r0:?} {r1:?}");
    assert_eq!([r0.boxes, r0.shell, r1.boxes, r1.shell].map(|m| (m * 1e6).round() / 1e6), [16.0, 16.0, 24.0, 24.0]);
    let (ends, chords) = one_chord();
    let net = Net::new(&ends, &chords, Rules::embedded());
    let (r0, r1) = (boxes::reach(&net, 0), boxes::reach(&net, 1));
    assert!(
        !r0.closed && [r0.boxes, r0.shell, r1.boxes, r1.shell].iter().all(|m| (m - 5.0).abs() < 1e-6),
        "{r0:?} {r1:?}"
    );
}

#[test]
fn a_wing_ends_as_high_as_its_coping() {
    // With a tube 4 m wide inside, the frame's outer edge lies 4.5 m to each side, so columns of the wall stand half a
    // metre out from it. The test end's left row `6 4 2` runs 3 m out, its right row `3` 1 m, from `face` 7.1 m. Each
    // wing's top is at most a line from `face` at the edge to its coping at its end: 2.27 m per metre on the left, 6.8 m
    // on the right, as neither row is long enough for `wing_slope` (1 m per metre). Under it, the row.
    let (ends, mut chords) = tube();
    chords.iter_mut().for_each(|c| c.half = 4.0);
    let rules = Rules::embedded();
    let mesh = meshes([X - 1.0, Y - 1.0, X + 1.0, Y + 1.0], &ends[..1], &chords, rules);
    let top = |north: f64| {
        let at = |v: &[u32]| metric([v[0] as i32 as f64, v[1] as i32 as f64]);
        let column = mesh.vertices.chunks(WORDS).filter(|v| at(v)[0] < 1.0 && (at(v)[1] - north).abs() < 0.03);
        column.map(|v| f32::from_bits(v[2]) as f64 - 500.0).fold(f64::NEG_INFINITY, f64::max)
    };
    let coping = rules.ladder.bake.coping_m;
    let left = |d: f64| coping + (7.1 - coping) / 3.0 * (3.0 - d);
    // The floor of the frame falls 2.5 cm per metre in; the head wall's front stands 0.25 m out of the frame line.
    let want = [(4.5, 7.1), (5.0, left(0.5)), (6.0, left(1.5)), (7.0, 3.0f64.min(left(2.5))), (7.5, coping)];
    for (north, want) in want.into_iter().chain([(-5.0, (7.1 + coping) / 2.0), (-5.5, coping)]) {
        assert!((top(north) - want).abs() < 0.02, "{north} m to the left: the top at {} m, not {want} m", top(north));
    }
}

#[test]
fn the_faces_along_a_tube_face_out_of_its_shell_and_into_its_room() {
    // The tubes run east along the line 0 m north, about 2.5 m over their floor at about 500.7 m: a face of the outer
    // shell faces away from that line, a face of the inside (the walls, the vault or roof, the floor) toward it.
    let rules = Rules::embedded();
    let shell = rules.looks["concrete"].shell;
    for (name, (ends, chords)) in [("round", tube()), ("box", underpass(tube(), "primary"))] {
        let mesh = meshes([0.0, 0.0, 1.07e9, 1.07e9], &ends, &chords, rules);
        let point = |i: u32| {
            let w = &mesh.vertices[i as usize * WORDS..];
            let p = metric([w[0] as i32 as f64, w[1] as i32 as f64]);
            ([p[0], p[1], f32::from_bits(w[2]) as f64], w[3])
        };
        let (mut out, mut inside) = (0, 0);
        for t in mesh.indices.chunks(3) {
            let [(a, c), (b, _), (d, _)] = [point(t[0]), point(t[1]), point(t[2])];
            let side = if c == shell {
                1.0
            } else if (c >> 24) & mesh::BORE != 0 {
                -1.0
            } else {
                continue;
            };
            let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [d[0] - a[0], d[1] - a[1], d[2] - a[2]]);
            let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
            let size = libm::sqrt(n[0] * n[0] + n[1] * n[1] + n[2] * n[2]);
            if n[0].abs() > 0.5 * size {
                continue;
            }
            let mid = [(a[1] + b[1] + d[1]) / 3.0, (a[2] + b[2] + d[2]) / 3.0 - 502.5];
            assert!(side * (n[1] * mid[0] + n[2] * mid[1]) > 0.0, "{name}: a face at {a:?} faces the wrong way");
            if side > 0.0 {
                out += 1;
            } else {
                inside += 1;
            }
        }
        assert!(out > 20 && inside > 20, "{name}: {out} faces of the shell, {inside} inside");
    }
}

/// A tube as one of `part` at both ends, of a way of `class`.
fn kind((mut ends, mut chords): (Vec<End>, Vec<Chord>), part: Part, class: &str) -> (Vec<End>, Vec<Chord>) {
    for e in &mut ends {
        e.part = part;
    }
    for c in &mut chords {
        (c.ends, c.class) = ([Some(part); 2], class.into());
    }
    (ends, chords)
}

/// Two road tubes 40 m east from a pair at each end: the first on the line 0 m north, the second `apart` metres north
/// with its floor `dz` higher; each of two chords.
fn pair(apart: f64, dz: f64) -> (Vec<End>, Vec<Chord>) {
    let pi = std::f64::consts::PI;
    let mut ends = vec![end(0.0, 0.0, 500.0, 0.0, 6.0), end(40.0, 0.0, 500.2, pi, 3.0)];
    // Into the tubes from the west the second lies to the left, from the east to the right.
    for (e, mate) in ends.iter_mut().zip([apart, -apart]) {
        e.part = Part::Pair;
        e.mate = Some(Mate { across: mate, way: [1, 1], shift: 0.0, collar: e.collar });
    }
    let mut chords = [lane(0.0, 500.0), lane(apart, 500.0 + dz)].concat();
    chords.iter_mut().for_each(|c| c.ends = [Some(Part::Pair); 2]);
    (ends, chords)
}

/// The two chords of a road tube 40 m east on the line `north` metres north, its floor from z rising 0.5 %.
fn lane(north: f64, z: f64) -> Vec<Chord> {
    [(0.0, 20.0), (20.0, 40.0)]
        .map(|(a, b)| chord([a, north, z + a * 0.005], [b, north, z + b * 0.005], 5.0, a))
        .to_vec()
}

/// Road tubes as those of `pair`, on the lines `norths` metres north, with an arch at each end: no end has a mate.
fn row(norths: &[f64]) -> (Vec<End>, Vec<Chord>) {
    let (mut ends, mut chords) = (Vec::new(), Vec::new());
    for &north in norths {
        ends.extend([end(0.0, north, 500.0, 0.0, 6.0), end(40.0, north, 500.2, std::f64::consts::PI, 3.0)]);
        chords.extend(lane(north, 500.0));
    }
    for e in &mut ends {
        e.part = Part::Arch;
    }
    for c in &mut chords {
        c.ends = [Some(Part::Arch); 2];
    }
    (ends, chords)
}

/// The triangles of a mesh in metres east, north and up, but the dark faces of the openings and the lamps.
fn solid(mesh: &Mesh) -> Vec<[[f64; 3]; 3]> {
    let point = |i: u32| {
        let w = &mesh.vertices[i as usize * WORDS..];
        let p = metric([w[0] as i32 as f64, w[1] as i32 as f64]);
        [p[0], p[1], f32::from_bits(w[2]) as f64]
    };
    let shown = |i: u32| (mesh.vertices[i as usize * WORDS + 3] >> 24) & (mesh::VOID | mesh::LAMP) == 0;
    mesh.indices.chunks(3).filter(|t| shown(t[0])).map(|t| [point(t[0]), point(t[1]), point(t[2])]).collect()
}

/// The first face that a ray from `o` along `d` meets within `far` metres: its distance, and whether the ray meets its
/// front, or the front of a face at the same distance (two faces back to back).
fn hit(faces: &[[[f64; 3]; 3]], o: [f64; 3], d: [f64; 3], far: f64) -> Option<(f64, bool)> {
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let cross =
        |a: [f64; 3], b: [f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let mut best: Option<(f64, bool)> = None;
    for [a, b, c] in faces {
        let (e1, e2) = (sub(*b, *a), sub(*c, *a));
        let p = cross(d, e2);
        let det = dot(e1, p);
        if det.abs() < 1e-12 {
            continue;
        }
        let tv = sub(o, *a);
        let (u, q) = (dot(tv, p) / det, cross(tv, e1));
        let (v, t) = (dot(d, q) / det, dot(e2, q) / det);
        if u < -1e-9 || v < -1e-9 || u + v > 1.0 + 1e-9 || t <= 1e-6 || t > far {
            continue;
        }
        let front = dot(cross(e1, e2), d) < 0.0;
        best = match best {
            Some(b) if (t - b.0).abs() < 1e-6 => Some((b.0, b.1 || front)),
            Some(b) if b.0 < t => Some(b),
            _ => Some((t, front)),
        };
    }
    best
}

#[test]
fn the_tubes_of_a_pair_or_side_by_side_are_one_room_and_their_heads_end_between() {
    let rules = Rules::embedded();
    let frame = rules.design.frame_m;
    // Tubes 7 m apart, 4.5 m wide each: their rooms overlap by 2 m, so they are cut in the middle; with the second's
    // floor 0.8 m higher, faces close the steps there. Tubes 10.5 m apart do not overlap. Arches that are no pair end
    // their head walls at the middle too, the middle tube of three on both sides.
    let cases: [(&[f64], f64, bool); 5] = [
        (&[0.0, 7.0], 0.0, true),
        (&[0.0, 7.0], 0.8, true),
        (&[0.0, 10.5], 0.0, true),
        (&[0.0, 10.5], 0.0, false),
        (&[0.0, 7.0, 14.0], 0.0, false),
    ];
    for (norths, dz, paired) in cases {
        let name = format!("tubes at {norths:?} m north, the second {dz} m higher, a pair: {paired}");
        let (ends, chords) = if paired { pair(norths[1], dz) } else { row(norths) };
        let (room, box_points) = boxes_fit(&name, &ends, &chords);
        assert!(room > 200 && box_points > 100, "{name}: {room} room samples, {box_points} box samples");
        let mesh = meshes([0.0, 0.0, 1.07e9, 1.07e9], &ends, &chords, rules);
        let faces = solid(&mesh);
        let o = tube::outline_of(&Net::new(&ends, &chords, rules), 0);
        // A point's tube, the nearest; and its floor at the opening.
        let of = |north: f64| {
            let i = (0..norths.len()).min_by(|&i, &j| (north - norths[i]).abs().total_cmp(&(north - norths[j]).abs()));
            let i = i.unwrap();
            (norths[i], if i == 1 { 500.0 + dz } else { 500.0 })
        };
        let middles: Vec<f64> = norths.windows(2).map(|w| (w[0] + w[1]) / 2.0).collect();
        // Each tube's room up to the middles: no face lies in it. A face in a middle lies within the rounding of its
        // vertices to world units.
        let inside = |p: [f64; 3]| {
            let (axis, z) = of(p[1]);
            let (x, up) = (p[1] - axis, p[2] - z - p[0] * 0.005);
            middles.iter().all(|m| (p[1] - m).abs() >= 0.05)
                && p[0] > 0.5
                && p[0] < 39.5
                && x.abs() < 4.45
                && up > 0.25
                && tube::top_at(&o, x).is_some_and(|t| up < t - 0.05)
        };
        for f in &faces {
            let c = [0, 1, 2].map(|k| (f[0][k] + f[1][k] + f[2][k]) / 3.0);
            assert!(!inside(c), "{name}: a face in the room at {c:?}");
        }
        // From inside the room, every way across meets the front of a face: the room is closed at the middles.
        let (first, last) = (norths[0], norths[norths.len() - 1]);
        let mut from = vec![(first - 3.0, 1.0), (last + 3.0, 1.0)];
        for &n in norths {
            from.extend([(n - 2.5, 4.0), (n, 2.5), (n + 2.5, 4.0)]);
        }
        for (north, up) in from {
            let z = of(north).1;
            for k in 0..24 {
                let a = (k as f64 * 15.0 + 7.0).to_radians();
                let d = [0.0, libm::cos(a), libm::sin(a)];
                let got = hit(&faces, [20.0, north, z + 0.1 + up], d, 30.0);
                assert!(got.is_some_and(|h| h.1), "{name}: from {north} m north, {up} m up, at {a} rad: {got:?}");
            }
        }
        // One head wall: in front of the frame line, a ray east meets its front between two holes, but none in a hole;
        // where the holes overlap, they are one opening.
        for &north in &middles {
            let got = hit(&faces, [-3.0, north, 501.0], [1.0, 0.0, 0.0], 10.0);
            if norths[1] - norths[0] < 2.0 * (4.5 + frame) {
                assert!(got.is_none_or(|h| h.0 > 3.5), "{name}: the middle of one opening: {got:?}");
            } else {
                assert!(got.is_some_and(|h| h.1 && h.0 < 3.0), "{name}: the head wall between the holes: {got:?}");
            }
        }
        for &north in norths {
            let got = hit(&faces, [-3.0, north, 502.5], [1.0, 0.0, 0.0], 10.0);
            assert!(got.is_none_or(|h| h.0 > 3.5), "{name}: the hole at {north} m north: {got:?}");
        }
    }
}

#[test]
fn a_rim_hugs_its_opening_and_a_covered_free_end_is_closed() {
    let rules = Rules::embedded();
    let d = &rules.design;
    let (shell, frame) = (rules.ladder.bake.shell_m, d.frame_m);
    // An opening in the rock and a free tube end: the head is a rim round the opening, 0.95 m wide, with no wings.
    for (name, (ends, chords)) in [
        ("rock", kind(bent(30.0, 20.0), Part::Rock, "footway")),
        ("free", kind(tube(), Part::Free, "primary")),
        ("closed", kind(tube(), Part::Free, "primary")),
    ] {
        let mut ends = ends;
        if name == "closed" {
            ends[0].collar = 0.0;
        }
        let (room, box_points) = boxes_fit(name, &ends, &chords);
        assert!(room > 100 && box_points > 50, "{name}: {room} room samples, {box_points} box samples");
        let (open, crowded) = not_closed(&ends, &chords);
        assert!(open.is_empty(), "{name}: {} open edges, the first {:?}", open.len(), &open[..open.len().min(4)]);
        assert!(
            crowded.is_empty(),
            "{name}: {} crowded edges, the first {:?}",
            crowded.len(),
            &crowded[..crowded.len().min(4)]
        );
        let net = Net::new(&ends, &chords, rules);
        let ring = footprints(&ends, &chords, rules);
        let (at, st) = (ends[0].at, net.frame(0, 0.0));
        let (l, m) = (mesh::left_of(ends[0].way_in()), metres_per_unit(at[1]));
        let across = |p: [f64; 2]| ((p[0] - at[0]) * l[0] + (p[1] - at[1]) * l[1]) * m;
        if name == "closed" {
            // No opening: its first box starts on the frame line, no daylight comes from it, and it has no ring.
            assert_eq!(boxes::boxes(&net, 0)[0][..2], at);
            assert_eq!(net.daylight(st.at([0.0, 2.0])), 0.0);
            assert_eq!(ring.len(), 1);
        } else {
            // The ring's head: from the front's right end to its left end and back, then the back's right end.
            let head = [ring[0][0], ring[0][1], ring[0][2], ring[0][ring[0].len() - 1]];
            let wide = head.iter().map(|&p| across(p).abs()).fold(0.0, f64::max);
            let half = 4.5;
            assert!(wide <= half + frame + shell + 1e-6, "{name}: the ring reaches {wide} m across");
        }
    }
}

/// Arches at both ends of road tubes along polylines of metres east and north at the floor 500 m: an end at the first
/// vertex leads along the first chord, one at the last vertex back along the last.
fn tubes(lines: &[&[[f64; 2]]]) -> (Vec<End>, Vec<Chord>) {
    let (mut ends, mut chords) = (Vec::new(), Vec::new());
    for line in lines {
        let n = line.len();
        let heading = |a: [f64; 2], b: [f64; 2]| libm::atan2(-(b[1] - a[1]), b[0] - a[0]);
        ends.push(end(line[0][0], line[0][1], 500.0, heading(line[0], line[1]), 6.0));
        ends.push(end(line[n - 1][0], line[n - 1][1], 500.0, heading(line[n - 1], line[n - 2]), 6.0));
        let mut along = 0.0;
        for w in line.windows(2) {
            chords.push(chord([w[0][0], w[0][1], 500.0], [w[1][0], w[1][1], 500.0], 5.0, along));
            along += libm::hypot(w[1][0] - w[0][0], w[1][1] - w[0][1]);
        }
    }
    (ends, chords)
}

#[test]
fn two_tubes_meet_only_where_both_are_and_a_cut_head_is_closed() {
    let rules = Rules::embedded();
    let faces_of =
        |(ends, chords): &(Vec<End>, Vec<Chord>)| solid(&meshes([0.0, 0.0, 1.07e9, 1.07e9], ends, chords, rules));
    let north = [0.0, 1.0, 0.0];
    // Staggered openings: a tube 7 m north starts 3 m after the first, so their rooms overlap from there on. Before
    // it, the first tube keeps its wall; after it, a way north meets the second's far wall; a way west in the first's
    // wall, from the second's room, meets the face that closes the first's shell where its cut begins.
    let staggered = tubes(&[&[[0.0, 0.0], [10.0, 0.0], [40.0, 0.0]], &[[3.0, 7.0], [10.0, 7.0], [40.0, 7.0]]]);
    let faces = faces_of(&staggered);
    for east in [0.5, 1.5, 2.5, 2.95] {
        let got = hit(&faces, [east, 0.0, 502.5], north, 30.0);
        assert!(got.is_some_and(|h| h.1 && h.0 < 5.1), "staggered, {east} m east: {got:?}");
    }
    for east in [5.0, 15.0] {
        let got = hit(&faces, [east, 0.0, 502.5], north, 30.0);
        assert!(got.is_some_and(|h| h.1 && h.0 > 11.0), "staggered, {east} m east: {got:?}");
    }
    for up in [1.0, 2.0] {
        let got = hit(&faces, [6.0, 4.7, 500.0 + up], [-1.0, 0.0, 0.0], 30.0);
        assert!(got.is_some_and(|h| h.1 && h.0 < 3.2), "staggered, the first's shell {up} m up: {got:?}");
    }
    // A tube beside the first for 20 m that turns away north: past the turn, the first keeps its wall.
    let away = tubes(&[&[[0.0, 0.0], [100.0, 0.0]], &[[0.0, 7.0], [20.0, 7.0], [50.0, 40.0]]]);
    let faces = faces_of(&away);
    for east in [30.0, 60.0, 90.0] {
        let got = hit(&faces, [east, 0.0, 502.5], north, 30.0);
        assert!(got.is_some_and(|h| h.1 && h.0 < 5.1), "turns away, {east} m east: {got:?}");
    }
    assert!(hit(&faces, [10.0, 0.0, 502.5], north, 30.0).is_some_and(|h| h.1 && h.0 > 11.0), "turns away, 10 m east");
    // A path tube whose axis lies 3 m from a road tube's, in the road's room: the middle lies past the path's axis. A
    // way up from either room meets a vault, and a way north from the path's axis meets its far wall.
    let mut inner = tubes(&[&[[0.0, 0.0], [10.0, 0.0], [40.0, 0.0]], &[[0.0, 3.0], [10.0, 3.0], [40.0, 3.0]]]);
    for e in &mut inner.0[2..] {
        (e.half, e.height) = (0.9, 2.6);
    }
    for c in &mut inner.1[2..] {
        (c.half, c.height) = (0.9, 2.6);
    }
    let faces = faces_of(&inner);
    for east in [2.0, 10.0, 30.0] {
        for y in [-2.0, 0.0, 2.0, 3.0] {
            let p = [east, y, 501.0];
            assert!(hit(&faces, p, [0.0, 0.0, 1.0], 30.0).is_some_and(|h| h.1), "a path in a road's room: up at {p:?}");
        }
        let got = hit(&faces, [east, 3.0, 501.0], north, 30.0);
        assert!(got.is_some_and(|h| h.1 && h.0 < 1.0), "a path in a road's room: north at {east} m: {got:?}");
    }
    // A pair whose tubes run 25 degrees apart from one frame line, their rooms overlapping there: both find one plane,
    // so a way north from the first meets the second's far wall, and a way up anywhere between their axes meets a
    // vault.
    let t = libm::tan(25f64.to_radians());
    let mut angled = tubes(&[
        &[[0.0, 0.0], [10.0, 0.0], [40.0, 0.0]],
        &[[0.0, 7.0], [10.0, 7.0 + 10.0 * t], [40.0, 7.0 + 40.0 * t]],
    ]);
    angled.0.remove(2);
    angled.0[0].part = Part::Pair;
    angled.0[0].mate = Some(Mate { across: 7.0, way: [1, 1], shift: 0.0, collar: 6.0 });
    angled.1.iter_mut().for_each(|c| c.ends = [Some(Part::Pair), Some(Part::Arch)]);
    let faces = faces_of(&angled);
    for east in [1.0, 2.0, 3.0] {
        let got = hit(&faces, [east, 0.0, 502.5], north, 30.0);
        assert!(got.is_some_and(|h| h.1 && h.0 > 11.0), "angled, {east} m east: {got:?}");
        for k in 0..=20 {
            let p = [east, 2.0 + 0.2 * f64::from(k), 500.5];
            assert!(hit(&faces, p, [0.0, 0.0, 1.0], 30.0).is_some_and(|h| h.1), "angled: a way up at {p:?}");
        }
    }
    // Two arches on one frame line, the first's head wall 2 m taller: over the lower wall, a way to the taller one
    // meets the face that closes it at the middle.
    let mut taller = row(&[0.0, 7.0]);
    taller.0[0].face = 9.1;
    let faces = faces_of(&taller);
    for up in [7.6, 8.6] {
        let got = hit(&faces, [0.15, 7.0, 500.0 + up], [0.0, -1.0, 0.0], 30.0);
        assert!(got.is_some_and(|h| h.1 && (h.0 - 3.5).abs() < 0.2), "taller, {up} m up: {got:?}");
    }
    // Two arches whose frame lines lie 3 m apart: the first's tube crosses the second's line, so the second's head
    // ends at the middle, closed over the first's tube; the second does not cross the first's line, so the first's
    // head wall stays whole.
    let faces = faces_of(&staggered);
    let got = hit(&faces, [3.15, 0.0, 506.2], north, 30.0);
    assert!(got.is_some_and(|h| h.1 && (h.0 - 3.5).abs() < 0.2), "frame lines apart, the second's head: {got:?}");
    let got = hit(&faces, [-5.0, 5.0, 506.2], [1.0, 0.0, 0.0], 30.0);
    assert!(got.is_some_and(|h| h.1 && h.0 < 5.1), "frame lines apart, the first's head: {got:?}");
}
