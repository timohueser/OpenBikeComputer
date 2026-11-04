//! The rules of the faces: `plan.json` (shape values, ranks, junction pair rules) with the class
//! fields of `roads.json` that `Sections` reads, and the faces' rows of the reach table.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde_json::{Map, Value};

use crate::frame::CHAIN_MAX_M;
use crate::graph::{HEADING_M, ROOM_MAX_M, SNAP_REACH_M};
use crate::sections::HALF_MAX_M;
use crate::sections::{Reach, Sections};

/// The overlay ranks, each named once in `ranks`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    Carriageway,
    RailBed,
    ShoulderPaved,
    Shoulder,
    Verge,
    Slope,
    Terrain,
}

const RANKS: [(&str, Rank); 7] = [
    ("carriageway", Rank::Carriageway),
    ("rail_bed", Rank::RailBed),
    ("shoulder_paved", Rank::ShoulderPaved),
    ("shoulder", Rank::Shoulder),
    ("verge", Rank::Verge),
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
    pub radius_m: f64,
    /// The radius where either arm has a sidewalk on the corner's side.
    pub radius_walk_m: Option<f64>,
    pub blend: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanRules {
    /// How far a point's arms reach along their ways: its rounds and its seams read this far.
    pub extent_m: f64,
    /// A seam lies this far past the farthest point that a point's rounding or another arm reaches.
    pub seam_m: f64,
    /// GP6's window past the cell, its blend steepness and blend radius.
    pub window_m: f64,
    pub blend_steep: f64,
    pub blend_radius_m: f64,
    /// The index of each rank in `ranks`.
    ranks: [u8; 7],
    /// "host group/guest group" and "default".
    pairs: BTreeMap<String, Pair>,
}

impl PlanRules {
    pub fn rank(&self, rank: Rank) -> u8 {
        self.ranks[rank as usize]
    }

    /// The pair rule of a guest group at a host group, else `default`.
    pub fn pair(&self, host: &str, guest: &str) -> Pair {
        self.pairs.get(&format!("{host}/{guest}")).copied().unwrap_or(self.pairs["default"])
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
        let names = top.get("ranks").and_then(Value::as_array).ok_or("plan.json: no list ranks")?;
        let names: Vec<&str> = names.iter().filter_map(Value::as_str).collect();
        let mut ranks = [u8::MAX; 7];
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
        let groups: Vec<&str> = classes.iter().map(|c| c.group).collect();
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
            let field =
                |f: &str| number(row, f).map_err(|e| e.replace("plan.json ", &format!("plan.json pairs {key:?}.")));
            let radius_walk_m = row.contains_key("radius_walk_m").then(|| field("radius_walk_m")).transpose()?;
            let blend = row.get("blend").and_then(Value::as_bool);
            let blend = blend.ok_or_else(|| format!("plan.json pairs {key:?}.blend: not true or false"))?;
            pairs.insert(key.clone(), Pair { mouth, radius_m: field("radius_m")?, radius_walk_m, blend });
        }
        if !pairs.contains_key("default") {
            return Err("plan.json pairs: no default".into());
        }
        let f = |field: &str| number(top, field);
        let plan = PlanRules {
            extent_m: f("extent_m")?,
            seam_m: f("seam_m")?,
            window_m: h("window_m")?,
            blend_steep: h("blend_steep")?,
            blend_radius_m: h("blend_radius_m")?,
            ranks,
            pairs,
        };
        if classes.iter().any(|c| c.return_grade <= 0.0) {
            return Err("roads.json: a return_grade of 0".into());
        }
        let radius = plan.pairs.values().map(|p| p.radius_m.max(p.radius_walk_m.unwrap_or(0.0))).fold(0.0, f64::max);
        let shoulder = classes.iter().map(|c| c.shoulder_m).fold(0.0, f64::max);
        if radius + shoulder > plan.blend_radius_m {
            return Err(format!(
                "plan.json: the greatest radius {radius} m and shoulder {shoulder} m are over heights.blend_radius_m"
            ));
        }
        let reaches = reach_rows(&plan, band_m(&sections));
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

/// The distance past the cell within which a cell's faces read ways and points: a point's arms and
/// seam, a half-width and the widest band.
pub fn input_m(plan: &PlanRules, sections: &Sections) -> f64 {
    plan.extent_m + plan.seam_m + HALF_MAX_M + band_m(sections)
}

/// The faces' rows: what a cell's faces read, and the chain through GP2's room decision.
fn reach_rows(plan: &PlanRules, band_m: f64) -> Vec<Reach> {
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
    let mut cell = vec![("GP2 room_m", room)];
    cell.extend(input);
    let mut hosts = vec![("HEADING_M", HEADING_M)];
    hosts.extend(input);
    vec![chain("faces input", &input), chain("faces cell", &cell), chain("faces hosts", &hosts)]
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = include_str!("../../../../data/trailview/plan.json");

    #[test]
    fn the_committed_rules_parse_and_a_broken_one_names_its_field() {
        let rules = Rules::embedded();
        assert_eq!(rules.plan.rank(Rank::Carriageway), 0);
        assert_eq!(rules.plan.pair("road", "path").mouth, Mouth::Stop);
        assert_eq!(rules.plan.pair("water", "road").mouth, Mouth::Join);
        let cell = rules.reaches().find(|r| r.name == "faces cell").unwrap();
        assert!(cell.metres() < 191.0);
        let fails = |plan: &str| Rules::parse(Sections::embedded().clone(), plan).unwrap_err();
        assert!(fails(&PLAN.replacen(r#""verge", "#, "", 1)).contains(r#"no rank "verge""#));
        assert!(fails(&PLAN.replacen(r#""road/track""#, r#""road/lane""#, 1)).contains("not two groups"));
        assert!(fails(&PLAN.replacen(r#""extent_m": 20"#, r#""extent_m": 100"#, 1)).contains("faces cell"));
        assert!(fails(&PLAN.replacen(r#""radius_m": 7"#, r#""radius_m": 9.5"#, 1)).contains("blend_radius_m"));
    }
}
