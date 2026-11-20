//! The tunnel parts that TrailView draws, made from the part records of a feature tile (`tunnel-end`,
//! `tunnel-tube`): their meshes in the vertex layout of the viewer's structure meshes, the boxes in which the ground
//! does not draw, the floor segments of the walking camera and the ring that each part takes from the ground. A pure
//! function: no I/O and no threads, the same maths natively and in WASM.
//!
//! Points are world units of the viewer: 2^30 per side of the Web Mercator square, y down.

mod arch;
mod boxes;
mod footprint;
mod mesh;
mod rules;
#[cfg(test)]
mod tests;
mod tube;

use std::collections::BTreeMap;

pub use boxes::{outside, outside_plan, outside_wall, BOX};
pub use mesh::WORDS;
pub use rules::{Bake, Bore, Design, Group, Inside, Ladder, Look, Rules};

use tube::Net;

/// A kind of part at a tunnel end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Arch,
    Pair,
    Rock,
    Box,
    Free,
}

impl Part {
    pub fn name(self) -> &'static str {
        match self {
            Part::Arch => "arch",
            Part::Pair => "pair",
            Part::Rock => "rock",
            Part::Box => "box",
            Part::Free => "free",
        }
    }

    /// The part of a name; None for `none` and for an unknown name.
    pub fn parse(name: &str) -> Option<Part> {
        [Part::Arch, Part::Pair, Part::Rock, Part::Box, Part::Free].into_iter().find(|p| p.name() == name)
    }
}

/// The kinds of part that the builder makes. A chain draws as parts only where both of its ends are of these kinds.
pub const BUILT: &[Part] = &[Part::Arch];

/// Whether all ends of a chain are built; `None` is an end with no part of its own.
pub fn built(ends: &[Option<Part>]) -> bool {
    ends.iter().all(|e| e.is_some_and(|p| BUILT.contains(&p)))
}

/// The cross-section of a tube.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Round,
    Box,
}

/// Metres per world unit at world row y, on the sphere of the viewer's frame (`metresPerUnit` of `bins.ts`).
pub fn metres_per_unit(y: f64) -> f64 {
    const UNITS: f64 = 1073741824.0;
    2.0 * std::f64::consts::PI * 6_371_000.0 / UNITS / libm::cosh(std::f64::consts::PI * (1.0 - 2.0 * y / UNITS))
}

/// The tags of a record.
pub type Tags = BTreeMap<String, String>;

fn tag<'a>(tags: &'a Tags, name: &str) -> Result<&'a str, String> {
    tags.get(name).map(String::as_str).ok_or(format!("no tag {name}"))
}

fn number(tags: &Tags, name: &str) -> Result<f64, String> {
    let v = tag(tags, name)?;
    v.parse::<f64>().ok().filter(|v| v.is_finite()).ok_or(format!("tag {name}: not a number: {v}"))
}

fn row(tags: &Tags, name: &str) -> Result<Vec<f64>, String> {
    let v = tag(tags, name)?;
    v.split(' ')
        .map(|x| x.parse::<f64>().ok().filter(|v| v.is_finite()))
        .collect::<Option<_>>()
        .ok_or(format!("tag {name}: {v}"))
}

fn vertex(tags: &Tags, name: &str) -> Result<[i64; 2], String> {
    let v = tag(tags, name)?;
    let xy: Vec<i64> = v.split(' ').filter_map(|x| x.parse().ok()).collect();
    xy.try_into().map_err(|_| format!("tag {name}: not x y: {v}"))
}

fn section(tags: &Tags) -> Result<Section, String> {
    match tag(tags, "section")? {
        "round" => Ok(Section::Round),
        "box" => Ok(Section::Box),
        other => Err(format!("tag section: {other}")),
    }
}

/// A `tunnel-end` record (`specs/trailview-features.md`, Tunnel parts): the opening's point and floor, and its numbers in
/// metres over that floor.
#[derive(Clone, Debug, PartialEq)]
pub struct End {
    pub at: [f64; 2],
    pub z: f64,
    pub part: Part,
    /// Radians from x to y of world units, into the tube.
    pub heading: f64,
    pub section: Section,
    pub half: f64,
    pub height: f64,
    pub look: String,
    pub collar: f64,
    /// `face`, `left` and `right` are 0 and empty on a free end.
    pub face: f64,
    pub left: Vec<f64>,
    pub right: Vec<f64>,
    pub foot: f64,
    /// The metres to the left from its axis to the other tube of a pair.
    pub mate: Option<f64>,
    pub way: [i64; 2],
    pub shift: f64,
    pub mate_way: Option<[i64; 2]>,
    pub mate_shift: Option<f64>,
}

impl End {
    /// The end of a record; its `look` is one of `rules`.
    pub fn from_tags(at: [f64; 2], z: f64, tags: &Tags, rules: &Rules) -> Result<End, String> {
        let name = tag(tags, "part")?;
        let part = Part::parse(name).ok_or(format!("tag part: {name}"))?;
        let look = tag(tags, "look")?;
        if !rules.looks.contains_key(look) {
            return Err(format!("tag look: no look {look}"));
        }
        let walls = part != Part::Free;
        let pair = part == Part::Pair;
        Ok(End {
            at,
            z,
            part,
            heading: number(tags, "heading")?,
            section: section(tags)?,
            half: number(tags, "half")?,
            height: number(tags, "height")?,
            look: look.to_string(),
            collar: number(tags, "collar")?,
            face: if walls { number(tags, "face")? } else { 0.0 },
            left: if walls { row(tags, "left")? } else { Vec::new() },
            right: if walls { row(tags, "right")? } else { Vec::new() },
            foot: number(tags, "foot")?,
            mate: pair.then(|| number(tags, "mate")).transpose()?,
            way: vertex(tags, "way")?,
            shift: number(tags, "shift")?,
            mate_way: pair.then(|| vertex(tags, "mate_way")).transpose()?,
            mate_shift: pair.then(|| number(tags, "mate_shift")).transpose()?,
        })
    }

    /// The unit direction into the tube, in world units.
    pub fn way_in(&self) -> [f64; 2] {
        [libm::cos(self.heading), libm::sin(self.heading)]
    }
}

/// A `tunnel-tube` record: one chord of a tube from `a` to `b` (world point and floor), in the rising metres of its
/// way, with the surface code and the lines of its road as the viewer gives them (`code`, `marked`).
#[derive(Clone, Debug, PartialEq)]
pub struct Chord {
    pub a: [f64; 3],
    pub b: [f64; 3],
    pub section: Section,
    pub half: f64,
    pub height: f64,
    pub look: String,
    pub class: String,
    /// The half-width of the carriageway.
    pub road: f64,
    /// The metres along the stroke of its way at `a`.
    pub along: f64,
    /// The parts of the chain's two ends; None for an end with no part of its own.
    pub ends: [Option<Part>; 2],
    pub code: u32,
    pub marked: bool,
}

impl Chord {
    /// The chord of a record, whose `class` is in a group of `rules`; `code` and `marked` are the caller's.
    pub fn from_tags(a: [f64; 3], b: [f64; 3], tags: &Tags, rules: &Rules) -> Result<Chord, String> {
        let class = tag(tags, "class")?;
        if rules.ladder.group(class).is_none() {
            return Err(format!("tag class: {class} is in no group"));
        }
        let ends: Vec<&str> = tag(tags, "ends")?.split(' ').collect();
        let [e0, e1] = ends[..] else { return Err(format!("tag ends: {}", ends.join(" "))) };
        Ok(Chord {
            a,
            b,
            section: section(tags)?,
            half: number(tags, "half")?,
            height: number(tags, "height")?,
            look: tag(tags, "look")?.to_string(),
            class: class.to_string(),
            road: number(tags, "road")?,
            along: number(tags, "along")?,
            ends: [Part::parse(e0), Part::parse(e1)],
            code: 0,
            marked: false,
        })
    }
}

/// A mesh in the layout of the viewer's structure meshes: per vertex 7 words (`mesh::WORDS`), each word's bits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<u32>,
    pub indices: Vec<u32>,
}

/// The volumes near a square: the boxes in which the ground does not draw (`BOX` words each) and the floor segments
/// (`FLOOR`, 7 words each).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Volumes {
    pub boxes: Vec<f64>,
    pub floors: Vec<f64>,
}

/// Words of a floor segment.
pub const FLOOR: usize = 7;

/// Whether a square `[x0, y0, x1, y1]` holds a point, half-open.
fn holds(square: [f64; 4], p: [f64; 2]) -> bool {
    p[0] >= square[0] && p[0] < square[2] && p[1] >= square[1] && p[1] < square[3]
}

/// Whether the box of points comes within `reach` world units of a square.
fn near(square: [f64; 4], points: &[[f64; 2]], reach: f64) -> bool {
    let (lo, hi) = points.iter().fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(lo, hi), p| {
        ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])])
    });
    hi[0] >= square[0] - reach && lo[0] <= square[2] + reach && hi[1] >= square[1] - reach && lo[1] <= square[3] + reach
}

/// The faces of the parts that a square draws: of each built end whose point it holds, and of each chord of a built
/// chain whose `a` it holds, so each face draws in one square.
pub fn meshes(square: [f64; 4], ends: &[End], chords: &[Chord], rules: &Rules) -> Mesh {
    let net = Net::new(ends, chords, rules);
    let mut b = mesh::Builder::new(|p| net.daylight(p));
    for (e, end) in ends.iter().enumerate() {
        if net.built_end(e) && holds(square, end.at) {
            arch::faces(&mut b, &net, e);
        }
    }
    for (c, chord) in chords.iter().enumerate() {
        if net.built_chord(c) && holds(square, [chord.a[0], chord.a[1]]) {
            tube::inside(&mut b, &net, c);
        }
    }
    b.mesh()
}

/// The boxes of the built ends that come within `reach_m` of a square, and the floor segments of the chords of built
/// chains within `reach_m`.
pub fn volumes(square: [f64; 4], ends: &[End], chords: &[Chord], rules: &Rules) -> Volumes {
    let net = Net::new(ends, chords, rules);
    let reach = rules.design.reach_m / metres_per_unit((square[1] + square[3]) / 2.0);
    let mut out = Volumes::default();
    for e in (0..ends.len()).filter(|&e| net.built_end(e)) {
        for words in boxes::boxes(&net, e) {
            let m = metres_per_unit(words[1]);
            let (into, left) = ([words[2], words[3]], mesh::left_of([words[2], words[3]]));
            let corner = |s: f64, a: f64| {
                [words[0] + (into[0] * s + left[0] * a) / m, words[1] + (into[1] * s + left[1] * a) / m]
            };
            let (depth, half, [start, end]) = (words[4], words[5], [words[10], words[11]]);
            let corners = [
                corner(start * half, half),
                corner(-start * half, -half),
                corner(depth + end * half, half),
                corner(depth - end * half, -half),
            ];
            if near(square, &corners, reach) {
                out.boxes.extend(words);
            }
        }
    }
    for (c, chord) in chords.iter().enumerate() {
        let (a, b) = ([chord.a[0], chord.a[1]], [chord.b[0], chord.b[1]]);
        if net.built_chord(c) && near(square, &[a, b], reach) {
            let lift = rules.design.lift_m;
            out.floors.extend([a[0], a[1], b[0], b[1], chord.a[2] + lift, chord.b[2] + lift, chord.half]);
        }
    }
    out
}

/// Per end, the boxes of its part (`BOX` words each, the opening's first); none for an end that is not built.
pub fn end_boxes(ends: &[End], chords: &[Chord], rules: &Rules) -> Vec<Vec<[f64; BOX]>> {
    let net = Net::new(ends, chords, rules);
    (0..ends.len()).map(|e| boxes::boxes(&net, e)).collect()
}

/// Per built end, the ring that its part takes from the ground: round its head wall and wings, from front to back,
/// and its collar.
pub fn footprints(ends: &[End], chords: &[Chord], rules: &Rules) -> Vec<Vec<[f64; 2]>> {
    let net = Net::new(ends, chords, rules);
    (0..ends.len()).filter(|&e| net.built_end(e)).map(|e| footprint::ring(&net, e)).collect()
}
