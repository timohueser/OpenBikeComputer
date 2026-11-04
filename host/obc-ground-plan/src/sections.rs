//! The cross-section of each way and waterway, from the rule tables of `data/trailview`, and the
//! reach table: every reach and chain of rules, bounded by `CHAIN_MAX_M`.
//!
//! A *finish* is what a way's top is made of, in the order of the mask's surface code (index + 1).

use std::sync::OnceLock;

use serde_json::{Map, Value};

use crate::frame::{CHAIN_MAX_M, MARGIN_M};
use crate::tile::Tags;

/// The widest half-width: the tiles' buffer holds the cone of a road up to 30 m wide.
pub const HALF_MAX_M: f64 = 15.0;
const HALF_MIN_M: f64 = 0.4;
/// A gauge narrower than this many mm is narrow.
const NARROW_MM: i64 = 1200;
const TRACKS: [&str; 4] = ["rail", "narrow_gauge", "light_rail", "funicular"];
const MAIN: [&str; 7] = ["motorway", "trunk", "primary", "secondary", "motorway_link", "trunk_link", "primary_link"];
const PAVED: [&str; 6] = ["asphalt", "paved", "concrete", "paving_stones", "sett", "chipseal"];
const GRAVEL: [&str; 4] = ["gravel", "compacted", "fine_gravel", "pebblestone"];
const DIRT: [&str; 6] = ["dirt", "ground", "earth", "mud", "unpaved", "sand"];
const GRASS: [&str; 2] = ["grass", "grass_paver"];
/// The path classes that a town paves where they have no `surface` tag.
const TOWN_PATHS: [&str; 4] = ["footway", "pedestrian", "cycleway", "steps"];
/// The path classes whose untagged finish is a path's: a cycleway is paved.
const PATHS: [&str; 6] = ["path", "footway", "bridleway", "steps", "pedestrian", "via_ferrata"];
const ONEWAY: [&str; 7] = ["yes", "true", "1", "-1", "reverse", "reversible", "alternating"];
const CLASS_FIELDS: [&str; 9] =
    ["half_m", "shoulder_m", "sidewalk_m", "kerb_m", "kerb_top_m", "cut", "fill", "reach", "widen_m"];
const WATER_FIELDS: [&str; 4] = ["width_m", "cut", "fill", "reach"];
/// The fields of the way graph: none is an option of the feature step.
const GRAPH_FIELDS: [&str; 5] = ["snap_m", "snap_height_m", "sag_plan_m", "sag_height_m", "clearance_m"];
/// The numeric fields of the faces: none is an option of the feature step.
const FACE_FIELDS: [&str; 5] = ["inner_radius_m", "verge_m", "return_grade", "corner_radius_m", "corner_radius_walk_m"];
const WATER_GRAPH_FIELDS: [&str; 9] = [
    "culvert_max_m",
    "culvert_width_m",
    "pipe_width_m",
    "stones_width_m",
    "clearance_m",
    "sag_plan_m",
    "sag_height_m",
    "curve_turn_max_deg",
    "curve_off_max_m",
];
/// The culvert chain's read along the water: a longer `culvert_max_m` would pass the graph's reach.
pub const CULVERT_MAX_M: f64 = 60.0;
/// Under this angle a way runs along the water or another way: the overlay of the faces decides, and
/// neither the graph nor the bake makes a crossing there.
pub const GRAZE_DEG: f64 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Finish {
    Main,
    Road,
    Gravel,
    Dirt,
    Grass,
    Path,
    Oneway,
    Paving,
    Rail,
    Narrow,
    Rack,
    NarrowRack,
}

impl Finish {
    pub const ALL: [Finish; 12] = [
        Finish::Main,
        Finish::Road,
        Finish::Gravel,
        Finish::Dirt,
        Finish::Grass,
        Finish::Path,
        Finish::Oneway,
        Finish::Paving,
        Finish::Rail,
        Finish::Narrow,
        Finish::Rack,
        Finish::NarrowRack,
    ];
    const NAMES: [&str; 12] = [
        "main",
        "road",
        "gravel",
        "dirt",
        "grass",
        "path",
        "oneway",
        "paving",
        "rail",
        "narrow",
        "rack",
        "narrow-rack",
    ];

    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }

    /// The mask's surface code.
    pub fn code(self) -> u8 {
        self as u8 + 1
    }

    /// The finish of a way: the port of `surfaceOf` and `trackSurface` of `terrain/bins.ts`. `town`
    /// where buildings are near.
    pub fn of(tags: &Tags, town: bool) -> Finish {
        let tag = |key: &str| tags.get(key).map_or("", String::as_str);
        if is_track(tags) {
            let narrow = match leading_integer(tag("gauge")) {
                Some(gauge) => gauge < NARROW_MM,
                None => matches!(tag("railway"), "narrow_gauge" | "funicular"),
            };
            return match (narrow, is_rack(tags)) {
                (true, true) => Finish::NarrowRack,
                (true, false) => Finish::Narrow,
                (false, true) => Finish::Rack,
                (false, false) => Finish::Rail,
            };
        }
        let (highway, surface) = (tag("highway"), tag("surface"));
        let oneway = match tag("oneway") {
            "" => matches!(highway, "motorway" | "motorway_link") || tag("junction") == "roundabout",
            value => ONEWAY.contains(&value),
        };
        let main = if oneway { Finish::Oneway } else { Finish::Main };
        if PAVED.contains(&surface) {
            return if MAIN[..4].contains(&highway) { main } else { Finish::Road };
        }
        if GRAVEL.contains(&surface) {
            return Finish::Gravel;
        }
        if DIRT.contains(&surface) {
            return Finish::Dirt;
        }
        if GRASS.contains(&surface) {
            return Finish::Grass;
        }
        if town && TOWN_PATHS.contains(&highway) {
            return Finish::Paving;
        }
        if PATHS.contains(&highway) {
            return Finish::Path;
        }
        if highway == "track" {
            return match tag("tracktype") {
                "grade1" => Finish::Road,
                "grade2" => Finish::Gravel,
                "grade3" => Finish::Dirt,
                _ => Finish::Grass,
            };
        }
        if MAIN.contains(&highway) {
            main
        } else {
            Finish::Road
        }
    }
}

/// Whether a way is a track: a `railway` of the tracks and no `highway`.
pub fn is_track(tags: &Tags) -> bool {
    tags.get("railway").is_some_and(|r| TRACKS.contains(&r.as_str())) && !tags.contains_key("highway")
}

fn is_rack(tags: &Tags) -> bool {
    tags.get("rack").is_some_and(|rack| rack != "no")
}

/// The class of a way: `rack` for a rack railway, else the `railway` of a track or its `highway`.
pub fn class_of(tags: &Tags) -> &str {
    if is_track(tags) {
        return if is_rack(tags) { "rack" } else { &tags["railway"] };
    }
    tags.get("highway").map_or("", String::as_str)
}

/// The text without the white space that JavaScript trims before a number.
fn trim_space(text: &str) -> &str {
    text.trim_start_matches(|c: char| c.is_whitespace() && c != '\u{85}' || c == '\u{feff}')
}

/// The number at the start of a tag, as JavaScript's `parseFloat` reads it: "8 m" is 8. None where
/// it is not finite.
pub fn leading_number(text: &str) -> Option<f64> {
    let text = trim_space(text);
    let span = text.find(|c: char| !matches!(c, '0'..='9' | '+' | '-' | '.' | 'e' | 'E')).unwrap_or(text.len());
    (1..=span).rev().find_map(|end| text[..end].parse::<f64>().ok()).filter(|v| v.is_finite())
}

/// The integer at the start of a tag, as JavaScript's `parseInt(text, 10)` reads it: "1435;1000" is
/// 1435. It saturates past `i64`.
pub fn leading_integer(text: &str) -> Option<i64> {
    let text = trim_space(text);
    let (sign, digits) = match text.as_bytes().first() {
        Some(b'-') => (-1, &text[1..]),
        Some(b'+') => (1, &text[1..]),
        _ => (1, text),
    };
    let digits = &digits[..digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len())];
    if digits.is_empty() {
        return None;
    }
    Some(digits.bytes().fold(0i64, |n, d| n.saturating_mul(10).saturating_add(i64::from(d - b'0'))) * sign)
}

/// The width of a waterway: the number at the start of its `width` where it is above 0, else
/// `width_m` of its row.
pub fn water_width_m(tags: &Tags, width_m: f64) -> f64 {
    tags.get("width").and_then(|w| leading_number(w)).filter(|&w| w > 0.0).unwrap_or(width_m)
}

/// The widths of one class row of `roads.json`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClassWidths {
    pub half_m: f64,
    pub shoulder_m: f64,
    pub sidewalk_m: f64,
}

/// One row of `finishes.json`: a number caps the class's value, None keeps it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FinishWidths {
    pub half_m: Option<f64>,
    pub shoulder_m: Option<f64>,
}

/// The widths of one way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Widths {
    /// The class's row in `roads.json`, from the most important down.
    pub rank: usize,
    pub finish: Finish,
    pub half_m: f64,
    pub shoulder_m: f64,
    pub sidewalk_m: f64,
}

impl Widths {
    /// The outer half-width of the outermost band: the shoulder or the sidewalk, the wider.
    pub fn outer_m(&self) -> f64 {
        self.half_m + self.shoulder_m.max(self.sidewalk_m)
    }
}

/// The width rule of the ways, a pure function of its rows. The crate builds it from the embedded
/// tables, the feature bake from its step options, so both give one width per way.
#[derive(Clone, Debug, PartialEq)]
pub struct WidthRules {
    classes: Vec<(String, ClassWidths)>,
    road: usize,
    path: usize,
    /// Per finish in code order.
    finishes: [FinishWidths; 12],
}

impl WidthRules {
    /// The rules, or an error that names the row and the field that break one.
    pub fn new(classes: Vec<(String, ClassWidths)>, finishes: Vec<(String, FinishWidths)>) -> Result<Self, String> {
        for (name, row) in &classes {
            if !(HALF_MIN_M..=HALF_MAX_M).contains(&row.half_m) {
                return Err(format!(
                    "roads.json {name:?}.half_m: {} is not from {HALF_MIN_M} to {HALF_MAX_M}",
                    row.half_m
                ));
            }
            for (field, value) in [("shoulder_m", row.shoulder_m), ("sidewalk_m", row.sidewalk_m)] {
                if !(0.0..).contains(&value) {
                    return Err(format!("roads.json {name:?}.{field}: {value} is not a number of 0 or more"));
                }
            }
        }
        let row = |fallback: &str| {
            classes
                .iter()
                .position(|(name, _)| name == fallback)
                .ok_or(format!("roads.json: no row {fallback:?}, the row of a class that the table lacks"))
        };
        let (road, path) = (row("road")?, row("path")?);
        let names: Vec<&str> = finishes.iter().map(|(name, _)| name.as_str()).collect();
        if names != Finish::NAMES {
            return Err(format!("finishes.json: the finishes are {names:?}, not {:?} in code order", Finish::NAMES));
        }
        for (name, row) in &finishes {
            if row.half_m.is_some_and(|h| !(HALF_MIN_M..=HALF_MAX_M).contains(&h)) {
                return Err(format!("finishes.json {name:?}.half_m: not from {HALF_MIN_M} to {HALF_MAX_M}"));
            }
            if row.shoulder_m.is_some_and(|s| !(0.0..).contains(&s)) {
                return Err(format!("finishes.json {name:?}.shoulder_m: not a number of 0 or more"));
            }
        }
        let finishes = std::array::from_fn(|at| finishes[at].1);
        Ok(WidthRules { classes, road, path, finishes })
    }

    /// The widths of a way of a feature kind. A class that the table lacks takes the row `path` for
    /// the kind `path`, else the row `road`. A track reads no `width`, as the viewer.
    pub fn way(&self, kind: &str, tags: &Tags, town: bool) -> Widths {
        let class = class_of(tags);
        let fallback = if kind == "path" { self.path } else { self.road };
        let rank = self.classes.iter().position(|(name, _)| name == class).unwrap_or(fallback);
        let row = self.classes[rank].1;
        let finish = Finish::of(tags, town);
        let caps = self.finishes[finish as usize];
        let width = (!is_track(tags)).then(|| tags.get("width").and_then(|w| leading_number(w))).flatten();
        let half_m = match width {
            Some(width) => (width / 2.0).max(HALF_MIN_M),
            None => caps.half_m.map_or(row.half_m, |cap| row.half_m.min(cap)),
        };
        Widths {
            rank,
            finish,
            half_m: half_m.min(HALF_MAX_M),
            shoulder_m: caps.shoulder_m.map_or(row.shoulder_m, |cap| row.shoulder_m.min(cap)),
            sidewalk_m: row.sidewalk_m,
        }
    }
}

/// The cross-section of a way. Metres, and rise per metre for `cut` and `fill`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaySection<'a> {
    pub class: &'a str,
    /// The class's row in `roads.json`, from the most important down.
    pub rank: usize,
    pub finish: Finish,
    /// The carriageway's half-width.
    pub half_m: f64,
    /// The flat strip past the edge where no sidewalk is.
    pub shoulder_m: f64,
    /// A sidewalk's width where `walk` puts one.
    pub sidewalk_m: f64,
    /// The kerb's rise over the carriageway's edge, and the width of its top.
    pub kerb_m: f64,
    pub kerb_top_m: f64,
    pub cut: f64,
    pub fill: f64,
    /// How far past the edge the slopes reach.
    pub reach_m: f64,
    /// An end within this of a carriageway's edge and `snap_height_m` of its height joins it.
    pub snap_m: f64,
    pub snap_height_m: f64,
    /// The most that a sample chord may leave the curve at the outer edge, and in height.
    pub sag_plan_m: f64,
    pub sag_height_m: f64,
    /// The height that this way needs under a deck.
    pub clearance_m: f64,
    /// The class group of the junction pair rules of `plan.json`.
    pub group: &'a str,
    /// The least radius of the inner edge in a tight bend; 0: no tip fill.
    pub inner_radius_m: f64,
    /// The radius of a junction corner, and of one with a sidewalk on its side; a corner takes the
    /// lesser of its two arms' values. 0: a sharp corner.
    pub corner_radius_m: f64,
    pub corner_radius_walk_m: f64,
    pub shoulder_paved: bool,
    /// The band past the shoulder, before the slope.
    pub verge_m: f64,
    /// The grade at which a guest's return ramp climbs back to its own heights.
    pub return_grade: f64,
}

impl WaySection<'_> {
    /// Its widths, whose `outer_m` is the outer half-width of the bands up to the verge.
    pub fn widths(&self) -> Widths {
        let Self { rank, finish, half_m, shoulder_m, sidewalk_m, .. } = *self;
        Widths { rank, finish, half_m, shoulder_m, sidewalk_m }
    }
}

/// The fields of a class row that the faces' rules check.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClassFaces<'a> {
    pub group: &'a str,
    pub shoulder_m: f64,
    pub reach_m: f64,
    pub verge_m: f64,
    pub return_grade: f64,
    pub inner_radius_m: f64,
    pub corner_radius_m: f64,
    pub corner_radius_walk_m: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterSection {
    /// The row's index in `waterways.json`.
    pub rank: usize,
    /// The width: `width`, else `width_m` of the row; and half of it, at most `HALF_MAX_M`.
    pub width_m: f64,
    pub half_m: f64,
    pub cut: f64,
    pub fill: f64,
    pub reach_m: f64,
    pub culvert_max_m: f64,
    /// The widest water that an untagged road, track or railway crosses on a culvert.
    pub culvert_width_m: f64,
    /// An untagged path crosses water up to this width on a pipe in stones, then on stepping stones.
    pub pipe_width_m: f64,
    pub stones_width_m: f64,
    /// The height over the water that a deck needs.
    pub clearance_m: f64,
    /// The most that a sample chord may leave the curve at the bank, and in height.
    pub sag_plan_m: f64,
    pub sag_height_m: f64,
    /// A vertex where the water turns by more than this is a corner: its two spans are chords.
    pub curve_turn_max_deg: f64,
    /// A span whose curve leaves its mapped chord by more than this is its chord: the bake reads the
    /// terrain for the water's level within the half-width and 2 m of the mapped line.
    pub curve_off_max_m: f64,
}

/// One row of the reach table: a reach, or a chain whose value is the sum of its terms.
#[derive(Clone, Debug, PartialEq)]
pub struct Reach {
    pub name: String,
    /// The module of the crate that owns the row, such as `sections`.
    pub owner: &'static str,
    pub chain: bool,
    /// (what, metres).
    pub terms: Vec<(String, f64)>,
}

impl Reach {
    pub fn metres(&self) -> f64 {
        self.terms.iter().map(|(_, m)| m).sum()
    }
}

/// The fields of a class row besides its widths.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    name: String,
    kerb_m: f64,
    kerb_top_m: f64,
    cut: f64,
    fill: f64,
    reach_m: f64,
    /// The most that a bend's alignment widens the half-width.
    widen_m: f64,
    /// `GRAPH_FIELDS`.
    graph: [f64; 5],
    group: String,
    shoulder_paved: bool,
    /// `FACE_FIELDS`.
    face: [f64; 5],
}

#[derive(Clone, Debug, PartialEq)]
struct WaterRow {
    name: String,
    width_m: f64,
    cut: f64,
    fill: f64,
    reach_m: f64,
    /// `WATER_GRAPH_FIELDS`.
    graph: [f64; 9],
}

/// The rule tables of the cross-sections.
#[derive(Clone, Debug, PartialEq)]
pub struct Sections {
    classes: Vec<Row>,
    widths: WidthRules,
    waters: Vec<WaterRow>,
    reaches: Vec<Reach>,
}

fn object<'a>(value: &'a Value, what: &str) -> Result<&'a Map<String, Value>, String> {
    value.as_object().ok_or_else(|| format!("{what}: not an object"))
}

/// A field that is a number, or an error that names the row and the field.
fn number(row: &Map<String, Value>, table: &str, name: &str, field: &str) -> Result<f64, String> {
    match row.get(field) {
        Some(Value::Object(_)) => Err(format!("{table} {name:?}.{field}: a value by place is not read yet")),
        Some(value) => value.as_f64().ok_or_else(|| format!("{table} {name:?}.{field}: not a number")),
        None => Err(format!("{table} {name:?}: no field {field}")),
    }
}

/// The fields of a row, each a number of 0 or more.
fn fields<const N: usize>(
    row: &Map<String, Value>,
    table: &str,
    name: &str,
    names: [&str; N],
) -> Result<[f64; N], String> {
    let mut out = [0.0; N];
    for (value, field) in out.iter_mut().zip(names) {
        *value = number(row, table, name, field)?;
        if *value < 0.0 {
            return Err(format!("{table} {name:?}.{field}: below 0"));
        }
    }
    Ok(out)
}

fn nullable(row: &Map<String, Value>, name: &str, field: &str) -> Result<Option<f64>, String> {
    match row.get(field) {
        Some(Value::Null) => Ok(None),
        _ => number(row, "finishes.json", name, field).map(Some),
    }
}

impl Sections {
    /// The committed tables, parsed once. It panics on an invalid table; a test parses them.
    pub fn embedded() -> &'static Sections {
        static SECTIONS: OnceLock<Sections> = OnceLock::new();
        SECTIONS.get_or_init(|| {
            Sections::parse(
                include_str!("../../../data/trailview/roads.json"),
                include_str!("../../../data/trailview/finishes.json"),
                include_str!("../../../data/trailview/waterways.json"),
            )
            .unwrap_or_else(|e| panic!("data/trailview: {e}"))
        })
    }

    /// The tables, or an error that names the row and the field that break a rule.
    pub fn parse(roads: &str, finishes: &str, waterways: &str) -> Result<Sections, String> {
        let json = |text: &str, table: &str| serde_json::from_str::<Value>(text).map_err(|e| format!("{table}: {e}"));
        let (mut classes, mut widths) = (Vec::new(), Vec::new());
        for (name, row) in object(&json(roads, "roads.json")?, "roads.json")? {
            let row = object(row, &format!("roads.json {name:?}"))?;
            let [half_m, shoulder_m, sidewalk_m, kerb_m, kerb_top_m, cut, fill, reach_m, widen_m] =
                CLASS_FIELDS.map(|field| number(row, "roads.json", name, field));
            let class = ClassWidths { half_m: half_m?, shoulder_m: shoulder_m?, sidewalk_m: sidewalk_m? };
            let row = Row {
                name: name.clone(),
                kerb_m: kerb_m?,
                kerb_top_m: kerb_top_m?,
                cut: cut?,
                fill: fill?,
                reach_m: reach_m?,
                widen_m: widen_m?,
                graph: fields(row, "roads.json", name, GRAPH_FIELDS)?,
                group: (row.get("group").and_then(Value::as_str))
                    .ok_or_else(|| format!("roads.json {name:?}.group: not a name"))?
                    .to_string(),
                shoulder_paved: (row.get("shoulder_paved").and_then(Value::as_bool))
                    .ok_or_else(|| format!("roads.json {name:?}.shoulder_paved: not true or false"))?,
                face: fields(row, "roads.json", name, FACE_FIELDS)?,
            };
            let fields = [row.kerb_m, row.kerb_top_m, row.cut, row.fill, row.reach_m, row.widen_m];
            if let Some(at) = fields.iter().position(|&v| v < 0.0) {
                return Err(format!("roads.json {name:?}.{}: below 0", CLASS_FIELDS[at + 3]));
            }
            let walk = class.sidewalk_m > 0.0;
            if (row.kerb_m > 0.0) != walk || (row.kerb_top_m > 0.0) != walk {
                return Err(format!(
                    "roads.json {name:?}: kerb_m and kerb_top_m are over 0 exactly where sidewalk_m is"
                ));
            }
            if HALF_MAX_M + row.widen_m + row.graph[0] > crate::graph::SNAP_REACH_M {
                return Err(format!(
                    "roads.json {name:?}: HALF_MAX_M + widen_m + snap_m is over the graph's SNAP_REACH_M"
                ));
            }
            classes.push(row);
            widths.push((name.clone(), class));
        }
        let mut finish_rows = Vec::new();
        for (name, row) in object(&json(finishes, "finishes.json")?, "finishes.json")? {
            let row = object(row, &format!("finishes.json {name:?}"))?;
            if let Some(field) = row.keys().find(|f| !["half_m", "shoulder_m"].contains(&f.as_str())) {
                return Err(format!("finishes.json {name:?}: an unknown field {field}"));
            }
            let caps =
                FinishWidths { half_m: nullable(row, name, "half_m")?, shoulder_m: nullable(row, name, "shoulder_m")? };
            finish_rows.push((name.clone(), caps));
        }
        let widths = WidthRules::new(widths, finish_rows)?;
        let mut waters = Vec::new();
        for (name, row) in object(&json(waterways, "waterways.json")?, "waterways.json")? {
            let row = object(row, &format!("waterways.json {name:?}"))?;
            let [width_m, cut, fill, reach_m] = WATER_FIELDS.map(|field| number(row, "waterways.json", name, field));
            let graph = fields(row, "waterways.json", name, WATER_GRAPH_FIELDS)?;
            let row =
                WaterRow { name: name.clone(), width_m: width_m?, cut: cut?, fill: fill?, reach_m: reach_m?, graph };
            if row.width_m <= 0.0 || [row.cut, row.fill, row.reach_m].iter().any(|&v| v < 0.0) {
                return Err(format!("waterways.json {name:?}: a width over 0, slopes and reach of 0 or more"));
            }
            if row.graph[0] > CULVERT_MAX_M {
                return Err(format!("waterways.json {name:?}.culvert_max_m: over {CULVERT_MAX_M}, the graph's read"));
            }
            waters.push(row);
        }
        for fallback in ["river", "stream"] {
            if !waters.iter().any(|row| row.name == fallback) {
                return Err(format!("waterways.json: no row {fallback:?}"));
            }
        }
        let mut reaches = reach_rows(&classes, &widths, &waters);
        reaches.extend(crate::graph::reach_rows(waters.iter().map(|row| row.graph[0]).fold(0.0, f64::max)));
        for row in reaches.iter().filter(|row| row.chain) {
            if row.metres() > CHAIN_MAX_M {
                let terms: Vec<String> = row.terms.iter().map(|(what, m)| format!("{what} {m} m")).collect();
                return Err(format!(
                    "the chain {:?} of {} is {} m = {}, over CHAIN_MAX_M {CHAIN_MAX_M} m",
                    row.name,
                    row.owner,
                    row.metres(),
                    terms.join(" + ")
                ));
            }
        }
        Ok(Sections { classes, widths, waters, reaches })
    }

    /// The cross-section of a way of a feature kind. A class that the table lacks takes the row
    /// `path` for the kind `path`, else the row `road`.
    pub fn way(&self, kind: &str, tags: &Tags, town: bool) -> WaySection<'_> {
        let Widths { rank, finish, half_m, shoulder_m, sidewalk_m } = self.widths.way(kind, tags, town);
        let row = &self.classes[rank];
        WaySection {
            class: &row.name,
            rank,
            finish,
            half_m,
            shoulder_m,
            sidewalk_m,
            kerb_m: row.kerb_m,
            kerb_top_m: row.kerb_top_m,
            cut: row.cut,
            fill: row.fill,
            reach_m: row.reach_m,
            snap_m: row.graph[0],
            snap_height_m: row.graph[1],
            sag_plan_m: row.graph[2],
            sag_height_m: row.graph[3],
            clearance_m: row.graph[4],
            group: &row.group,
            inner_radius_m: row.face[0],
            corner_radius_m: row.face[3],
            corner_radius_walk_m: row.face[4],
            shoulder_paved: row.shoulder_paved,
            verge_m: row.face[1],
            return_grade: row.face[2],
        }
    }

    /// The width rule of the ways.
    pub fn widths(&self) -> &WidthRules {
        &self.widths
    }

    /// The cross-section of a waterway: the row of its `waterway` value, else `river` for the kind
    /// `river`, else `stream`.
    pub fn water(&self, kind: &str, tags: &Tags) -> WaterSection {
        let value = tags.get("waterway").map_or("", String::as_str);
        let fallback = if kind == "river" { "river" } else { "stream" };
        let rank = (self.waters.iter().position(|row| row.name == value))
            .or_else(|| self.waters.iter().position(|row| row.name == fallback))
            .expect("parse keeps the rows river and stream");
        let row = &self.waters[rank];
        let width = water_width_m(tags, row.width_m);
        let [culvert_max_m, culvert_width_m, pipe_width_m, stones_width_m, clearance_m, sag_plan_m, sag_height_m, curve_turn_max_deg, curve_off_max_m] =
            row.graph;
        WaterSection {
            rank,
            width_m: width,
            half_m: (width / 2.0).min(HALF_MAX_M),
            cut: row.cut,
            fill: row.fill,
            reach_m: row.reach_m,
            culvert_max_m,
            culvert_width_m,
            pipe_width_m,
            stones_width_m,
            clearance_m,
            sag_plan_m,
            sag_height_m,
            curve_turn_max_deg,
            curve_off_max_m,
        }
    }

    /// The class row of a rank as the faces' rules check it; None past the table.
    pub fn class_faces(&self, rank: usize) -> Option<ClassFaces<'_>> {
        self.classes.get(rank).map(|row| ClassFaces {
            group: &row.group,
            shoulder_m: self.widths.classes[rank].1.shoulder_m,
            reach_m: row.reach_m,
            verge_m: row.face[1],
            return_grade: row.face[2],
            inner_radius_m: row.face[0],
            corner_radius_m: row.face[3],
            corner_radius_walk_m: row.face[4],
        })
    }

    /// The name of the class of a rank, or of the water kind of a water rank.
    pub fn rank_name(&self, water: bool, rank: usize) -> &str {
        if water {
            &self.waters[rank].name
        } else {
            &self.classes[rank].name
        }
    }

    /// Every reach and chain of rules.
    pub fn reaches(&self) -> &[Reach] {
        &self.reaches
    }
}

/// The rows of this module in the reach table.
fn reach_rows(classes: &[Row], widths: &WidthRules, waters: &[WaterRow]) -> Vec<Reach> {
    let term = |what: &str, metres: f64| (what.to_string(), metres);
    let reach = |name: &str, metres: f64| Reach {
        name: name.into(),
        owner: "sections",
        chain: false,
        terms: vec![term(name, metres)],
    };
    let widest = |values: &mut dyn Iterator<Item = (String, f64)>| {
        values.fold((String::new(), 0.0), |best, (name, m)| if m > best.1 { (name, m) } else { best })
    };
    let road_reach = widest(&mut classes.iter().map(|row| (row.name.clone(), row.reach_m)));
    let water_reach = widest(&mut waters.iter().map(|row| (row.name.clone(), row.reach_m)));
    let slope = widest(&mut classes.iter().zip(&widths.classes).map(|(row, (_, w))| {
        (row.name.clone(), row.widen_m + w.shoulder_m.max(w.sidewalk_m) + row.face[1] + row.reach_m)
    }));
    vec![
        reach("MARGIN_M", MARGIN_M),
        reach("HALF_MAX_M", HALF_MAX_M),
        reach(&format!("reach of roads.json {:?}", road_reach.0), road_reach.1),
        reach(&format!("reach of waterways.json {:?}", water_reach.0), water_reach.1),
        Reach {
            name: "way slope".into(),
            owner: "sections",
            chain: true,
            terms: vec![
                term("HALF_MAX_M", HALF_MAX_M),
                term(&format!("widen_m + max(shoulder_m, sidewalk_m) + verge_m + reach of {:?}", slope.0), slope.1),
            ],
        },
        Reach {
            name: "water bank".into(),
            owner: "sections",
            chain: true,
            terms: vec![term("HALF_MAX_M", HALF_MAX_M), term(&format!("reach of {:?}", water_reach.0), water_reach.1)],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> Tags {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    const ROADS: &str = include_str!("../../../data/trailview/roads.json");
    const FINISHES: &str = include_str!("../../../data/trailview/finishes.json");
    const WATERWAYS: &str = include_str!("../../../data/trailview/waterways.json");

    #[test]
    fn the_committed_tables_give_the_viewers_widths() {
        let s = Sections::embedded();
        let way = |kind: &str, pairs: &[(&str, &str)], town: bool| {
            let w = s.way(kind, &tags(pairs), town);
            (w.finish, w.half_m, w.shoulder_m)
        };
        // Golden values of halfWidth, surfaceOf and SHOULDER_M of terrain/bins.ts.
        assert_eq!(way("main-road", &[("highway", "primary")], false), (Finish::Main, 4.0, 0.6));
        assert_eq!(
            way("main-road", &[("highway", "primary"), ("surface", "gravel")], false),
            (Finish::Gravel, 1.75, 0.3)
        );
        assert_eq!(way("road", &[("highway", "residential"), ("width", "3 m")], false), (Finish::Road, 1.5, 0.4));
        assert_eq!(way("road", &[("highway", "track")], false), (Finish::Grass, 1.25, 0.2));
        assert_eq!(way("road", &[("highway", "track"), ("tracktype", "grade1")], false), (Finish::Road, 3.0, 0.4));
        assert_eq!(way("path", &[("highway", "path"), ("surface", "gravel")], false), (Finish::Gravel, 1.0, 0.0));
        assert_eq!(way("path", &[("highway", "pedestrian")], true), (Finish::Paving, 1.25, 0.0));
        assert_eq!(way("path", &[("highway", "cycleway")], false), (Finish::Road, 1.5, 0.0));
        assert_eq!(way("railway", &[("railway", "rail"), ("gauge", "1000")], false), (Finish::Narrow, 1.5, 0.35));
        assert_eq!(way("railway", &[("railway", "rail"), ("rack", "yes")], false), (Finish::Rack, 1.9, 0.35));
        assert_eq!(
            way("railway", &[("railway", "rail"), ("gauge", "1435;1000"), ("width", "9")], false),
            (Finish::Rail, 1.9, 0.35)
        );
        assert_eq!(way("road", &[("highway", "raceway")], false), (Finish::Road, 3.0, 0.4));
        assert_eq!(way("main-road", &[("highway", "motorway_link")], false), (Finish::Oneway, 4.5, 0.6));
        assert_eq!(
            way("main-road", &[("highway", "primary_link"), ("surface", "asphalt")], false),
            (Finish::Road, 3.0, 0.4)
        );
        assert_eq!(s.way("railway", &tags(&[("railway", "rail"), ("rack", "yes")]), false).class, "rack");
        assert_eq!(s.way("path", &tags(&[("highway", "steps")]), false).rank, 25);
        let main = s.way("main-road", &tags(&[("highway", "secondary")]), false);
        assert_eq!((main.sidewalk_m, main.kerb_m, main.kerb_top_m, main.reach_m), (2.0, 0.12, 0.16, 10.0));
    }

    #[test]
    fn odd_widths_follow_parse_float() {
        let s = Sections::embedded();
        let half = |width: &str| s.way("road", &tags(&[("highway", "residential"), ("width", width)]), false).half_m;
        assert_eq!(
            [half("60"), half("1e9"), half("3,5"), half("-2"), half("0"), half(" 4.5m")],
            [15.0, 15.0, 1.5, 0.4, 0.4, 2.25]
        );
        assert_eq!([half("~4"), half("nan"), half("")], [2.5; 3]);
        let n = leading_number;
        assert_eq!((n(" -2.5e1x"), n("1e"), n("1e+"), n("5-3")), (Some(-25.0), Some(1.0), Some(1.0), Some(5.0)));
        assert_eq!((n("m8"), n("."), n(""), n("+-5"), n("inf")), (None, None, None, None, None));
        assert_eq!((n("1e309"), n("\u{feff}2"), n("\u{85}2")), (None, Some(2.0), None));
        assert_eq!(
            [leading_integer("1435;1000"), leading_integer(" -5x"), leading_integer("x5")],
            [Some(1435), Some(-5), None]
        );
        let water = |kind: &str, pairs: &[(&str, &str)]| s.water(kind, &tags(pairs)).half_m;
        assert_eq!(water("stream", &[("waterway", "stream")]), 0.5);
        assert_eq!(water("river", &[("waterway", "canal"), ("width", "0")]), 5.0);
        assert_eq!(water("river", &[("waterway", "brook"), ("width", "40")]), 15.0);
    }

    #[test]
    fn a_broken_table_names_its_row_and_field() {
        let fails = |roads: &str, finishes: &str| Sections::parse(roads, finishes, WATERWAYS).unwrap_err();
        let kerb = ROADS.replacen(r#""kerb_m": 0,"#, r#""kerb_m": 0.1,"#, 1);
        assert!(fails(&kerb, FINISHES).contains(r#""rail": kerb_m and kerb_top_m"#));
        let missing = ROADS.replacen(r#", "kerb_top_m": 0"#, "", 1);
        assert!(fails(&missing, FINISHES).contains(r#"roads.json "rail": no field kerb_top_m"#));
        let missing = ROADS.replacen(r#", "snap_m": 1"#, "", 1);
        assert!(fails(&missing, FINISHES).contains(r#"roads.json "rail": no field snap_m"#));
        let long = WATERWAYS.replacen(r#""culvert_max_m": 60"#, r#""culvert_max_m": 61"#, 1);
        let message = Sections::parse(ROADS, FINISHES, &long).unwrap_err();
        assert!(message.contains(r#"waterways.json "river".culvert_max_m: over 60"#), "{message}");
        let by_place = ROADS.replacen(r#""half_m": 1.9"#, r#""half_m": {"CH": 1.9}"#, 1);
        assert!(fails(&by_place, FINISHES).contains("a value by place"));
        let order = FINISHES.replacen(r#""main""#, r#""tmp""#, 1).replacen(r#""road""#, r#""main""#, 1).replacen(
            r#""tmp""#,
            r#""road""#,
            1,
        );
        assert!(fails(ROADS, &order).contains("in code order"));
        let far = ROADS.replacen(r#""reach": 10"#, r#""reach": 180"#, 1);
        let message = fails(&far, FINISHES);
        assert!(message.contains(r#"the chain "way slope" of sections is 195.3"#), "{message}");
        assert!(
            message.contains(r#"reach of "rail" 180.3"#) && message.contains("over CHAIN_MAX_M 191 m"),
            "{message}"
        );
    }

    #[test]
    fn the_width_rule_checks_its_rows_and_a_water_width_falls_back_to_its_row() {
        let class = |half_m: f64| ClassWidths { half_m, shoulder_m: 0.4, sidewalk_m: 0.0 };
        let finishes: Vec<(String, FinishWidths)> = Finish::ALL
            .iter()
            .map(|f| (f.name().to_string(), FinishWidths { half_m: None, shoulder_m: None }))
            .collect();
        let rules = |rows: &[(&str, f64)], finishes: &[(String, FinishWidths)]| {
            WidthRules::new(rows.iter().map(|&(n, h)| (n.to_string(), class(h))).collect(), finishes.to_vec())
        };
        assert!(rules(&[("road", 3.0), ("path", 1.0)], &finishes).is_ok());
        assert!(rules(&[("path", 1.0)], &finishes).unwrap_err().contains(r#"no row "road""#));
        assert!(rules(&[("road", 0.3), ("path", 1.0)], &finishes).unwrap_err().contains(r#""road".half_m"#));
        let mut swapped = finishes.clone();
        swapped.swap(0, 1);
        assert!(rules(&[("road", 3.0), ("path", 1.0)], &swapped).unwrap_err().contains("in code order"));
        let width = |w: Option<&str>| water_width_m(&w.map_or_else(Tags::new, |w| tags(&[("width", w)])), 1.0);
        assert_eq!(
            [width(Some("0")), width(Some("-1")), width(Some("wide")), width(None), width(Some("2.5 m"))],
            [1.0, 1.0, 1.0, 1.0, 2.5]
        );
        let primary = Sections::embedded().widths().way("main-road", &tags(&[("highway", "primary")]), false);
        assert_eq!((primary.half_m, primary.outer_m()), (4.0, 6.0));
    }

    #[test]
    fn the_reach_table_holds_gp1s_rows() {
        let rows: Vec<(String, bool, f64)> =
            Sections::embedded().reaches().iter().map(|r| (r.name.clone(), r.chain, r.metres())).collect();
        assert!(rows.contains(&("way slope".into(), true, 29.0)), "{rows:?}");
        assert!(rows.contains(&("water bank".into(), true, 30.0)), "{rows:?}");
        assert!(rows.contains(&("MARGIN_M".into(), false, 192.0)), "{rows:?}");
        assert!(rows.contains(&("graph synthetic deck".into(), true, 80.0)), "{rows:?}");
    }
}
