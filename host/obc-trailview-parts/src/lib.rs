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
mod seam;
#[cfg(test)]
mod tests;
mod tube;

use std::collections::BTreeMap;

pub use boxes::{corners, need, outside, outside_plan, outside_wall, BOX};
pub use mesh::{BORE, LAMP, VOID, WORDS};
pub use rules::{Bake, Bore, Design, Group, Inside, Ladder, Look, Rules};
pub use tube::PAST_M;

use tube::Net;

/// A kind of part at the end of a room: at an opening, or the seam where it goes on as other rooms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Arch,
    Rock,
    Box,
    Free,
    Seam,
}

impl Part {
    pub fn name(self) -> &'static str {
        match self {
            Part::Arch => "arch",
            Part::Rock => "rock",
            Part::Box => "box",
            Part::Free => "free",
            Part::Seam => "seam",
        }
    }

    /// The part of a name; None for `none` and for an unknown name.
    pub fn parse(name: &str) -> Option<Part> {
        [Part::Arch, Part::Rock, Part::Box, Part::Free, Part::Seam].into_iter().find(|p| p.name() == name)
    }
}

/// Whether all ends of a chain are parts, which the builder builds; `None` is an end with no part of its own. A room
/// draws as parts only where both of its ends are.
pub fn built(ends: &[Option<Part>]) -> bool {
    ends.iter().all(Option::is_some)
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

/// A row of `n` values; with `alone`, a missing tag is that value.
fn row_of(tags: &Tags, name: &str, n: usize, alone: Option<f64>) -> Result<Vec<f64>, String> {
    let list = match (tags.contains_key(name), alone) {
        (false, Some(v)) => vec![v; n],
        _ => row(tags, name)?,
    };
    if list.len() != n {
        return Err(format!("tag {name}: not {n} values"));
    }
    Ok(list)
}

fn section_of(name: &str) -> Result<Section, String> {
    match name {
        "round" => Ok(Section::Round),
        "box" => Ok(Section::Box),
        other => Err(format!("tag section: {other}")),
    }
}

fn section(tags: &Tags) -> Result<Section, String> {
    section_of(tag(tags, "section")?)
}

/// A `tunnel-end` record (`specs/trailview-features.md`, Tunnel parts): a line with holes, its point on the axis of
/// its first hole and the floor there, and its numbers in metres over that floor. A seam's first hole is the room on
/// its side of the line, whose section holds the others; its heading leads from that room into them. A seam of one
/// hole is a cap: the room ends there in the hill. A seam's door is a lane of the room before it that ends on its line.
#[derive(Clone, Debug, PartialEq)]
pub struct End {
    pub at: [f64; 2],
    pub z: f64,
    pub part: Part,
    /// Radians from x to y of world units, into the tube.
    pub heading: f64,
    pub look: String,
    /// From the right to the left of the way in.
    pub holes: Vec<Hole>,
    /// Whether the record holds `face`, `left` and `right`. Else they are 0 and empty.
    pub walls: bool,
    pub face: f64,
    pub left: Vec<f64>,
    pub right: Vec<f64>,
    pub foot: f64,
    /// The ways that enter the head: per hole from the right, per lane of its tube from the right.
    pub ways: Vec<Way>,
    /// Of a seam, where a lane of the room before the line ends on it.
    pub doors: Vec<Door>,
}

/// Where a lane of the room before a seam ends on its line: the metres to the left of the record's point to its middle,
/// its half-width, and its floor and its height over the record's floor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Door {
    pub across: f64,
    pub half: f64,
    pub up: f64,
    pub height: f64,
}

/// A hole of a line: the metres to the left of the record's point to its axis and its floor over the record's, its
/// kind, its section and its collar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hole {
    pub across: f64,
    pub up: f64,
    pub kind: Part,
    pub section: Section,
    pub half: f64,
    pub height: f64,
    pub collar: f64,
}

/// A way that enters a head: the end vertex of the way in the frame of the ground plan, and the metres along the way
/// from it to the opening.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Way {
    pub way: [i64; 2],
    pub shift: f64,
}

impl End {
    /// The end of a record; its `look` is one of `rules`. A seam holds `heading`, and per hole `half`, `height`,
    /// `across` and `up` only.
    pub fn from_tags(at: [f64; 2], z: f64, tags: &Tags, rules: &Rules) -> Result<End, String> {
        let name = tag(tags, "part")?;
        let part = Part::parse(name).ok_or(format!("tag part: {name}"))?;
        if part == Part::Seam {
            let (half, height) = (row(tags, "half")?, row(tags, "height")?);
            let n = half.len();
            let (across, up) = (row_of(tags, "across", n, None)?, row_of(tags, "up", n, None)?);
            if height.len() != n || n == 0 {
                return Err(format!("tags half, height: not {n} values"));
            }
            let holes = (0..n)
                .map(|h| Hole {
                    across: across[h],
                    up: up[h],
                    kind: part,
                    section: Section::Round,
                    half: half[h],
                    height: height[h],
                    collar: 0.0,
                })
                .collect();
            let (left, right, ways) = (Vec::new(), Vec::new(), Vec::new());
            let heading = number(tags, "heading")?;
            let door = if tags.contains_key("door") { row(tags, "door")? } else { Vec::new() };
            if door.len() % 4 != 0 {
                return Err("tag door: not four values per door".into());
            }
            let doors = door.chunks(4).map(|d| Door { across: d[0], half: d[1], up: d[2], height: d[3] }).collect();
            return Ok(End {
                at,
                z,
                part,
                heading,
                look: String::new(),
                holes,
                walls: false,
                face: 0.0,
                left,
                right,
                foot: 0.0,
                ways,
                doors,
            });
        }
        let look = tag(tags, "look")?;
        if !rules.looks.contains_key(look) {
            return Err(format!("tag look: no look {look}"));
        }
        let half = row(tags, "half")?;
        let n = half.len();
        let one = |v: f64| (n == 1).then_some(v);
        let (across, up) = (row_of(tags, "across", n, one(0.0))?, row_of(tags, "up", n, one(0.0))?);
        let (height, collar) = (row_of(tags, "height", n, None)?, row_of(tags, "collar", n, None)?);
        let sections: Vec<&str> = tag(tags, "section")?.split(' ').collect();
        let kinds: Vec<&str> = if n == 1 { vec![name] } else { tag(tags, "kind")?.split(' ').collect() };
        if sections.len() != n || kinds.len() != n {
            return Err(format!("tags section, kind: not {n} values"));
        }
        let holes = (0..n)
            .map(|h| {
                let kind = Part::parse(kinds[h]).ok_or(format!("tag kind: {}", kinds[h]))?;
                let section = section_of(sections[h])?;
                Ok(Hole {
                    across: across[h],
                    up: up[h],
                    kind,
                    section,
                    half: half[h],
                    height: height[h],
                    collar: collar[h],
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let way: Vec<i64> = tag(tags, "way")?
            .split(' ')
            .map(|x| x.parse().map_err(|_| format!("tag way: {x}")))
            .collect::<Result<_, _>>()?;
        let shift = row(tags, "shift")?;
        if way.len() != 2 * shift.len() || shift.is_empty() {
            return Err("tags way, shift: not a vertex per shift".into());
        }
        let ways =
            shift.iter().enumerate().map(|(i, &shift)| Way { way: [way[2 * i], way[2 * i + 1]], shift }).collect();
        let walls = tags.contains_key("face");
        Ok(End {
            at,
            z,
            part,
            heading: number(tags, "heading")?,
            look: look.to_string(),
            holes,
            walls,
            face: if walls { number(tags, "face")? } else { 0.0 },
            left: if walls { row(tags, "left")? } else { Vec::new() },
            right: if walls { row(tags, "right")? } else { Vec::new() },
            foot: number(tags, "foot")?,
            ways,
            doors: Vec::new(),
        })
    }

    /// The unit direction into the tube, in world units.
    pub fn way_in(&self) -> [f64; 2] {
        [libm::cos(self.heading), libm::sin(self.heading)]
    }
}

/// A `tunnel-tube` record: one chord of a room from `a` to `b` (world point and the room's floor), in the rising metres
/// of its first lane's way, with its lanes from the right wall to the left and the places of the edges between them.
#[derive(Clone, Debug, PartialEq)]
pub struct Chord {
    pub a: [f64; 3],
    pub b: [f64; 3],
    pub section: Section,
    pub half: f64,
    /// At `a` and at `b`: of a room of lanes the room's height at each vertex, of a lone tube one height.
    pub height: [f64; 2],
    pub look: String,
    pub lanes: Vec<Lane>,
    /// Per edge between two lanes, its metres to the left of the axis at `a` and at `b`.
    pub edges: Vec<[f64; 2]>,
    /// The parts of the chain's two ends; None for an end with no part of its own.
    pub ends: [Option<Part>; 2],
}

/// A lane of a room: its way's class, the half-width of its carriageway, the metres along the stroke of its way at
/// `a`, its axis's metres to the left of the room's and its floor over the room's at `a` and at `b`, and the surface
/// code and the lines of its road as the viewer gives them (`code`, `marked`).
#[derive(Clone, Debug, PartialEq)]
pub struct Lane {
    pub class: String,
    pub road: f64,
    pub along: f64,
    pub off: [f64; 2],
    pub up: [f64; 2],
    pub code: u32,
    pub marked: bool,
}

impl Chord {
    /// The chord of a record, each of whose lanes' classes is in a group of `rules`; `code` and `marked` are the
    /// caller's.
    pub fn from_tags(a: [f64; 3], b: [f64; 3], tags: &Tags, rules: &Rules) -> Result<Chord, String> {
        let classes: Vec<&str> = tag(tags, "class")?.split(' ').collect();
        if let Some(class) = classes.iter().find(|c| rules.ladder.group(c).is_none()) {
            return Err(format!("tag class: {class} is in no group"));
        }
        let n = classes.len();
        let (road, along) = (row_of(tags, "road", n, None)?, row_of(tags, "along", n, None)?);
        let alone = (n == 1).then_some(0.0);
        let (off, up) = (row_of(tags, "off", 2 * n, alone)?, row_of(tags, "up", 2 * n, Some(0.0))?);
        let edges = if n == 1 { Vec::new() } else { row_of(tags, "edge", 2 * (n - 1), None)? };
        let lanes = (0..n)
            .map(|i| Lane {
                class: classes[i].to_string(),
                road: road[i],
                along: along[i],
                off: [off[2 * i], off[2 * i + 1]],
                up: [up[2 * i], up[2 * i + 1]],
                code: 0,
                marked: false,
            })
            .collect();
        let ends: Vec<&str> = tag(tags, "ends")?.split(' ').collect();
        let [e0, e1] = ends[..] else { return Err(format!("tag ends: {}", ends.join(" "))) };
        let height = match row(tags, "height")?[..] {
            [h] => [h, h],
            [a, b] => [a, b],
            _ => return Err("tag height: not one or two values".into()),
        };
        Ok(Chord {
            a,
            b,
            section: section(tags)?,
            half: number(tags, "half")?,
            height,
            look: tag(tags, "look")?.to_string(),
            lanes,
            edges: edges.chunks(2).map(|e| [e[0], e[1]]).collect(),
            ends: [Part::parse(e0), Part::parse(e1)],
        })
    }
}

/// A mesh in the layout of the viewer's structure meshes: per vertex 7 words (`mesh::WORDS`), each word's bits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<u32>,
    pub indices: Vec<u32>,
    /// The faces whose polygon the builder does not cut into triangles whole, so their edges stay open.
    pub unfilled: usize,
}

/// The volumes of parts: the boxes in which the ground does not draw (`BOX` words each) and the floor segments
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

/// The faces of the parts that a square draws: of each built record whose point it holds, and of each chord of a built
/// chain whose `a` it holds, so each face draws in one square.
pub fn meshes(square: [f64; 4], ends: &[End], chords: &[Chord], rules: &Rules) -> Mesh {
    let net = Net::new(ends, chords, rules);
    let mut b = mesh::Builder::new(|p| net.daylight(p));
    for (r, end) in ends.iter().enumerate() {
        if net.built_record(r) && holds(square, end.at) {
            if end.part == Part::Seam {
                seam::face(&mut b, &net, r);
            } else {
                arch::faces(&mut b, &net, r);
            }
        }
    }
    for (c, chord) in chords.iter().enumerate() {
        if net.built_chord(c) && holds(square, [chord.a[0], chord.a[1]]) {
            tube::inside(&mut b, &net, c);
        }
    }
    b.mesh()
}

/// What the builder finds in records as a tile holds them: per record whether it finds each of its holes (a record is
/// built where it finds all), and per chord whether each of its ends closes, as at a chord that does not draw: a chord
/// of a built chain closes at the opening of a built record or where it meets another such chord, as `meshes` draws
/// it. The bake reads its records back with it.
pub fn read_back(ends: &[End], chords: &[Chord], rules: &Rules) -> (Vec<Vec<bool>>, Vec<[bool; 2]>) {
    let net = Net::new(ends, chords, rules);
    let records = net.of_record.iter().map(|list| list.iter().map(|&e| net.found[e].is_some()).collect()).collect();
    let closed = (0..chords.len())
        .map(|c| [0, 1].map(|k| !net.built_chord(c) || net.opening_at(c, k) || net.meets(c, k)))
        .collect();
    (records, closed)
}

/// The boxes of the built holes and the floor segments of the lanes of the chords of built chains. `near_volumes` keeps those near
/// a square, so a tile's records build their volumes once.
pub fn volumes(ends: &[End], chords: &[Chord], rules: &Rules) -> Volumes {
    let net = Net::new(ends, chords, rules);
    let lift = rules.design.lift_m;
    let mut out = Volumes::default();
    for e in (0..net.holes.len()).filter(|&e| net.built_end(e) && net.holes[e].part != Part::Seam) {
        for words in boxes::boxes(&net, e) {
            out.boxes.extend(words);
        }
    }
    for (c, chord) in chords.iter().enumerate().filter(|&(c, _)| net.built_chord(c)) {
        let rings = [net.ring(c, 0), net.ring(c, 1)];
        for (i, lane) in chord.lanes.iter().enumerate() {
            // A lane's axis at each vertex, its floor there and its half-width, the lesser of its two.
            let at = |k: usize| {
                let (v, r) = (if k == 0 { chord.a } else { chord.b }, rings[k]);
                [v[0] + r.ax * lane.off[k], v[1] + r.ay * lane.off[k], v[2] + lane.up[k] + lift]
            };
            let half = [0, 1].map(|k| net.lane_edges(c, i, k)).map(|(lo, hi)| (hi - lo) / 2.0);
            let half = half[0].min(half[1]);
            let (p, q) = (at(0), at(1));
            out.floors.extend([p[0], p[1], q[0], q[1], p[2], q[2], half]);
        }
    }
    out
}

/// The boxes (`BOX` words each) and the floor segments (`FLOOR` words each) of volumes that come within `reach_m` of a
/// square, in their order: a box by its corners, a floor segment by its ends.
pub fn near_volumes(square: [f64; 4], boxes: &[f64], floors: &[f64], rules: &Rules) -> Volumes {
    let reach = rules.design.reach_m / metres_per_unit((square[1] + square[3]) / 2.0);
    let boxes = boxes.as_chunks::<BOX>().0.iter().filter(|b| near(square, &corners(b), reach));
    let floors = floors.as_chunks::<FLOOR>().0.iter().filter(|f| near(square, &[[f[0], f[1]], [f[2], f[3]]], reach));
    Volumes { boxes: boxes.flatten().copied().collect(), floors: floors.flatten().copied().collect() }
}

/// A way that enters a built head: its point on the frame line, its unit way into the tube, the metres from its point
/// to the jamb lines of its hole on each side (to the right negative), and the end vertex of the way and the metres
/// along it to the opening (`way`, `shift`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mouth {
    pub at: [f64; 2],
    pub into: [f64; 2],
    pub jamb: [f64; 2],
    pub way: [i64; 2],
    pub shift: f64,
}

/// The ways that enter the built heads.
pub fn mouths(ends: &[End], chords: &[Chord], rules: &Rules) -> Vec<Mouth> {
    let net = Net::new(ends, chords, rules);
    (0..ends.len()).filter(|&r| net.built_record(r)).flat_map(|r| net.mouths(r)).collect()
}

/// Per built hole with an opening, the ring that its part takes from the ground: round its head, from front to back,
/// and its collar.
pub fn footprints(ends: &[End], chords: &[Chord], rules: &Rules) -> Vec<Vec<[f64; 2]>> {
    let net = Net::new(ends, chords, rules);
    (0..net.holes.len())
        .filter(|&e| net.built_end(e) && !net.closed(e) && net.holes[e].part != Part::Seam)
        .map(|e| footprint::ring(&net, e))
        .collect()
}
