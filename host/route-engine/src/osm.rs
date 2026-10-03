//! Source OSM data is kept outside the pages used by route searches.
use crate::model::Point;
use crate::model::{BIKE, FOOT, PUSH};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type Tags = BTreeMap<String, String>;

pub fn granted(value: &str) -> bool {
    matches!(value, "yes" | "designated" | "official" | "permissive" | "discouraged")
}

/// German road classes and default access, before explicit tags and one-way rules.
pub fn highway_access(highway: &str) -> Option<(u8, u8)> {
    Some(match highway {
        "cycleway" => (0, BIKE),
        "residential" | "living_street" | "unclassified" | "service" | "tertiary" | "tertiary_link" => {
            (1, BIKE | FOOT | PUSH)
        }
        "trunk" | "trunk_link" | "primary" | "primary_link" | "secondary" | "secondary_link" => (2, BIKE | FOOT | PUSH),
        "motorway" | "motorway_link" => (2, 0),
        "track" | "road" => (3, BIKE | FOOT | PUSH),
        "path" => (4, BIKE | FOOT | PUSH),
        "footway" | "pedestrian" => (4, FOOT | PUSH),
        "bridleway" => (4, 0),
        "steps" => (5, FOOT | PUSH),
        _ => return None,
    })
}

pub fn inherited<'a>(get: &impl Fn(&str) -> Option<&'a str>, mode: &str, direction: &str) -> Option<&'a str> {
    let keys: &[&str] = if mode == "bicycle" { &["access", "vehicle", "bicycle"] } else { &["access", "foot"] };
    let mut result = None;
    for key in keys {
        result = get(key).or(result);
        result = get(&format!("{key}:{direction}")).or(result);
    }
    result
}

/// Cycle lanes use German (right-hand traffic) defaults; explicit directions follow the OSM way.
pub fn cycleway<'a>(get: impl Fn(&str) -> Option<&'a str>, reversed: bool) -> bool {
    let oneway = get("oneway").unwrap_or(if get("junction") == Some("roundabout") { "yes" } else { "no" });
    let road_direction = match oneway {
        "yes" | "1" | "true" => Some(false),
        "-1" | "reverse" => Some(true),
        _ => None,
    };
    ["cycleway", "cycleway:left", "cycleway:right"].iter().any(|key| {
        let both = *key != "cycleway";
        let value = get(key).or_else(|| both.then(|| get("cycleway:both")).flatten()).unwrap_or("");
        if !matches!(
            value,
            "lane"
                | "track"
                | "shared_lane"
                | "share_busway"
                | "shoulder"
                | "opposite"
                | "opposite_lane"
                | "opposite_track"
        ) {
            return false;
        }
        match get(&format!("{key}:oneway")).or_else(|| both.then(|| get("cycleway:both:oneway")).flatten()) {
            Some("no" | "0" | "false") => true,
            Some("yes" | "1" | "true") => !reversed,
            Some("-1" | "reverse") => reversed,
            Some(_) => false,
            None if value.starts_with("opposite") => reversed != road_direction.unwrap_or(false),
            None => road_direction.map_or(
                match *key {
                    "cycleway:left" => reversed,
                    "cycleway:right" => !reversed,
                    _ => true,
                },
                |direction| reversed == direction,
            ),
        }
    })
}

/// Pushing follows pedestrian access in the supported German region, unless explicitly restricted.
pub fn access<'a>(get: impl Fn(&str) -> Option<&'a str>, defaults: u8, direction: &str) -> u8 {
    let restricted_road =
        get("motorroad") == Some("yes") || matches!(get("highway"), Some("motorway" | "motorway_link"));
    let mut result = if restricted_road { 0 } else { defaults };
    for (mode, bits) in [("foot", FOOT | PUSH), ("bicycle", BIKE)] {
        if let Some(value) = inherited(&get, mode, direction) {
            if granted(value) {
                result |= bits;
            } else {
                result &= !bits;
            }
        }
        if restricted_road && !get(&format!("{mode}:{direction}")).or(get(mode)).is_some_and(granted) {
            result &= !bits;
        }
    }
    if !restricted_road
        && inherited(&get, "bicycle", direction) == Some("dismount")
        && get(&format!("foot:{direction}")).or(get("foot")).is_none_or(granted)
    {
        result |= FOOT | PUSH;
    }
    if !restricted_road && inherited(&get, "bicycle", direction).is_none() && get("bicycle_road") == Some("yes") {
        result |= BIKE;
    }
    if let Some(value) = get(&format!("bicycle:pushing:{direction}")).or(get("bicycle:pushing")) {
        if granted(value) {
            result |= PUSH;
        } else {
            result &= !PUSH;
        }
    }
    if result & FOOT == 0 {
        result &= !PUSH;
    }
    result
}

/// Modes whose conditional access cannot be resolved by the regional importer. With
/// `routing`, seasonal closures do not count: the router keeps them open, the map still shows them.
pub fn conditional_modes<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>, routing: bool) -> u8 {
    conditions(tags).filter(|(_, _, seasons)| !routing || seasons.is_none()).fold(0, |modes, (_, m, _)| modes | m)
}

/// The seasonal closures that `conditional_modes` ignores for routing, so that a route can report
/// them. Only a restricting value closes: `yes @ (May-Oct)` names the open season. A seasonal
/// one-way closes one direction only, so it is no closure of the road.
pub fn seasonal_closures<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<(u8, String)> {
    let mut closures: Vec<(u8, String)> = Vec::new();
    for (key, modes, seasons) in conditions(tags) {
        if modes == 0 || key.starts_with("oneway") {
            continue;
        }
        for (_, condition) in seasons.into_iter().flatten().filter(|(value, _)| !granted(value)) {
            match closures.iter_mut().find(|(_, known)| known == condition) {
                Some((closed, _)) => *closed |= modes,
                None => closures.push((modes, condition.to_owned())),
            }
        }
    }
    closures
}

type Seasons<'a> = Option<Vec<(&'a str, &'a str)>>;

/// The key, modes and, when every clause is seasonal, the value and condition of each clause of
/// each conditional tag. A tag whose clauses all concern hazardous loads closes nothing.
fn conditions<'a, I: Iterator<Item = (&'a str, &'a str)>>(
    tags: I,
) -> impl Iterator<Item = (&'a str, u8, Seasons<'a>)> + use<'a, I> {
    tags.filter_map(|(key, value)| {
        let key = key.strip_suffix(":conditional")?;
        let modes = match key {
            "access" | "access:forward" | "access:backward" => BIKE | FOOT | PUSH,
            "vehicle" | "vehicle:forward" | "vehicle:backward" | "bicycle" | "bicycle:forward" | "bicycle:backward"
            | "oneway" | "oneway:bicycle" => BIKE,
            "bicycle:pushing" | "bicycle:pushing:forward" | "bicycle:pushing:backward" => PUSH,
            "foot" | "foot:forward" | "foot:backward" | "oneway:foot" => FOOT | PUSH,
            _ => 0,
        };
        let mut seasons = Vec::new();
        for clause in value.split(';') {
            let Some((value, condition)) = clause.split_once('@') else { return Some((key, modes, None)) };
            let condition = condition.trim().trim_matches(['(', ')']).trim();
            if seasonal(condition) {
                seasons.push((value.trim(), condition));
            } else if !hazmat(condition) {
                return Some((key, modes, None));
            }
        }
        (!seasons.is_empty()).then_some((key, modes, Some(seasons)))
    })
}

/// A hazardous-load condition never concerns a rider or walker.
fn hazmat(condition: &str) -> bool {
    matches!(condition, "hazmat" | "hazmat:water")
}

/// A closure such as `Nov-May` moves with the snow each year; the snow layer tells the rider
/// when the road is usually clear. A dated closure with a year is not seasonal.
fn seasonal(condition: &str) -> bool {
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    const SEASONS: [&str; 4] = ["winter", "spring", "summer", "autumn"];
    let condition = condition.to_ascii_lowercase();
    let mut named = false;
    let dates = condition.split([' ', '-', ',']).filter(|token| !token.is_empty()).all(|token| {
        let day = token.trim_end_matches(char::is_alphabetic);
        if !day.is_empty() {
            let suffix = &token[day.len()..];
            return matches!(suffix, "" | "st" | "nd" | "rd" | "th")
                && day.parse::<u8>().is_ok_and(|d| (1..=31).contains(&d));
        }
        let name = SEASONS.contains(&token) || MONTHS.iter().any(|m| token.starts_with(m));
        named |= name;
        name
    });
    dates && named
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: i64,
    pub point: Point,
    pub tags: Tags,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Way {
    pub id: i64,
    pub nodes: Vec<i64>,
    pub tags: Tags,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Id {
    Node(i64),
    Way(i64),
    Relation(i64),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Relation {
    pub id: i64,
    pub tags: Tags,
    pub members: Vec<(Id, String)>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Data {
    pub nodes: BTreeMap<i64, Node>,
    pub ways: BTreeMap<i64, Way>,
    pub relations: BTreeMap<i64, Relation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seasonal_closures_name_only_restricting_conditions_with_their_modes() {
        let closures = |tags: &[(&'static str, &'static str)]| seasonal_closures(tags.iter().copied());
        assert_eq!(
            closures(&[("bicycle:conditional", "no @ (Nov-May)"), ("foot:conditional", "no @ (Mar 1-Jul 31)")]),
            vec![(BIKE, "Nov-May".into()), (FOOT | PUSH, "Mar 1-Jul 31".into())]
        );
        assert_eq!(
            closures(&[("access:conditional", "no @ (Nov-May)"), ("vehicle:conditional", "no @ (Nov-May)")]),
            vec![(BIKE | FOOT | PUSH, "Nov-May".into())]
        );
        // An open season, a one-way, a motor-vehicle rule and a weather rule close nothing seasonally.
        for tag in [
            ("access:conditional", "yes @ (May-Oct)"),
            ("oneway:conditional", "yes @ (Nov-May)"),
            ("motor_vehicle:conditional", "no @ (Nov-May)"),
            ("access:conditional", "no @ (Nov-May); no @ (wet)"),
        ] {
            assert_eq!(closures(&[tag]), vec![], "{tag:?}");
        }
    }
}
