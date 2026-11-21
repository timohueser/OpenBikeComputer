//! The table `data/trailview/parts.json`: the ladder of the feature bake (`groups`, `bake`), and the design and the
//! colours of the parts that the builder makes (`design`, `looks`, `inside`).

use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

use serde_json::Value;

/// The rule values of the bake, in the order of `Bake`.
const BAKE_FIELDS: [&str; 19] = [
    "margin_m",
    "margin_narrow_m",
    "shell_m",
    "band_m",
    "coping_m",
    "face_over_crown_max_m",
    "cover_m",
    "wing_ground_m",
    "foot_m",
    "underpass_max_m",
    "pair_max_m",
    "pair_max_deg",
    "slab_m",
    "rock_cover_max_m",
    "free_max_m",
    "junction_shift_max_m",
    "narrow_bore_m",
    "grade_default",
    "vertex_gap_m",
];
const GROUP_FIELDS: [&str; 7] = ["classes", "look", "collar_max_m", "wing_max_m", "least_height_m", "rock", "bore"];
/// The values of the design, in the order of `Design`.
const DESIGN_FIELDS: [&str; 18] = [
    "frame_m",
    "recess_m",
    "face_m",
    "arch_segments",
    "spring",
    "lamp_spacing_m",
    "lamp_m",
    "kerb_m",
    "daylight_m",
    "void_m",
    "portal_cells",
    "mesh_texels",
    "sleeper_m",
    "reach_m",
    "lift_m",
    "mitre_max",
    "approach_m",
    "wing_slope",
];
const LOOK_FIELDS: [&str; 4] = ["face", "frame", "coping", "shell"];
const INSIDE_FIELDS: [&str; 8] = ["tiles", "arch_lining", "lining", "walk", "rock", "rock_floor", "lamp", "void"];

/// The values of a group of road classes (`groups`).
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub name: String,
    pub classes: Vec<String>,
    pub look: String,
    pub collar_max_m: f64,
    pub wing_max_m: f64,
    pub least_height_m: f64,
    /// An end covered within `rock_cover_max_m` opens in the rock.
    pub rock: bool,
    pub bore: Bore,
}

/// The inside of a group's tubes: a road with kerbs, walks, tiles and lamps; a track in a lining; a floor and walls of
/// rock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bore {
    Road,
    Track,
    Rock,
}

/// The rule values of the placement (`bake`), metres unless named.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bake {
    pub margin_m: f64,
    pub margin_narrow_m: f64,
    pub shell_m: f64,
    pub band_m: f64,
    pub coping_m: f64,
    pub face_over_crown_max_m: f64,
    pub cover_m: f64,
    pub wing_ground_m: f64,
    pub foot_m: f64,
    pub underpass_max_m: f64,
    pub pair_max_m: f64,
    pub pair_max_deg: f64,
    pub slab_m: f64,
    pub rock_cover_max_m: f64,
    pub free_max_m: f64,
    pub junction_shift_max_m: f64,
    /// A bore of this height or less takes `margin_narrow_m`.
    pub narrow_bore_m: f64,
    /// The steepest grade of the floor of a tube whose class has none.
    pub grade_default: f64,
    /// The vertices of a tube lie this far apart at least, but at its openings and at the joins of its ways.
    pub vertex_gap_m: f64,
}

/// The rules of the feature bake: `groups` and `bake`, its option `parts`.
#[derive(Clone, Debug, PartialEq)]
pub struct Ladder {
    pub groups: Vec<Group>,
    pub bake: Bake,
}

/// The sizes of the parts (`design`), metres unless named.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Design {
    /// The width of the frame round an opening, and how far the head wall's front stands out of it.
    pub frame_m: f64,
    pub recess_m: f64,
    /// The thickness of a head wall.
    pub face_m: f64,
    pub arch_segments: usize,
    /// A round section springs at this share of its height.
    pub spring: f64,
    pub lamp_spacing_m: f64,
    pub lamp_m: f64,
    pub kerb_m: f64,
    /// The daylight in a tube falls to 0 over this many metres from an opening.
    pub daylight_m: f64,
    pub void_m: f64,
    /// As `PORTAL_CELLS` and `MESH_TEXELS` of the viewer's `RULES`: the depth of a box.
    pub portal_cells: f64,
    pub mesh_texels: f64,
    pub sleeper_m: f64,
    /// A box or a floor segment within this of a square belongs to it.
    pub reach_m: f64,
    /// A road face and a floor lie this far over the floor.
    pub lift_m: f64,
    /// A mitre grows at most this many times the section in a sharp bend.
    pub mitre_max: f64,
    /// The ground way before an opening widens to the opening's inner width and back over this length.
    pub approach_m: f64,
    /// Toward its end, the top of a wing wall falls this many metres per metre to its coping, or more steeply where its
    /// row is too short to fall from the head wall's top.
    pub wing_slope: f64,
}

/// The colours of a look, as the viewer's `rgb` packs them (red in the low byte).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub face: u32,
    pub frame: u32,
    pub coping: u32,
    pub shell: u32,
}

/// The colours of the inside of a tube.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inside {
    pub tiles: u32,
    pub arch_lining: u32,
    pub lining: u32,
    pub walk: u32,
    pub rock: u32,
    pub rock_floor: u32,
    pub lamp: u32,
    pub void: u32,
}

/// The whole table.
#[derive(Clone, Debug, PartialEq)]
pub struct Rules {
    pub ladder: Ladder,
    pub design: Design,
    pub looks: BTreeMap<String, Look>,
    pub inside: Inside,
}

/// The numbers of the fields of `row`, which holds these fields and no other.
fn numbers(row: &Value, fields: &[&str], at: &str) -> Result<Vec<f64>, String> {
    if row.as_object().is_none_or(|r| r.len() != fields.len()) {
        return Err(format!("parts {at}: the fields {fields:?}"));
    }
    (fields.iter())
        .map(|f| row[f].as_f64().filter(|v| *v >= 0.0).ok_or(format!("parts {at}.{f}: not a number of 0 or more")))
        .collect()
}

/// The colours of the fields of `row`, which holds these fields and no other, each `#rrggbb`.
fn colours(row: &Value, fields: &[&str], at: &str) -> Result<Vec<u32>, String> {
    if row.as_object().is_none_or(|r| r.len() != fields.len()) {
        return Err(format!("parts {at}: the fields {fields:?}"));
    }
    let colour = |f: &&str| {
        let hex = row[*f].as_str().and_then(|s| s.strip_prefix('#')).filter(|h| h.len() == 6);
        let rgb = hex.and_then(|h| u32::from_str_radix(h, 16).ok()).ok_or(format!("parts {at}.{f}: not #rrggbb"))?;
        Ok((rgb >> 16) | (rgb & 0xff00) | ((rgb & 0xff) << 16))
    };
    fields.iter().map(colour).collect()
}

impl Ladder {
    /// The ladder of the option `parts`, or an error that names the field that breaks one.
    pub fn parse(rows: &Value) -> Result<Ladder, String> {
        let v = numbers(&rows["bake"], &BAKE_FIELDS, "bake")?;
        let bake = Bake {
            margin_m: v[0],
            margin_narrow_m: v[1],
            shell_m: v[2],
            band_m: v[3],
            coping_m: v[4],
            face_over_crown_max_m: v[5],
            cover_m: v[6],
            wing_ground_m: v[7],
            foot_m: v[8],
            underpass_max_m: v[9],
            pair_max_m: v[10],
            pair_max_deg: v[11],
            slab_m: v[12],
            rock_cover_max_m: v[13],
            free_max_m: v[14],
            junction_shift_max_m: v[15],
            narrow_bore_m: v[16],
            grade_default: v[17],
            vertex_gap_m: v[18],
        };
        let mut groups = Vec::new();
        for (name, row) in rows["groups"].as_object().ok_or("parts groups: not an object")? {
            let at = format!("groups.{name}");
            if row.as_object().is_none_or(|r| r.len() != GROUP_FIELDS.len()) {
                return Err(format!("parts {at}: the fields {GROUP_FIELDS:?}"));
            }
            let number = |f: &str| {
                row[f].as_f64().filter(|v| *v >= 0.0).ok_or(format!("parts {at}.{f}: not a number of 0 or more"))
            };
            let classes = row["classes"].as_array().ok_or(format!("parts {at}.classes: not a list"))?;
            groups.push(Group {
                name: name.clone(),
                classes: classes.iter().filter_map(|c| c.as_str().map(String::from)).collect(),
                look: row["look"].as_str().ok_or(format!("parts {at}.look: not a string"))?.to_string(),
                collar_max_m: number("collar_max_m")?,
                wing_max_m: number("wing_max_m")?,
                least_height_m: number("least_height_m")?,
                rock: row["rock"].as_bool().ok_or(format!("parts {at}.rock: not true or false"))?,
                bore: match row["bore"].as_str() {
                    Some("road") => Bore::Road,
                    Some("track") => Bore::Track,
                    Some("rock") => Bore::Rock,
                    _ => return Err(format!("parts {at}.bore: not road, track or rock")),
                },
            });
        }
        let mut seen = HashSet::new();
        if let Some(class) = groups.iter().flat_map(|g| &g.classes).find(|c| !seen.insert(c.as_str())) {
            return Err(format!("parts groups: the class {class} is in two groups"));
        }
        Ok(Ladder { groups, bake })
    }

    /// The group of a road class.
    pub fn group(&self, class: &str) -> Option<usize> {
        self.groups.iter().position(|g| g.classes.iter().any(|c| c == class))
    }
}

impl Rules {
    /// The rules of `data/trailview/parts.json` as this build holds it.
    pub fn embedded() -> &'static Rules {
        static RULES: OnceLock<Rules> = OnceLock::new();
        RULES.get_or_init(|| Rules::parse(include_str!("../../../data/trailview/parts.json")).expect("parts.json"))
    }

    /// The rules of the text of a table, or an error that names the field that breaks one.
    pub fn parse(text: &str) -> Result<Rules, String> {
        let table: Value = serde_json::from_str(text).map_err(|e| format!("parts: {e}"))?;
        let ladder = Ladder::parse(&table)?;
        let v = numbers(&table["design"], &DESIGN_FIELDS, "design")?;
        if v[3] < 1.0 || v[3].fract() != 0.0 {
            return Err("parts design.arch_segments: not a whole number of 1 or more".into());
        }
        let design = Design {
            frame_m: v[0],
            recess_m: v[1],
            face_m: v[2],
            arch_segments: v[3] as usize,
            spring: v[4],
            lamp_spacing_m: v[5],
            lamp_m: v[6],
            kerb_m: v[7],
            daylight_m: v[8],
            void_m: v[9],
            portal_cells: v[10],
            mesh_texels: v[11],
            sleeper_m: v[12],
            reach_m: v[13],
            lift_m: v[14],
            mitre_max: v[15],
            approach_m: v[16],
            wing_slope: v[17],
        };
        let mut looks = BTreeMap::new();
        for (name, row) in table["looks"].as_object().ok_or("parts looks: not an object")? {
            let c = colours(row, &LOOK_FIELDS, &format!("looks.{name}"))?;
            looks.insert(name.clone(), Look { face: c[0], frame: c[1], coping: c[2], shell: c[3] });
        }
        if let Some(g) = ladder.groups.iter().find(|g| !looks.contains_key(&g.look)) {
            return Err(format!("parts groups.{}.look: no look {}", g.name, g.look));
        }
        let c = colours(&table["inside"], &INSIDE_FIELDS, "inside")?;
        let inside = Inside {
            tiles: c[0],
            arch_lining: c[1],
            lining: c[2],
            walk: c[3],
            rock: c[4],
            rock_floor: c[5],
            lamp: c[6],
            void: c[7],
        };
        Ok(Rules { ladder, design, looks, inside })
    }
}
