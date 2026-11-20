//! The tubes: the chords of a tile by their vertices, the ring of a chord at each vertex, the inside of a chord, and
//! the light of day in a tube.

use std::collections::HashMap;

use crate::mesh::{flagged, flat, left_of, outline, Builder, Corner, Paint, Station, BORE, LAMP, NO_SLEEPERS, PAINT};
use crate::{built, metres_per_unit, Chord, End, Rules, Section};

/// An end's point lies within this of the vertex of its tube's chord.
const END_M: f64 = 0.5;
/// A lamp's half-width and its gap under the crown.
const LAMP_HALF_M: f64 = 0.15;
const LAMP_DROP_M: f64 = 0.05;

/// The key of a world point: its bits.
fn key(p: [f64; 3]) -> [u64; 2] {
    [p[0].to_bits(), p[1].to_bits()]
}

/// A chord's vertex: 0 is `a`, 1 is `b`.
fn vertex(c: &Chord, k: usize) -> [f64; 3] {
    if k == 0 {
        c.a
    } else {
        c.b
    }
}

/// The unit direction of a chord from a to b, in world units, and its metres.
pub fn direction(c: &Chord) -> ([f64; 2], f64) {
    let d = [c.b[0] - c.a[0], c.b[1] - c.a[1]];
    let l = libm::hypot(d[0], d[1]).max(f64::MIN_POSITIVE);
    ([d[0] / l, d[1] / l], l * metres_per_unit((c.a[1] + c.b[1]) / 2.0))
}

/// The inside of a tube by the group of its class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Use {
    Road,
    Rail,
    Path,
}

/// The chords of a tile by their vertices, and per end the chord vertex at its opening.
pub struct Net<'a> {
    pub ends: &'a [End],
    pub chords: &'a [Chord],
    pub rules: &'a Rules,
    at: HashMap<[u64; 2], Vec<usize>>,
    /// Per end, the chord and vertex at its opening, where its chain is built.
    opening: Vec<Option<(usize, usize)>>,
    /// The end at each opening's chord vertex.
    end_at: HashMap<(usize, usize), usize>,
    /// Per end, the rise of its first chord per metre in.
    slope: Vec<f64>,
}

impl<'a> Net<'a> {
    pub fn new(ends: &'a [End], chords: &'a [Chord], rules: &'a Rules) -> Net<'a> {
        let mut at: HashMap<[u64; 2], Vec<usize>> = HashMap::new();
        for (c, chord) in chords.iter().enumerate() {
            for k in 0..2 {
                at.entry(key(vertex(chord, k))).or_default().push(c);
            }
        }
        let mut opening = vec![None; ends.len()];
        let mut end_at = HashMap::new();
        let mut slope = vec![0.0; ends.len()];
        for (e, end) in ends.iter().enumerate() {
            let m = metres_per_unit(end.at[1]);
            let gap = |c: usize, k: usize| {
                let v = vertex(&chords[c], k);
                libm::hypot(v[0] - end.at[0], v[1] - end.at[1]) * m
            };
            let nearest =
                (0..chords.len()).flat_map(|c| [(c, 0), (c, 1)]).min_by(|x, y| gap(x.0, x.1).total_cmp(&gap(y.0, y.1)));
            if let Some((c, k)) = nearest.filter(|&(c, k)| gap(c, k) <= END_M) {
                if built(&[Some(end.part)]) && built(&chords[c].ends) {
                    opening[e] = Some((c, k));
                    end_at.insert((c, k), e);
                    let (near, far) = (vertex(&chords[c], k), vertex(&chords[c], 1 - k));
                    slope[e] = (far[2] - near[2]) / direction(&chords[c]).1.max(1e-3);
                }
            }
        }
        Net { ends, chords, rules, at, opening, end_at, slope }
    }

    pub fn built_end(&self, e: usize) -> bool {
        self.opening[e].is_some()
    }

    /// Whether vertex k of chord c is the opening of a built end.
    pub fn opening_at(&self, c: usize, k: usize) -> bool {
        self.end_at.contains_key(&(c, k))
    }

    pub fn built_chord(&self, c: usize) -> bool {
        built(&self.chords[c].ends)
    }

    /// The one other chord at vertex k of chord c, if exactly one shares it.
    fn other(&self, c: usize, k: usize) -> Option<usize> {
        let list = &self.at[&key(vertex(&self.chords[c], k))];
        let mut others = list.iter().copied().filter(|&o| o != c);
        let o = others.next()?;
        others.next().is_none().then_some(o)
    }

    /// The straight frame of an end: its station `s` metres in along its heading, at the grade of its first chord,
    /// across to the left of the way in.
    pub fn frame(&self, e: usize, s: f64) -> Station {
        let end = &self.ends[e];
        let (d, m) = (end.way_in(), metres_per_unit(end.at[1]));
        let p = [end.at[0] + d[0] * s / m, end.at[1] + d[1] * s / m];
        let l = left_of(d);
        Station { x: p[0], y: p[1], z: end.z + self.slope[e] * s, ax: l[0] / m, ay: l[1] / m }
    }

    /// The ring of chord c at its vertex k, across to the left of the chord: on the frame line at an opening; in the
    /// plane between the two chords where exactly one other chord shares the vertex, its width grown by the bend up to
    /// `mitre_max`; else square to the chord.
    pub fn ring(&self, c: usize, k: usize) -> Station {
        let chord = &self.chords[c];
        let (d, _) = direction(chord);
        if let Some(&e) = self.end_at.get(&(c, k)) {
            let s = self.frame(e, 0.0);
            let into = self.ends[e].way_in();
            return if d[0] * into[0] + d[1] * into[1] >= 0.0 { s } else { s.turned() };
        }
        let v = vertex(chord, k);
        let Some(o) = self.other(c, k) else { return Station::across([v[0], v[1]], v[2], d, 1.0) };
        let other = &self.chords[o];
        let (od, _) = direction(other);
        // The way through the vertex in the sense of chord c.
        let toward = if key(other.a) == key(v) { od } else { [-od[0], -od[1]] };
        let (din, dout) = if k == 1 { (d, toward) } else { ([-toward[0], -toward[1]], d) };
        let sum = [din[0] + dout[0], din[1] + dout[1]];
        let l = libm::hypot(sum[0], sum[1]);
        if l < 1e-9 {
            return Station::across([v[0], v[1]], v[2], d, 1.0);
        }
        let m = [sum[0] / l, sum[1] / l];
        let cos = (m[0] * din[0] + m[1] * din[1] + (m[0] * dout[0] + m[1] * dout[1])) / 2.0;
        Station::across([v[0], v[1]], v[2], m, 1.0 / cos.max(1.0 / self.rules.design.mitre_max))
    }

    /// The ring of chord c at vertex k across to the left of a walk that runs through c forward or back.
    pub fn ring_along(&self, c: usize, k: usize, forward: bool) -> Station {
        let s = self.ring(c, k);
        if forward {
            s
        } else {
            s.turned()
        }
    }

    /// The walk from end e into its tube: per chord, the chord, whether the walk runs from its a to its b, and the
    /// metres from the opening to the chord's start; as far as `metres`, or to a vertex that not exactly two chords
    /// share.
    pub fn walk(&self, e: usize, metres: f64) -> Vec<(usize, bool, f64)> {
        let mut out = Vec::new();
        let Some((mut c, k)) = self.opening[e] else { return out };
        let mut forward = k == 0;
        let mut s = 0.0;
        while s < metres && out.len() < self.chords.len() {
            out.push((c, forward, s));
            s += direction(&self.chords[c]).1;
            let far = if forward { 1 } else { 0 };
            let Some(o) = self.other(c, far) else { break };
            if out.iter().any(|w| w.0 == o) {
                break;
            }
            forward = key(self.chords[o].a) == key(vertex(&self.chords[c], far));
            c = o;
        }
        out
    }

    /// The share of daylight at a world point: over the openings whose heading line passes within the half-width of
    /// the tube, the shell and a metre beside it, `1 − smoothstep(0, daylight_m, m)` of the metres m in from its frame
    /// line, the largest; 0 with none.
    pub fn daylight(&self, p: [f64; 3]) -> f64 {
        let (shell, reach) = (self.rules.ladder.bake.shell_m, self.rules.design.daylight_m);
        let mut most: f64 = 0.0;
        for end in self.ends {
            let (d, m) = (end.way_in(), metres_per_unit(end.at[1]));
            let l = left_of(d);
            let mates = [Some(0.0), end.mate];
            for q in mates.iter().flatten().map(|a| [end.at[0] + l[0] * a / m, end.at[1] + l[1] * a / m]) {
                let (dx, dy) = (p[0] - q[0], p[1] - q[1]);
                let (along, beside) = ((dx * d[0] + dy * d[1]) * m, (dx * l[0] + dy * l[1]) * m);
                if beside.abs() <= end.half + shell + 1.0 {
                    let t = (along / reach).clamp(0.0, 1.0);
                    most = most.max(1.0 - t * t * (3.0 - 2.0 * t));
                }
            }
        }
        most
    }

    /// The inside of a chord's class: its group's use.
    fn use_of(&self, c: &Chord) -> Use {
        let ladder = &self.rules.ladder;
        match ladder.group(&c.class).map(|g| ladder.groups[g].name.as_str()) {
            Some("rail") => Use::Rail,
            Some("path") => Use::Path,
            _ => Use::Road,
        }
    }
}

/// The spring height of a section of height `h`.
pub fn spring(net: &Net, section: Section, h: f64) -> f64 {
    if section == Section::Box {
        h
    } else {
        net.rules.design.spring * h
    }
}

/// The inside outline of a chord.
fn inside_outline(net: &Net, c: &Chord) -> Vec<Corner> {
    outline(c.section, c.half, c.height, spring(net, c.section, c.height), net.rules.design.arch_segments)
}

/// The direction [across, up] from the middle of an outline's segment to the axis, at the spring height.
pub fn inward(a: Corner, b: Corner, spring: f64) -> Corner {
    [-(a[0] + b[0]), -(a[1] + b[1] - 2.0 * spring)]
}

/// The inside of chord c from its ring at a to its ring at b, seen from inside: the floor, the walls and the vault by
/// its use, its lamps, and at a vertex where a lower chord goes on, the face that closes the step in the crown.
pub fn inside<L: Fn([f64; 3]) -> f64>(b: &mut Builder<L>, net: &Net, c: usize) {
    let chord = &net.chords[c];
    let (design, inside) = (&net.rules.design, &net.rules.inside);
    let (s, t) = (net.ring(c, 0), net.ring(c, 1));
    let (_, length) = direction(chord);
    let use_ = net.use_of(chord);
    let (half, road, lift, kerb) = (chord.half, chord.road, design.lift_m, design.kerb_m);
    let sp = spring(net, chord.section, chord.height);
    let o = inside_outline(net, chord);
    let bore = |colour: u32| flagged(colour, BORE);
    let (wall, vault) = match use_ {
        Use::Road => (inside.tiles, inside.arch_lining),
        Use::Rail => (inside.lining, inside.lining),
        Use::Path => (inside.rock, inside.rock),
    };
    if use_ == Use::Path {
        b.strip(&s, &t, [half, lift], [-half, lift], bore(inside.rock_floor), [0.0, 1.0]);
    } else {
        // The road or the track: in its colour the surface code and the lines; along it, the metres of its stroke or
        // its sleepers.
        let paint = flagged(chord.code | (u32::from(chord.marked) << 8), PAINT | BORE);
        let count = |m: f64| match (use_, chord.along) {
            (_, None) => NO_SLEEPERS,
            (Use::Rail, Some(a)) => (a + m) / design.sleeper_m,
            (_, Some(a)) => a + m,
        };
        let corner = |b: &mut Builder<L>, st: &Station, across: f64, m: f64| {
            b.vertex(st.at([across, lift]), paint, Paint { across, half: road, sleepers: count(m) })
        };
        let v = [
            corner(b, &s, -road, 0.0),
            corner(b, &s, road, 0.0),
            corner(b, &t, road, length),
            corner(b, &t, -road, length),
        ];
        b.quad(v, s.mix(&t, 0.5).out([0.0, 1.0]));
        for side in [1.0, -1.0] {
            b.strip(&s, &t, [side * road, lift], [side * road, kerb], bore(inside.walk), [-side, 0.0]);
            b.strip(&s, &t, [side * road, kerb], [side * half, kerb], bore(inside.walk), [0.0, 1.0]);
        }
        // At an opening, the walks end in a face.
        let (d, _) = direction(chord);
        for (k, r) in [(0, &s), (1, &t)].into_iter().filter(|(k, _)| net.opening_at(c, *k)) {
            let out = flat(if k == 0 { [-d[0], -d[1]] } else { d });
            for side in [1.0, -1.0] {
                b.plane(
                    r,
                    [[side * road, lift], [side * half, lift], [side * half, kerb], [side * road, kerb]],
                    bore(inside.walk),
                    out,
                );
            }
        }
    }
    let n = o.len();
    for j in 0..n - 1 {
        let colour = if j == 0 || j == n - 2 { wall } else { vault };
        b.strip(&s, &t, o[j], o[j + 1], bore(colour), inward(o[j], o[j + 1], sp));
    }
    for k in 0..2 {
        step(b, net, c, k, &o, vault);
    }
    if use_ != Use::Road {
        return;
    }
    // A lamp under the crown where the metres along pass a multiple of the spacing.
    let start = chord.along.unwrap_or(0.0);
    let mut at = (start / design.lamp_spacing_m).ceil() * design.lamp_spacing_m;
    while at < start + length {
        let m = at - start;
        let (p, q) = (((m - design.lamp_m / 2.0) / length).max(0.0), ((m + design.lamp_m / 2.0) / length).min(1.0));
        let (sp, sq) = (s.mix(&t, p), s.mix(&t, q));
        let up = chord.height - LAMP_DROP_M;
        let light = flagged(inside.lamp, LAMP);
        let v = [
            b.at(&sp, [-LAMP_HALF_M, up], light),
            b.at(&sp, [LAMP_HALF_M, up], light),
            b.at(&sq, [LAMP_HALF_M, up], light),
            b.at(&sq, [-LAMP_HALF_M, up], light),
        ];
        b.quad(v, [0.0, 0.0, -1.0]);
        at += design.lamp_spacing_m;
    }
}

/// The face at vertex k of chord c that closes the step in the crown to a lower chord that goes on there, facing
/// into chord c.
fn step<L: Fn([f64; 3]) -> f64>(b: &mut Builder<L>, net: &Net, c: usize, k: usize, o: &[Corner], colour: u32) {
    let chord = &net.chords[c];
    let Some(other) = net.other(c, k).filter(|_| !net.opening_at(c, k)) else { return };
    let lower = &net.chords[other];
    if lower.height >= chord.height || lower.section != chord.section {
        return;
    }
    let r = net.ring(c, k);
    let low = outline(
        chord.section,
        chord.half,
        lower.height,
        spring(net, chord.section, lower.height),
        net.rules.design.arch_segments,
    );
    let (d, _) = direction(chord);
    let into = flat(if k == 0 { d } else { [-d[0], -d[1]] });
    for j in 1..o.len() - 2 {
        b.plane(&r, [o[j], o[j + 1], low[j + 1], low[j]], flagged(colour, BORE), into);
    }
}

/// The outer shell of a run of pieces, each its start and end station and its chord's height and section, with the
/// inside half-width `half`; its walls go down to the world height `foot`. Where two pieces of different height meet,
/// a face closes the step, facing the lower one.
pub fn shell<L: Fn([f64; 3]) -> f64>(
    b: &mut Builder<L>,
    net: &Net,
    pieces: &[(Station, Station, f64, Section)],
    half: f64,
    foot: f64,
    colour: u32,
) {
    let (shell_m, segments) = (net.rules.ladder.bake.shell_m, net.rules.design.arch_segments);
    let outer = |st: &Station, h: f64, section: Section| {
        let sp = spring(net, section, h);
        let mut o = outline(section, half + shell_m, h + shell_m, sp, segments);
        let n = o.len();
        o[0][1] = foot - st.z;
        o[n - 1][1] = foot - st.z;
        (o, sp)
    };
    for (i, (s, t, h, section)) in pieces.iter().enumerate() {
        let ((os, sp), (ot, _)) = (outer(s, *h, *section), outer(t, *h, *section));
        for j in 0..os.len() - 1 {
            let v = [
                b.at(s, os[j], colour),
                b.at(s, os[j + 1], colour),
                b.at(t, ot[j + 1], colour),
                b.at(t, ot[j], colour),
            ];
            let o = inward(os[j], os[j + 1], sp);
            b.quad(v, s.mix(t, 0.5).out([-o[0], -o[1]]));
        }
        let Some((_, _, next, next_section)) = pieces.get(i + 1) else { continue };
        if next == h || next_section != section {
            continue;
        }
        // The step at t: from the taller outline to the lower, facing the lower piece.
        let ((tall, _), (low, _)) = (outer(t, h.max(*next), *section), outer(t, h.min(*next), *section));
        let ahead = s.mix(t, 0.5);
        let d = [t.x - ahead.x, t.y - ahead.y];
        let toward = if next < h { flat(d) } else { flat([-d[0], -d[1]]) };
        for j in 1..tall.len() - 2 {
            b.plane(t, [tall[j], tall[j + 1], low[j + 1], low[j]], colour, toward);
        }
    }
}

#[cfg(test)]
pub(crate) fn outline_of(net: &Net, c: usize) -> Vec<Corner> {
    inside_outline(net, &net.chords[c])
}
