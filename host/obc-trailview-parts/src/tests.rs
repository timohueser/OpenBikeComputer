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
    c.lanes[0].code = 2;
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
        e.part = Part::Box;
        (e.holes[0].kind, e.holes[0].section) = (Part::Box, Section::Box);
    }
    for c in &mut chords {
        (c.section, c.ends, c.lanes[0].class) = (Section::Box, [Some(Part::Box); 2], class.into());
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
    let h = chord.height[0] + (chord.height[1] - chord.height[0]) * share;
    let o = tube::shell_outline(net, (chord.section, tube::segments(net, chord)), chord.half, h, foot - floor);
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
    for e in (0..net.holes.len()).filter(|&e| net.built_end(e)) {
        let list = boxes::boxes(&net, e);
        let small: Vec<[f64; BOX]> = list.iter().map(|b| shrunk(b, MARGIN_M)).collect();
        let held = |boxes: &[[f64; BOX]], p: [f64; 3]| {
            boxes.iter().map(|b| outside(b, p)).fold(f64::INFINITY, f64::min) <= tol
        };
        let reach = boxes::reach(&net, e).boxes;
        let foot = boxes::foot(&net, e);
        let (c0, k0) = net.opening(e).unwrap();
        let (first, high) = (&net.chords[c0], net.chords[c0].height[k0]);
        let sp = tube::spring(&net, first.section, high);
        // The room: the opening's hole from the front of the head wall to the frame line.
        for k in (0..=4).filter(|_| !net.closed(e)) {
            let st = net.frame(e, -design.recess_m * (1.0 - k as f64 / 4.0));
            for a in [-1.0, -0.5, 0.0, 0.5, 1.0] {
                let hw = first.half + design.frame_m;
                for up in [design.lift_m + 0.01, 1.0, sp, high] {
                    let r = if up > sp {
                        libm::sqrt(1.0 - ((up - sp) / (high + design.frame_m - sp)).powi(2))
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
            let (h0, h1) =
                if forward { (chord.height[0], chord.height[1]) } else { (chord.height[1], chord.height[0]) };
            for k in 0..=8 {
                let u = k as f64 / 8.0;
                if start + u * len > reach {
                    continue;
                }
                let st = s.mix(&t, u);
                let high = h0 + (h1 - h0) * u;
                let sp = tube::spring(&net, chord.section, high);
                for a in [-1.0, -0.5, 0.0, 0.5, 1.0] {
                    for up in [design.lift_m + 0.01, 1.0, sp, (sp + high) / 2.0, high] {
                        let r = if up > sp && chord.section == Section::Round {
                            libm::sqrt((1.0 - ((up - sp) / (high - sp)).powi(2)).max(0.0))
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
        let m = metres_per_unit(net.holes[e].at[1]);
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
        (c.a[2], c.b[2], c.height, z) = (z, z + grade * len, [5.0; 2], z + grade * len);
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
    // Rooms of two and three lanes, at one height and rising 1.5 m over their 40 m.
    for rise in [0.0, 1.5 / 40.0] {
        for (name, lanes, line) in rooms().into_iter().filter(|r| r.1.len() > 1) {
            let (ends, chords) = room(&lanes, &line, rise);
            cases.push((format!("{name}, rising {rise}"), ends, chords));
        }
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
    assert!(reach.boxes >= straight[0].holes[0].collar + 1.0 && reach.closed, "{reach:?}");
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
    let built = || (0..net.holes.len()).filter(|&e| net.built_end(e));
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
    rock.iter_mut().for_each(|c| c.lanes[0].class = "footway".into());
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
        (e.part, e.holes[0].kind) = (part, part);
    }
    for c in &mut chords {
        (c.ends, c.lanes[0].class) = ([Some(part); 2], class.into());
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
            ends[0].holes[0].collar = 0.0;
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

/// The corners of solid faces that lie inside an edge of another solid face, within half a world unit, and are no
/// corner of a face of that edge: there a face meets another in the middle of an edge.
fn t_vertices(mesh: &Mesh) -> Vec<[f64; 3]> {
    let words = |i: u32| &mesh.vertices[i as usize * WORDS..(i as usize + 1) * WORDS];
    let shown = |i: u32| (words(i)[3] >> 24) & (mesh::VOID | mesh::LAMP) == 0;
    // A corner in world units, its height too, and its key.
    let point = |i: u32| {
        let w = words(i);
        let y = w[1] as i32 as f64;
        [w[0] as i32 as f64, y, f32::from_bits(w[2]) as f64 / metres_per_unit(y)]
    };
    let key = |i: u32| {
        let w = words(i);
        [w[0], w[1], w[2]]
    };
    // Per edge, the third corners of its faces.
    type Key = [u32; 3];
    let mut edges: BTreeMap<(Key, Key), Vec<Key>> = BTreeMap::new();
    let mut corners: BTreeMap<[u32; 3], [f64; 3]> = BTreeMap::new();
    for t in mesh.indices.chunks(3).filter(|t| shown(t[0])) {
        for k in 0..3 {
            let (a, b, o) = (key(t[k]), key(t[(k + 1) % 3]), key(t[(k + 2) % 3]));
            edges.entry(if a < b { (a, b) } else { (b, a) }).or_default().push(o);
            corners.insert(a, point(t[k]));
        }
    }
    const CELL: f64 = 128.0;
    let cell = |p: [f64; 3]| ((p[0] / CELL).floor() as i64, (p[1] / CELL).floor() as i64);
    let mut grid: BTreeMap<(i64, i64), Vec<[u32; 3]>> = BTreeMap::new();
    for (&k, &p) in &corners {
        grid.entry(cell(p)).or_default().push(k);
    }
    let mut out = Vec::new();
    for ((a, b), thirds) in &edges {
        let (p, q) = (corners[a], corners[b]);
        let d = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
        let l2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        let (lo, hi) = (
            cell([p[0].min(q[0]) - 1.0, p[1].min(q[1]) - 1.0, 0.0]),
            cell([p[0].max(q[0]) + 1.0, p[1].max(q[1]) + 1.0, 0.0]),
        );
        for x in lo.0..=hi.0 {
            for y in lo.1..=hi.1 {
                for k in grid.get(&(x, y)).into_iter().flatten() {
                    if k == a || k == b || thirds.contains(k) {
                        continue;
                    }
                    let v = corners[k];
                    let r = [v[0] - p[0], v[1] - p[1], v[2] - p[2]];
                    let u = (r[0] * d[0] + r[1] * d[1] + r[2] * d[2]) / l2;
                    let off = [r[0] - u * d[0], r[1] - u * d[1], r[2] - u * d[2]];
                    if u > 0.0 && u < 1.0 && libm::sqrt(off[0] * off[0] + off[1] * off[1] + off[2] * off[2]) < 0.5 {
                        out.push(v);
                    }
                }
            }
        }
    }
    out
}

/// A record of a head of `part` at metres east and north, its floor z, its heading in radians of world units, with a
/// hole per (across, up) from the right, each of a round tube 4.5 m wide and 5 m high, and a way per hole.
fn head(east: f64, north: f64, z: f64, heading: f64, holes: &[(f64, f64)], part: Part) -> End {
    let row = |f: &dyn Fn(usize) -> String| (0..holes.len()).map(f).collect::<Vec<_>>().join(" ");
    let mut t = vec![
        ("part", part.name().to_string()),
        ("heading", heading.to_string()),
        ("section", row(&|_| "round".into())),
        ("half", row(&|_| "4.5".into())),
        ("height", row(&|_| "5".into())),
        ("look", "concrete".into()),
        ("collar", row(&|_| "6".into())),
        ("foot", "-2".into()),
        ("way", row(&|_| "0 0".into())),
        ("shift", row(&|_| "0".into())),
    ];
    if !matches!(part, Part::Rock | Part::Free) {
        t.extend([("face", "7.1".into()), ("left", "6 4 2".into()), ("right", "3".into())]);
    }
    if holes.len() > 1 {
        t.push(("across", row(&|h| holes[h].0.to_string())));
        t.push(("up", row(&|h| holes[h].1.to_string())));
        t.push(("kind", row(&|_| part.name().into())));
    }
    End::from_tags(world(east, north), z, &tags(&t), Rules::embedded()).unwrap()
}

/// Road tubes 40 m east, each of two chords on the line `north` metres north with its floor from z rising 0.5 %, and
/// at each end one head of `part` with a hole per tube.
fn side_by_side(lines: &[(f64, f64)], part: Part, class: &str) -> (Vec<End>, Vec<Chord>) {
    let mut chords = Vec::new();
    for &(north, z) in lines {
        for (a, b) in [(0.0, 20.0), (20.0, 40.0)] {
            let mut c = chord([a, north, z + a * 0.005], [b, north, z + b * 0.005], 5.0, a);
            (c.ends, c.lanes[0].class) = ([Some(part); 2], class.into());
            chords.push(c);
        }
    }
    let (n0, z0) = lines[0];
    let west: Vec<(f64, f64)> = lines.iter().map(|&(n, z)| (n - n0, z - z0)).collect();
    let (nl, zl) = lines[lines.len() - 1];
    let east: Vec<(f64, f64)> = lines.iter().rev().map(|&(n, z)| (nl - n, z - zl)).collect();
    let pi = std::f64::consts::PI;
    (vec![head(0.0, n0, z0, 0.0, &west, part), head(40.0, nl, zl + 0.2, pi, &east, part)], chords)
}

#[test]
fn a_line_with_holes_is_closed_and_has_no_t_vertex() {
    let rules = Rules::embedded();
    // Heads of one, two and three road tubes 10.5 m apart (their frames 0.6 m apart), one with the second tube's floor
    // 0.8 m higher; two rims whose outlines overlap.
    let cases = [
        ("one", vec![(0.0, 500.0)], Part::Arch, "primary"),
        ("two", vec![(0.0, 500.0), (10.5, 500.0)], Part::Arch, "primary"),
        ("a step", vec![(0.0, 500.0), (10.5, 500.8)], Part::Arch, "primary"),
        ("three", vec![(0.0, 500.0), (10.5, 500.0), (21.0, 500.0)], Part::Arch, "primary"),
        ("rims", vec![(0.0, 500.0), (10.5, 500.0)], Part::Rock, "footway"),
    ];
    for (name, lines, part, class) in cases {
        let (ends, chords) = side_by_side(&lines, part, class);
        let (room, box_points) = boxes_fit(name, &ends, &chords);
        assert!(room > 200 && box_points > 100, "{name}: {room} room samples, {box_points} box samples");
        let (open, crowded) = not_closed(&ends, &chords);
        assert!(open.is_empty(), "{name}: {} open edges, the first {:?}", open.len(), &open[..open.len().min(4)]);
        assert!(
            crowded.is_empty(),
            "{name}: {} crowded edges, the first {:?}",
            crowded.len(),
            &crowded[..crowded.len().min(4)]
        );
        let mesh = meshes([0.0, 0.0, 1.07e9, 1.07e9], &ends, &chords, rules);
        let t = t_vertices(&mesh);
        let shown: Vec<_> = t.iter().map(|p| (metric([p[0], p[1]]), p[2] * metres_per_unit(p[1]))).collect();
        assert!(t.is_empty(), "{name}: {} corners inside an edge: {shown:?}", t.len());
        let faces = solid(&mesh);
        // A way into each hole meets no face; between two holes of a wall, a way in meets the head wall's front.
        for &(north, z) in &lines {
            let got = hit(&faces, [-3.0, north, z + 2.5], [1.0, 0.0, 0.0], 10.0);
            assert!(got.is_none(), "{name}: the way into the hole at {north} m north: {got:?}");
        }
        for w in lines.windows(2).filter(|_| part == Part::Arch) {
            let got = hit(&faces, [-3.0, (w[0].0 + w[1].0) / 2.0, w[0].1 + 1.0], [1.0, 0.0, 0.0], 10.0);
            assert!(got.is_some_and(|h| h.1 && h.0 < 3.0), "{name}: the head wall between the holes: {got:?}");
        }
    }
}

#[test]
fn a_tube_of_one_lane_draws_as_before_lanes() {
    // The counts of vertices and indices and an FNV-1a digest of all words of the meshes of the one-lane fixtures, as
    // the builder drew them before rooms had lanes.
    let rules = Rules::embedded();
    let (ends, mut rock) = bent(30.0, 20.0);
    rock.iter_mut().for_each(|c| c.lanes[0].class = "footway".into());
    for (name, (ends, chords), want) in [
        ("tube", tube(), (1692, 2760, 0x76ca59c6c0417d99)),
        ("bent", bent(30.0, 20.0), (1796, 2922, 0x32770b2e08b39be2)),
        ("rock", (ends, rock), (1596, 2604, 0x7985ddc9a6419370)),
        ("underpass", underpass(tube(), "primary"), (804, 1308, 0x605ceef7d1cd5375)),
        ("path underpass", underpass(bent(30.0, 20.0), "footway"), (676, 1116, 0x8b983f3aff05a770)),
        ("one chord", one_chord(), (532, 876, 0xe9940653d7761b90)),
        ("short", short(), (1326, 2136, 0xdc35cfb127bf2d81)),
        ("graded", graded(60.0, 0.15), (1556, 2538, 0x775cdd3aa508c8eb)),
    ] {
        let m = meshes([0.0, 0.0, 1.07e9, 1.07e9], &ends, &chords, rules);
        let mut h: u64 = 0xcbf29ce484222325;
        for w in m.vertices.iter().chain(&m.indices) {
            h = (h ^ u64::from(*w)).wrapping_mul(0x100000001b3);
        }
        assert_eq!((m.vertices.len() / WORDS, m.indices.len(), h), want, "{name}");
    }
}

/// A lane of a test room: its class, the half-width of its carriageway and of its own space, and per metre along the
/// room its axis's metres to the left of the room's and its floor over the room's.
struct Way {
    class: &'static str,
    road: f64,
    half: f64,
    off: fn(f64) -> f64,
    up: fn(f64) -> f64,
}

/// A room of `lanes` along a line of metres east and north at the floor 500 m, with a head of one hole at each end and
/// a way per lane into it. Its half-width holds each lane's space; between two lanes the edge lies in the middle of
/// their two spaces' edges; it is 7 m high and `rise` metres higher per metre along the line.
fn room(lanes: &[Way], line: &[[f64; 2]], rise: f64) -> (Vec<End>, Vec<Chord>) {
    let at: Vec<f64> = (line.windows(2)).fold(vec![0.0], |mut s, w| {
        s.push(s[s.len() - 1] + libm::hypot(w[1][0] - w[0][0], w[1][1] - w[0][1]));
        s
    });
    let half = (at.iter()).flat_map(|&m| lanes.iter().map(move |l| (l.off)(m).abs() + l.half)).fold(0.0, f64::max);
    let num = |v: f64| format!("{}", (v * 1000.0).round() / 1000.0);
    let row = |f: &dyn Fn(&Way) -> String| lanes.iter().map(f).collect::<Vec<_>>().join(" ");
    let mut chords = Vec::new();
    for (k, w) in line.windows(2).enumerate() {
        let (m0, m1) = (at[k], at[k + 1]);
        let pair = |f: &dyn Fn(f64) -> f64| format!("{} {}", num(f(m0)), num(f(m1)));
        let high = |m: f64| 7.0 + rise * m;
        let edge = |i: usize, m: f64| {
            let (r, l) = (&lanes[i], &lanes[i + 1]);
            ((r.off)(m) + r.half + (l.off)(m) - l.half) / 2.0
        };
        let edges = (0..lanes.len() - 1).map(|i| format!("{} {}", num(edge(i, m0)), num(edge(i, m1))));
        let mut t = vec![
            ("section", "round".to_string()),
            ("half", num(half)),
            ("height", pair(&high)),
            ("look", "concrete".into()),
            ("class", row(&|l| l.class.into())),
            ("road", row(&|l| num(l.road))),
            ("along", row(&|_| num(m0))),
            ("ends", "arch arch".into()),
            ("off", lanes.iter().map(|l| pair(&l.off)).collect::<Vec<_>>().join(" ")),
            ("up", lanes.iter().map(|l| pair(&l.up)).collect::<Vec<_>>().join(" ")),
        ];
        if lanes.len() > 1 {
            t.push(("edge", edges.collect::<Vec<_>>().join(" ")));
        }
        let (pa, pb) = (world(w[0][0], w[0][1]), world(w[1][0], w[1][1]));
        let mut c =
            Chord::from_tags([pa[0], pa[1], 500.0], [pb[0], pb[1], 500.0], &tags(&t), Rules::embedded()).unwrap();
        c.lanes.iter_mut().for_each(|l| l.code = 2);
        chords.push(c);
    }
    let n = line.len();
    let heading = |a: [f64; 2], b: [f64; 2]| libm::atan2(-(b[1] - a[1]), b[0] - a[0]);
    let end = |p: [f64; 2], q: [f64; 2], m: f64| {
        let t = tags(&[
            ("part", "arch".into()),
            ("heading", heading(p, q).to_string()),
            ("section", "round".into()),
            ("half", num(half)),
            ("height", num(7.0 + rise * m)),
            ("look", "concrete".into()),
            ("collar", "6".into()),
            ("face", "8.1".into()),
            ("left", "6 4 2".into()),
            ("right", "3".into()),
            ("foot", "-2".into()),
            ("way", row(&|_| "0 0".into())),
            ("shift", row(&|_| "0".into())),
        ]);
        End::from_tags(world(p[0], p[1]), 500.0, &t, Rules::embedded()).unwrap()
    };
    (vec![end(line[0], line[1], 0.0), end(line[n - 1], line[n - 2], at[n - 1])], chords)
}

/// A test room: its name, its lanes and its line.
type Room = (&'static str, Vec<Way>, Vec<[f64; 2]>);

/// The test rooms.
fn rooms() -> Vec<Room> {
    let road = |off: fn(f64) -> f64, up: fn(f64) -> f64| Way { class: "primary", road: 3.5, half: 4.2, off, up };
    let path = |off: fn(f64) -> f64, up: fn(f64) -> f64| Way { class: "footway", road: 1.0, half: 1.6, off, up };
    let straight = vec![[0.0, 0.0], [10.0, 0.0], [20.0, 0.0], [30.0, 0.0], [40.0, 0.0]];
    // A bend of 30° at 20 m, in chords of a metre.
    let (c, s) = (libm::cos(30f64.to_radians()), libm::sin(30f64.to_radians()));
    let bent: Vec<[f64; 2]> = (0..=40)
        .map(f64::from)
        .map(|m| if m <= 20.0 { [m, 0.0] } else { [20.0 + (m - 20.0) * c, (m - 20.0) * s] })
        .collect();
    vec![
        ("one", vec![road(|_| 0.0, |_| 0.0)], straight.clone()),
        ("two, level", vec![road(|_| -4.5, |_| 0.0), road(|_| 4.5, |_| 0.0)], straight.clone()),
        ("two, 0.8 m apart", vec![road(|_| -4.5, |_| 0.0), road(|_| 4.5, |_| 0.8)], straight.clone()),
        ("a footway 3 m up", vec![road(|_| -1.5, |_| 0.0), path(|_| 4.6, |_| 3.0)], straight.clone()),
        ("three", vec![road(|_| -7.0, |_| 0.4), road(|_| 1.0, |_| 0.0), path(|_| 6.5, |_| 3.0)], straight.clone()),
        (
            "a lane that drifts 3 m",
            vec![road(|m| -5.0 + 3.0 * m / 40.0, |_| 0.0), road(|_| 4.5, |m| m / 40.0)],
            straight.clone(),
        ),
        ("carriageways that touch", vec![road(|_| -3.5, |_| 0.0), road(|_| 3.5, |_| 0.0)], straight.clone()),
        ("a bend of 30° in 1 m chords", vec![road(|_| -4.5, |_| 0.0), road(|_| 4.5, |m| 0.5 + m / 80.0)], bent),
    ]
}

#[test]
fn a_room_is_closed_exactly() {
    // Each room at one height, and rising 1.5 m over its 40 m: a room of lanes has a height per vertex.
    for (rise, (name, lanes, line)) in
        [0.0, 1.5 / 40.0].into_iter().flat_map(|rise| rooms().into_iter().map(move |r| (rise, r)))
    {
        let (ends, chords) = room(&lanes, &line, rise);
        let (open, crowded) = not_closed(&ends, &chords);
        assert!(
            open.is_empty(),
            "{name}, {rise}: {} open edges, the first {:?}",
            open.len(),
            &open[..open.len().min(4)]
        );
        assert!(
            crowded.is_empty(),
            "{name}, {rise}: {} crowded edges, the first {:?}",
            crowded.len(),
            &crowded[..crowded.len().min(4)]
        );
    }
}

#[test]
fn from_each_lane_a_way_up_meets_the_vault_and_a_way_across_the_far_wall() {
    let rules = Rules::embedded();
    for (name, lanes, line) in rooms() {
        let (ends, chords) = room(&lanes, &line, 0.0);
        let net = Net::new(&ends, &chords, rules);
        let faces = solid(&meshes([0.0, 0.0, 1.07e9, 1.07e9], &ends, &chords, rules));
        let p3 = |p: [f64; 3]| {
            let q = metric([p[0], p[1]]);
            [q[0], q[1], p[2]]
        };
        for (c, chord) in chords.iter().enumerate().skip(1).step_by(3).take(chords.len().saturating_sub(2)) {
            let st = net.ring(c, 0).mix(&net.ring(c, 1), 0.5);
            let lane = |i: usize| {
                let l = &chord.lanes[i];
                ((l.off[0] + l.off[1]) / 2.0, (l.up[0] + l.up[1]) / 2.0)
            };
            for i in 0..chord.lanes.len() {
                let (off, up) = lane(i);
                let o = p3(st.at([off, up + 1.0]));
                let got = hit(&faces, o, [0.0, 0.0, 1.0], 20.0);
                assert!(got.is_some_and(|h| h.1), "{name}, chord {c}, lane {i}: up {got:?}");
                let got = hit(&faces, o, [0.0, 0.0, -1.0], 20.0);
                assert!(got.is_some_and(|h| h.1 && h.0 < 1.0), "{name}, chord {c}, lane {i}: down {got:?}");
                // Across, over both floors, past the next lane's axis to the far wall.
                if i + 1 < chord.lanes.len() {
                    let (next, up_next) = lane(i + 1);
                    let from = p3(st.at([off, up.max(up_next) + 1.5]));
                    let far = p3(st.at([next, up.max(up_next) + 1.5]));
                    let d = [far[0] - from[0], far[1] - from[1], 0.0];
                    let l = libm::hypot(d[0], d[1]);
                    let got = hit(&faces, from, [d[0] / l, d[1] / l, 0.0], 40.0);
                    assert!(got.is_some_and(|h| h.1 && h.0 > l), "{name}, chord {c}, lane {i}: across {got:?}");
                }
            }
        }
    }
}

/// A lane before a seam: its metres to the left of the room's axis, its floor over the room's, its class, its half-width
/// and the half-width of its carriageway.
type Before = (f64, f64, &'static str, f64, f64);

/// A room 20 m east of `lanes` from a head at the origin to a seam, as wide as `half` and `high`, which goes on as a
/// tube per lane, 20 m long, turned `turn` degrees away from the room's axis to each side, to a covered free end.
fn seamed(lanes: &[Before], (half, high): (f64, f64), turn: f64) -> (Vec<End>, Vec<Chord>) {
    let rules = Rules::embedded();
    let num = |v: f64| format!("{}", (v * 1000.0).round() / 1000.0);
    let row = |f: &dyn Fn(&Before) -> String| lanes.iter().map(f).collect::<Vec<_>>().join(" ");
    let pair = |f: &dyn Fn(&Before) -> f64| {
        lanes.iter().map(|l| format!("{} {}", num(f(l)), num(f(l)))).collect::<Vec<_>>().join(" ")
    };
    let mut chords = Vec::new();
    for (a, b) in [(0.0, 10.0), (10.0, 20.0)] {
        let mut t = vec![
            ("section", "round".to_string()),
            ("half", num(half)),
            ("height", num(high)),
            ("look", "concrete".into()),
            ("class", row(&|l| l.2.into())),
            ("road", row(&|l| num(l.4))),
            ("along", row(&|_| num(a))),
            ("ends", "arch seam".into()),
            ("off", pair(&|l| l.0)),
            ("up", pair(&|l| l.1)),
        ];
        if lanes.len() > 1 {
            let edge = |w: &[Before]| (w[0].0 + w[0].3 + w[1].0 - w[1].3) / 2.0;
            t.push((
                "edge",
                lanes.windows(2).map(|w| format!("{} {}", num(edge(w)), num(edge(w)))).collect::<Vec<_>>().join(" "),
            ));
        }
        let (pa, pb) = (world(a, 0.0), world(b, 0.0));
        let mut c = Chord::from_tags([pa[0], pa[1], 500.0], [pb[0], pb[1], 500.0], &tags(&t), rules).unwrap();
        c.lanes.iter_mut().for_each(|l| l.code = 2);
        chords.push(c);
    }
    let head = |p: [f64; 2], z: f64, heading: f64, (half, ways): (f64, usize)| {
        let t = tags(&[
            ("part", "arch".into()),
            ("heading", heading.to_string()),
            ("section", "round".into()),
            ("half", num(half)),
            ("height", if ways > 1 { num(high) } else { "5".into() }),
            ("look", "concrete".into()),
            ("collar", "6".into()),
            ("face", "8.1".into()),
            ("left", "6 4 2".into()),
            ("right", "3".into()),
            ("foot", "-2".into()),
            ("way", vec!["0 0"; ways].join(" ")),
            ("shift", vec!["0"; ways].join(" ")),
        ]);
        End::from_tags(world(p[0], p[1]), z, &t, rules).unwrap()
    };
    let mut ends = vec![head([0.0, 0.0], 500.0, 0.0, (half, lanes.len()))];
    // The seam: the room, then per lane its tube, from the right; a lane's tube is its half over the cosine of its turn
    // wide along the line.
    let n = lanes.len();
    let turns: Vec<f64> =
        (0..n).map(|i| if n == 1 { 0.0 } else { turn * (2.0 * i as f64 / (n - 1) as f64 - 1.0) }).collect();
    let mut t = vec![
        ("part", "seam".to_string()),
        ("heading", "0".into()),
        (
            "half",
            [num(half)]
                .into_iter()
                .chain(lanes.iter().zip(&turns).map(|(l, a)| num(l.3 / a.to_radians().cos())))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        ("height", [num(high)].into_iter().chain(lanes.iter().map(|_| "5".to_string())).collect::<Vec<_>>().join(" ")),
        ("across", ["0".to_string()].into_iter().chain(lanes.iter().map(|l| num(l.0))).collect::<Vec<_>>().join(" ")),
        ("up", ["0".to_string()].into_iter().chain(lanes.iter().map(|l| num(l.1))).collect::<Vec<_>>().join(" ")),
    ];
    t.retain(|_| true);
    ends.push(End::from_tags(world(20.0, 0.0), 500.0, &tags(&t), rules).unwrap());
    for (l, &a) in lanes.iter().zip(&turns) {
        let (c, s) = (libm::cos(a.to_radians()), libm::sin(a.to_radians()));
        let points = [0.0, 10.0, 20.0].map(|m| [20.0 + m * c, l.0 + m * s]);
        let z = 500.0 + l.1;
        for w in points.windows(2) {
            let t = tags(&[
                ("section", "round".into()),
                ("half", num(l.3)),
                ("height", "5".into()),
                ("look", "concrete".into()),
                ("class", l.2.into()),
                ("road", num(l.4)),
                ("along", "0".into()),
                ("ends", "seam free".into()),
            ]);
            let (pa, pb) = (world(w[0][0], w[0][1]), world(w[1][0], w[1][1]));
            let mut chord = Chord::from_tags([pa[0], pa[1], z], [pb[0], pb[1], z], &t, rules).unwrap();
            chord.lanes[0].code = 2;
            chords.push(chord);
        }
        let heading = libm::atan2(-(points[1][1] - points[2][1]), points[1][0] - points[2][0]);
        let mut end = head(points[2], z, heading, (l.3, 1));
        (end.part, end.holes[0].kind, end.holes[0].collar) = (Part::Free, Part::Free, 0.0);
        ends.push(end);
    }
    (ends, chords)
}

#[test]
fn a_seam_is_closed_and_has_no_t_vertex() {
    let rules = Rules::embedded();
    let road = |off: f64, up: f64| (off, up, "primary", 4.2, 3.5);
    let path = |off: f64, up: f64| (off, up, "footway", 1.6, 1.0);
    let cases: [(&str, Vec<Before>, _, f64); 6] = [
        ("one, in a wider room", vec![road(0.0, 0.0)], (7.0, 7.0), 0.0),
        ("two", vec![road(-5.2, 0.0), road(5.2, 0.0)], (9.4, 7.0), 0.0),
        ("two, turned 20°", vec![road(-5.2, 0.0), road(5.2, 0.0)], (9.7, 7.0), 20.0),
        ("a step", vec![road(-5.2, 0.0), road(5.2, 0.8)], (9.4, 7.0), 0.0),
        ("three", vec![road(-7.0, 0.0), road(3.0, 0.0), path(10.5, 3.0)], (12.5, 11.0), 0.0),
        ("three, turned 20°", vec![road(-7.0, 0.0), road(3.0, 0.4), path(10.5, 3.0)], (12.5, 11.0), 20.0),
    ];
    for (name, lanes, size, turn) in cases {
        let (ends, chords) = seamed(&lanes, size, turn);
        let (open, crowded) = not_closed(&ends, &chords);
        assert!(open.is_empty(), "{name}: {} open edges, the first {:?}", open.len(), &open[..open.len().min(4)]);
        assert!(
            crowded.is_empty(),
            "{name}: {} crowded edges, the first {:?}",
            crowded.len(),
            &crowded[..crowded.len().min(4)]
        );
        let mesh = meshes([0.0, 0.0, 1.07e9, 1.07e9], &ends, &chords, rules);
        let t = t_vertices(&mesh);
        let shown: Vec<_> = t.iter().map(|p| (metric([p[0], p[1]]), p[2] * metres_per_unit(p[1]))).collect();
        assert!(t.is_empty(), "{name}: {} corners inside an edge: {shown:?}", t.len());
        // From the room along each lane's axis into its tube behind the seam: no face.
        let faces = solid(&mesh);
        for l in &lanes {
            let got = hit(&faces, [16.0, l.0, 500.0 + l.1 + 1.5], [1.0, 0.0, 0.0], 6.0);
            assert!(got.is_none(), "{name}: the way from the lane at {} m into its tube: {got:?}", l.0);
        }
    }
}

/// A road tube of lone chords along `line` (metres east and north) at the floor 500 m, `height` high, with its `ends`.
fn lone(line: &[[f64; 2]], height: f64, ends: &str) -> Vec<Chord> {
    let along =
        |k: usize| line[..=k].windows(2).map(|w| libm::hypot(w[1][0] - w[0][0], w[1][1] - w[0][1])).sum::<f64>();
    (0..line.len() - 1)
        .map(|k| {
            let (a, b) = (line[k], line[k + 1]);
            let mut c = chord([a[0], a[1], 500.0], [b[0], b[1], 500.0], height, along(k));
            c.ends = [Part::parse(&ends[..4]), Part::parse(&ends[5..])];
            c
        })
        .collect()
}

/// A seam record at metres east and north heading east, with a hole per (across, half, height).
fn seam_at(east: f64, north: f64, holes: &[(f64, f64, f64)]) -> End {
    let row = |f: &dyn Fn(&(f64, f64, f64)) -> String| holes.iter().map(f).collect::<Vec<_>>().join(" ");
    let t = tags(&[
        ("part", "seam".into()),
        ("heading", "0".into()),
        ("half", row(&|h| h.1.to_string())),
        ("height", row(&|h| h.2.to_string())),
        ("across", row(&|h| h.0.to_string())),
        ("up", row(&|_| "0".into())),
    ]);
    End::from_tags(world(east, north), 500.0, &t, Rules::embedded()).unwrap()
}

#[test]
fn a_cap_a_door_and_a_hole_narrower_than_its_tube_are_closed() {
    let rules = Rules::embedded();
    let line = [[0.0, 0.0], [10.0, 0.0], [20.0, 0.0]];
    // A tube 20 m east from a head to a cap: the room ends in the hill.
    let cap =
        (vec![end(0.0, 0.0, 500.0, 0.0, 6.0), seam_at(20.0, 0.0, &[(0.0, 4.5, 5.0)])], lone(&line, 5.0, "arch seam"));
    // A tube 5.5 m high that goes on behind a seam from a hole 0.3 m to the left of its axis and 0.5 m narrower than
    // it, 5 m high, to its axis 2 m on, and on to a covered free end.
    let mut ends = vec![end(0.0, 0.0, 500.0, 0.0, 6.0), seam_at(20.0, 0.0, &[(0.0, 4.5, 5.5), (0.3, 4.0, 5.0)])];
    let mut free = end(40.0, 0.0, 500.0, std::f64::consts::PI, 0.0);
    (free.part, free.holes[0].kind) = (Part::Free, Part::Free);
    ends.push(free);
    let mut chords = lone(&line, 5.5, "arch seam");
    chords.extend(lone(&[[20.0, 0.3], [22.0, 0.0], [40.0, 0.0]], 5.0, "seam free"));
    // A room of a road and a footway 1 m up at its side, whose footway ends on the seam's line with a door: the road
    // goes on alone through its hole.
    let road = (-1.0, 0.0, "primary", 4.2, 3.5);
    let (mut door_ends, mut door_chords) = seamed(&[road, (4.6, 1.0, "footway", 1.6, 1.0)], (6.2, 7.0), 0.0);
    door_ends.truncate(3);
    door_chords.truncate(4);
    let t = [("half", "6.2 3.9"), ("height", "7 5"), ("across", "0 -1.3"), ("up", "0 0"), ("door", "4.9 1.3 1 2.5")];
    let mut tags = door_ends[1].clone();
    let mut record: Tags =
        [("part", "seam"), ("heading", "0")].iter().chain(&t).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    record.retain(|_, _| true);
    tags = End::from_tags(tags.at, tags.z, &record, rules).unwrap();
    door_ends[1] = tags;
    for (name, (ends, chords)) in
        [("a cap", cap), ("a tapered hole", (ends, chords)), ("a door", (door_ends, door_chords))]
    {
        let (open, crowded) = not_closed(&ends, &chords);
        assert!(open.is_empty(), "{name}: {} open edges, the first {:?}", open.len(), &open[..open.len().min(4)]);
        assert!(
            crowded.is_empty(),
            "{name}: {} crowded edges, the first {:?}",
            crowded.len(),
            &crowded[..crowded.len().min(4)]
        );
        let mesh = meshes([0.0, 0.0, 1.07e9, 1.07e9], &ends, &chords, rules);
        let t = t_vertices(&mesh);
        assert!(t.is_empty(), "{name}: {} corners inside an edge", t.len());
        let net = Net::new(&ends, &chords, rules);
        assert!((0..ends.len()).all(|r| net.built_record(r)), "{name}: a record the builder does not find");
    }
}
