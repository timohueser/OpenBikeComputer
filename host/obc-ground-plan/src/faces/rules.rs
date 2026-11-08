//! The rules of the faces: `plan.json` (shape values, ranks, junction pair rules) with the class
//! fields of `roads.json` that `Sections` reads, and the faces' rows of the reach table.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::{Map, Value};

use super::heights::{HeightRules, Legs, RULE_FIELDS};
use super::records::MARGIN_M;
use crate::frame::CHAIN_MAX_M;
use crate::graph::{HEADING_M, ROOM_MAX_M, SNAP_REACH_M};
use crate::sections::HALF_MAX_M;
use crate::sections::{Finish, Reach, Sections};

/// The overlay ranks, each named once in `ranks`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    Wet,
    Carriageway,
    Platform,
    RailBed,
    ShoulderPaved,
    WaterLine,
    Shoulder,
    Verge,
    Zone,
    Bank,
    Slope,
    Terrain,
}

const RANKS: [(&str, Rank); 12] = [
    ("wet", Rank::Wet),
    ("carriageway", Rank::Carriageway),
    ("platform", Rank::Platform),
    ("rail_bed", Rank::RailBed),
    ("shoulder_paved", Rank::ShoulderPaved),
    ("water_line", Rank::WaterLine),
    ("shoulder", Rank::Shoulder),
    ("verge", Rank::Verge),
    ("zone", Rank::Zone),
    ("bank", Rank::Bank),
    ("slope", Rank::Slope),
    ("terrain", Rank::Terrain),
];

/// How a guest meets its host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mouth {
    /// The guest's carriageway reaches the host's carriageway edge.
    Reach,
    /// The guest stops at the host's core edge.
    Stop,
    /// The bands meet by key only.
    Join,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pair {
    pub mouth: Mouth,
    pub blend: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanRules {
    /// How far a point's arms reach along their ways: its rounds and its seams read this far.
    pub extent_m: f64,
    /// A seam lies this far past the farthest point that a point's rounding or another arm reaches.
    pub seam_m: f64,
    /// Where the two arms of a joint differ in half-width, the wider one's carriageway edge runs into
    /// the narrower one's by this much per metre along it, over at most `extent_m`. The edge then turns
    /// by atan(0.15) = 8.5° at each end of the taper: under the kink check's `CORNER_TURN` (10.5°) by
    /// 2°, so neither end needs a round and a joint's own small turn there still makes no corner.
    pub taper_rate: f64,
    /// The block `heights`.
    pub heights: HeightRules,
    /// The half-width of a platform line without a `width` tag.
    pub platform_half_m: f64,
    /// A platform's `reach_m`: its edge is a wall, with no blend into the ground.
    pub platform_reach_m: f64,
    /// Per rank, its place in the list `ranks` of `plan.json`.
    ranks: [u8; RANKS.len()],
    /// "host group/guest group" and "default".
    pairs: BTreeMap<String, Pair>,
    /// Per finish, whether it is in `tread_finishes`: a guest of it keeps its own tread at a stop
    /// mouth also beside a host of its look.
    tread: [bool; 12],
}

impl PlanRules {
    pub fn rank(&self, rank: Rank) -> u8 {
        self.ranks[rank as usize]
    }

    /// The pair rule of a guest group at a host group, else `default`.
    pub fn pair(&self, host: &str, guest: &str) -> Pair {
        self.pairs.get(&format!("{host}/{guest}")).copied().unwrap_or(self.pairs["default"])
    }

    pub fn keeps_tread(&self, finish: Finish) -> bool {
        self.tread[finish as usize]
    }
}

/// The rules that `faces::build` reads.
#[derive(Clone, Debug, PartialEq)]
pub struct Rules {
    pub sections: Sections,
    pub plan: PlanRules,
    reaches: Vec<Reach>,
}

fn number(row: &Map<String, Value>, field: &str) -> Result<f64, String> {
    match row.get(field) {
        Some(Value::Object(_)) => Err(format!("plan.json {field}: a value by place is not read yet")),
        Some(v) => {
            v.as_f64().filter(|v| *v >= 0.0).ok_or_else(|| format!("plan.json {field}: not a number of 0 or more"))
        }
        None => Err(format!("plan.json: no field {field}")),
    }
}

impl Rules {
    /// The committed tables, parsed once. It panics on an invalid table; a test parses them.
    pub fn embedded() -> &'static Rules {
        static RULES: OnceLock<Rules> = OnceLock::new();
        RULES.get_or_init(|| {
            Rules::parse(Sections::embedded().clone(), include_str!("../../../../data/trailview/plan.json"))
                .unwrap_or_else(|e| panic!("data/trailview: {e}"))
        })
    }

    pub fn parse(sections: Sections, plan: &str) -> Result<Rules, String> {
        let json: Value = serde_json::from_str(plan).map_err(|e| format!("plan.json: {e}"))?;
        let top = json.as_object().ok_or("plan.json: not an object")?;
        let heights = top.get("heights").and_then(Value::as_object).ok_or("plan.json: no object heights")?;
        let h = |field: &str| number(heights, field).map_err(|e| e.replace("plan.json ", "plan.json heights."));
        let mut values = [0.0; RULE_FIELDS.len()];
        for (v, field) in values.iter_mut().zip(RULE_FIELDS) {
            *v = h(field)?;
        }
        if values[RULE_FIELDS.len() - 1] > MARGIN_M {
            return Err("plan.json heights.region_fade_m: over the records' MARGIN_M".into());
        }
        let legs = match heights.get("legs").and_then(Value::as_str) {
            Some("ramp") => Legs::Ramp,
            Some("wall") => Legs::Wall,
            _ => return Err("plan.json heights.legs: not ramp or wall".into()),
        };
        let names = top.get("ranks").and_then(Value::as_array).ok_or("plan.json: no list ranks")?;
        let names: Vec<&str> = names.iter().filter_map(Value::as_str).collect();
        let mut ranks = [u8::MAX; RANKS.len()];
        for (at, name) in names.iter().enumerate() {
            let Some(&(_, rank)) = RANKS.iter().find(|(n, _)| n == name) else {
                return Err(format!("plan.json ranks: an unknown rank {name:?}"));
            };
            if ranks[rank as usize] != u8::MAX {
                return Err(format!("plan.json ranks: {name:?} twice"));
            }
            ranks[rank as usize] = at as u8;
        }
        if let Some(&(name, _)) = RANKS.iter().find(|(_, r)| ranks[*r as usize] == u8::MAX) {
            return Err(format!("plan.json ranks: no rank {name:?}"));
        }
        let classes: Vec<_> = (0..).map_while(|rank| sections.class_faces(rank)).collect();
        // The groups of roads.json, and line water's.
        let groups: Vec<&str> = classes.iter().map(|c| c.group).chain(["water"]).collect();
        let mut pairs = BTreeMap::new();
        for (key, row) in top.get("pairs").and_then(Value::as_object).ok_or("plan.json: no object pairs")? {
            if key != "default" {
                let known = key.split_once('/').is_some_and(|(a, b)| groups.contains(&a) && groups.contains(&b));
                if !known {
                    return Err(format!("plan.json pairs {key:?}: not two groups of roads.json"));
                }
            }
            let row = row.as_object().ok_or_else(|| format!("plan.json pairs {key:?}: not an object"))?;
            let mouth = match row.get("mouth").and_then(Value::as_str) {
                Some("reach") => Mouth::Reach,
                Some("stop") => Mouth::Stop,
                Some("join") => Mouth::Join,
                _ => return Err(format!("plan.json pairs {key:?}.mouth: not reach, stop or join")),
            };
            let blend = row.get("blend").and_then(Value::as_bool);
            let blend = blend.ok_or_else(|| format!("plan.json pairs {key:?}.blend: not true or false"))?;
            pairs.insert(key.clone(), Pair { mouth, blend });
        }
        if !pairs.contains_key("default") {
            return Err("plan.json pairs: no default".into());
        }
        let mut tread = [false; 12];
        for name in top.get("tread_finishes").and_then(Value::as_array).ok_or("plan.json: no list tread_finishes")? {
            let at = Finish::ALL.iter().position(|f| Some(f.name()) == name.as_str());
            tread[at.ok_or_else(|| format!("plan.json tread_finishes: {name} is no finish"))?] = true;
        }
        let f = |field: &str| number(top, field);
        let plan = PlanRules {
            extent_m: f("extent_m")?,
            seam_m: f("seam_m")?,
            taper_rate: f("taper_rate")?,
            platform_half_m: f("platform_half_m")?,
            platform_reach_m: f("platform_reach_m")?,
            heights: HeightRules::new(values, legs),
            ranks,
            pairs,
            tread,
        };
        if classes.iter().any(|c| c.return_grade <= 0.0) {
            return Err("roads.json: a return_grade of 0".into());
        }
        let reaches = reach_rows(&plan, band_m(&sections), inner_max_m(&sections), radius_max_m(&sections));
        for row in reaches.iter().filter(|r| r.chain) {
            if row.metres() > CHAIN_MAX_M {
                return Err(format!(
                    "the chain {:?} of faces is {} m, over CHAIN_MAX_M {CHAIN_MAX_M} m",
                    row.name,
                    row.metres()
                ));
            }
        }
        Ok(Rules { sections, plan, reaches })
    }

    /// The reach table: the rows of `Sections` and those of the faces.
    pub fn reaches(&self) -> impl Iterator<Item = &Reach> {
        self.sections.reaches().iter().chain(&self.reaches)
    }
}

/// The widest band past a carriageway edge: shoulder, verge and slope reach.
fn band_m(sections: &Sections) -> f64 {
    (0..).map_while(|rank| sections.class_faces(rank)).map(|c| c.shoulder_m + c.verge_m + c.reach_m).fold(0.0, f64::max)
}

/// The distance past the cell within which a point's faces can reach the window, so that a cell
/// rounds the points within it: a point's arms and seam, a half-width and the widest band.
pub fn input_m(plan: &PlanRules, sections: &Sections) -> f64 {
    plan.extent_m + plan.seam_m + HALF_MAX_M + band_m(sections)
}

/// The distance past the cell within which a cell's faces read points: a point within `input_m`
/// rounds its arms up to the next point, at most `extent_m` on.
pub fn points_m(plan: &PlanRules, sections: &Sections) -> f64 {
    input_m(plan, sections) + plan.extent_m
}

/// The greatest inner radius of a class.
pub fn inner_max_m(sections: &Sections) -> f64 {
    (0..).map_while(|rank| sections.class_faces(rank)).map(|c| c.inner_radius_m).fold(0.0, f64::max)
}

/// The terms of the outline rounds' reach: a round's feet lie within `extent_m / 2` of its place
/// along the outline, so its rings lie within that and its radius; a place's round depends on the
/// rounds of the places within `extent_m`; a round reads the outline that its circle can meet,
/// another radius on; the outline holds the carriageways of the ways within a half-width. It also
/// holds the fans of the points within `input_m`, which these terms do not bound.
fn outline_terms(plan: &PlanRules, inner_m: f64) -> [(&'static str, f64); 4] {
    let round = plan.extent_m / 2.0 + inner_m;
    [
        ("extent_m / 2 + inner radius: a round's rings", round),
        ("extent_m: the places that a round depends on", plan.extent_m),
        ("extent_m / 2 + 2 inner radii: the outline that it reads", round + inner_m),
        ("HALF_MAX_M", HALF_MAX_M),
    ]
}

/// The greatest corner radius of a class, with a sidewalk or without.
fn corner_max_m(sections: &Sections) -> f64 {
    (0..)
        .map_while(|rank| sections.class_faces(rank))
        .map(|c| c.corner_radius_m.max(c.corner_radius_walk_m))
        .fold(0.0, f64::max)
}

/// The greatest corner or inner radius of a class.
fn radius_max_m(sections: &Sections) -> f64 {
    corner_max_m(sections).max(inner_max_m(sections))
}

/// The terms of the off-line test of a point's rounds past `input_m`: a round's centre lies up to
/// `extent_m` along an arm, a half-width and its radius off it, and the carriageways that its circle
/// meets lie another radius and half-width on.
fn off_terms(radius_m: f64) -> [(&'static str, f64); 2] {
    [
        ("HALF_MAX_M + radius: a round's centre off its arm", HALF_MAX_M + radius_m),
        ("radius + HALF_MAX_M: a carriageway it meets", radius_m + HALF_MAX_M),
    ]
}

/// The distance past the cell within which a cell's faces build carriageways: the points'
/// `input_m`, and the outline that the outline rounds read.
pub fn lines_m(plan: &PlanRules, sections: &Sections) -> f64 {
    let outline: f64 = outline_terms(plan, inner_max_m(sections)).iter().map(|t| t.1).sum();
    input_m(plan, sections).max(outline)
}

/// The distance past the cell within which a cell's faces read ways: the carriageways of `lines_m`,
/// and the ways that the rounds of the points within `input_m` test (`points_m` and `off_terms`).
pub fn ways_m(plan: &PlanRules, sections: &Sections) -> f64 {
    let off: f64 = off_terms(radius_max_m(sections)).iter().map(|t| t.1).sum();
    lines_m(plan, sections).max(points_m(plan, sections) + off)
}

/// The faces' rows: what a cell's faces read, and the chains through GP2's room decision.
fn reach_rows(plan: &PlanRules, band_m: f64, inner_m: f64, radius_m: f64) -> Vec<Reach> {
    let chain = |name: &str, terms: &[(&str, f64)]| Reach {
        name: name.into(),
        owner: "faces",
        chain: true,
        terms: terms.iter().map(|&(what, m)| (what.to_string(), m)).collect(),
    };
    // GP2's `room_m` reads the room and a snap foot there.
    let room = ROOM_MAX_M + 2.0 * SNAP_REACH_M;
    let input =
        [("extent_m", plan.extent_m), ("seam_m", plan.seam_m), ("HALF_MAX_M", HALF_MAX_M), ("widest band", band_m)];
    let mut points = input.to_vec();
    points.push(("extent_m: a run to the next point", plan.extent_m));
    let mut cell = vec![("GP2 room_m", room)];
    cell.extend(&points);
    let mut hosts = vec![("HEADING_M", HEADING_M)];
    hosts.extend(input);
    let mut outline = vec![("GP2 room_m", room)];
    outline.extend(outline_terms(plan, inner_m));
    let mut off = vec![("GP2 room_m", room)];
    off.extend(&points);
    off.extend(off_terms(radius_m));
    let rows = [
        ("input", &input[..]),
        ("cell", &cell),
        ("hosts", &hosts),
        ("points", &points),
        ("outline", &outline),
        ("off-line test", &off),
    ];
    let mut rows: Vec<Reach> = rows.iter().map(|(name, terms)| chain(&format!("faces {name}"), terms)).collect();
    // The records' blend edges are the neighbours' own up to `MARGIN_M` past the cell: the longest
    // chain reads that much farther.
    let longest = rows.iter().max_by(|a, b| a.metres().total_cmp(&b.metres())).map(|r| r.terms.clone());
    let mut terms = longest.unwrap_or_default();
    terms.push(("records MARGIN_M".into(), MARGIN_M));
    rows.push(Reach { name: "faces blend edges".into(), owner: "faces", chain: true, terms });
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = include_str!("../../../../data/trailview/plan.json");

    #[test]
    fn the_committed_rules_parse_and_a_broken_one_names_its_field() {
        let rules = Rules::embedded();
        assert_eq!(rules.plan.rank(Rank::Wet), 0);
        assert_eq!(rules.plan.pair("road", "path").mouth, Mouth::Stop);
        assert_eq!(rules.plan.pair("water", "road").mouth, Mouth::Join);
        assert_eq!(rules.plan.pair("water", "water"), Pair { mouth: Mouth::Join, blend: true });
        let cell = rules.reaches().find(|r| r.name == "faces cell").unwrap();
        assert!(cell.metres() < 191.0);
        let fails = |plan: &str| Rules::parse(Sections::embedded().clone(), plan).unwrap_err();
        assert!(fails(&PLAN.replacen(r#""verge", "#, "", 1)).contains(r#"no rank "verge""#));
        assert!(fails(&PLAN.replacen(r#""road/track""#, r#""road/lane""#, 1)).contains("not two groups"));
        assert!(fails(&PLAN.replacen(r#""extent_m": 20"#, r#""extent_m": 100"#, 1)).contains("faces cell"));
        assert!(fails(&PLAN.replacen(r#""legs": "ramp""#, r#""legs": "lean""#, 1)).contains("heights.legs"));
        let wide = PLAN.replacen(r#""region_fade_m": 1.5"#, r#""region_fade_m": 3.5"#, 1);
        assert!(fails(&wide).contains("heights.region_fade_m"));
        for field in RULE_FIELDS {
            let without = PLAN.replacen(&format!(r#""{field}":"#), r#""unread":"#, 1);
            assert!(fails(&without).contains(&format!("no field {field}")), "{field}");
        }
    }
}
