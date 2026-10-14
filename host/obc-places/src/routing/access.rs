//! Mode access, one-way rules and cycle facilities shared by the device and planner importers.
use std::collections::BTreeMap;

pub const BIKE: u8 = 1;
pub const FOOT: u8 = 2;
pub const PUSH: u8 = 4;

/// An access restriction whose meaning the caller can display or conservatively exclude.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Restriction {
    Permit,
    Private,
    Farm,
    Sidepath,
    Discouraged,
    Limited,
    Unclear,
}

/// What an access value tells the router. Only `no` blocks a mode; every other restriction stays
/// routable, and the route reports it as a closure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    Open,
    Uncertain(Restriction),
    Closed,
}

/// A list such as `agricultural;forestry` takes its most open value.
pub fn classify(value: &str) -> Access {
    let one = |value: &str| match value.trim() {
        // `mtb` designates a way for mountain bikes, as in Switzerland.
        "yes" | "designated" | "official" | "permissive" | "mtb" | "optional_sidepath" => Access::Open,
        // `dismount` closes riding only; pushing follows foot access.
        "no" | "dismount" => Access::Closed,
        "permit" => Access::Uncertain(Restriction::Permit),
        "private" => Access::Uncertain(Restriction::Private),
        "agricultural" | "forestry" => Access::Uncertain(Restriction::Farm),
        "use_sidepath" => Access::Uncertain(Restriction::Sidepath),
        "discouraged" => Access::Uncertain(Restriction::Discouraged),
        "destination" | "customers" | "delivery" | "residents" | "military" | "psv" | "bus" | "emergency" | "hgv"
        | "taxi" | "motor_vehicle" | "motorcar" => Access::Uncertain(Restriction::Limited),
        _ => Access::Uncertain(Restriction::Unclear),
    };
    value.split(';').map(one).min().unwrap_or(Access::Closed)
}

pub fn granted(value: &str) -> bool {
    classify(value) == Access::Open
}

/// Road class and default access, before explicit tags and one-way rules. The modes are the
/// worldwide defaults, unless the way's country differs.
pub fn highway_access(highway: &str, country_defaults: Option<u8>) -> Option<(u8, u8)> {
    let (class, modes) = match highway {
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
    };
    Some((class, country_defaults.unwrap_or(modes)))
}

fn access_keys(mode: &str) -> &'static [&'static str] {
    match mode {
        "bicycle" => &["access", "vehicle", "bicycle"],
        "bicycle:pushing" => &["bicycle:pushing"],
        _ => &["access", "foot"],
    }
}

pub fn inherited<'a>(get: &impl Fn(&str) -> Option<&'a str>, mode: &str, direction: &str) -> Option<&'a str> {
    let mut result = None;
    for key in access_keys(mode) {
        result = get(key).or(result);
        result = get(&format!("{key}:{direction}")).or(result);
    }
    result
}

/// The direction that a one-way rule leaves open for a mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Oneway {
    Both,
    Forward,
    Backward,
    /// The direction changes over the day; the route reports it as a closure.
    Reversible,
    Unknown,
}

fn oneway_value(value: &str) -> Oneway {
    match value {
        "yes" | "1" | "true" => Oneway::Forward,
        "-1" | "reverse" => Oneway::Backward,
        // Traffic takes turns, as on a one-lane bridge with lights.
        "no" | "0" | "false" | "alternating" => Oneway::Both,
        "reversible" => Oneway::Reversible,
        _ => Oneway::Unknown,
    }
}

/// The road's own one-way rule; a roundabout implies one.
fn road_oneway<'a>(get: &impl Fn(&str) -> Option<&'a str>) -> Oneway {
    let implied = get("junction") == Some("roundabout");
    oneway_value(get("oneway").unwrap_or(if implied { "yes" } else { "no" }))
}

/// The one-way rule for `mode` ("bicycle" or "foot"). `oneway` binds riders only, and not where a
/// cycle lane runs against it.
pub fn oneway<'a>(get: impl Fn(&str) -> Option<&'a str>, mode: &str) -> Oneway {
    if mode == "foot" {
        return get("oneway:foot").map_or(Oneway::Both, oneway_value);
    }
    if let Some(value) = get("oneway:bicycle") {
        return oneway_value(value);
    }
    match road_oneway(&get) {
        // A lane on a one-way road follows the road, so the side of traffic does not matter.
        Oneway::Forward if cycleway(&get, true, false) => Oneway::Both,
        Oneway::Backward if cycleway(&get, false, false) => Oneway::Both,
        oneway => oneway,
    }
}

/// Whether a cycle lane runs in the travel direction. A lane on a two-way road without a direction
/// runs with the traffic on its side of the road.
pub fn cycleway<'a>(get: impl Fn(&str) -> Option<&'a str>, reversed: bool, left_hand: bool) -> bool {
    cycleways(get, reversed, left_hand).next().is_some()
}

/// The side and value of mapped cycle facilities that run in the travel direction.
/// Side-specific tags override `both` and unsided tags, including an explicit absence.
pub fn cycleways<'a>(
    get: impl Fn(&str) -> Option<&'a str>,
    reversed: bool,
    left_hand: bool,
) -> impl Iterator<Item = (&'static str, &'a str)> {
    let road_direction = match road_oneway(&get) {
        Oneway::Forward => Some(false),
        Oneway::Backward => Some(true),
        _ => None,
    };
    [("cycleway:left", "cycleway:left:oneway"), ("cycleway:right", "cycleway:right:oneway")]
        .map(|(key, oneway_key)| {
            let value = get(key).or_else(|| get("cycleway:both")).or_else(|| get("cycleway")).unwrap_or("");
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
                return None;
            }
            let follows =
                match get(oneway_key).or_else(|| get("cycleway:both:oneway")).or_else(|| get("cycleway:oneway")) {
                    Some("no" | "0" | "false") => true,
                    Some("yes" | "1" | "true") => !reversed,
                    Some("-1" | "reverse") => reversed,
                    Some(_) => false,
                    None if value.starts_with("opposite") => reversed != road_direction.unwrap_or(false),
                    None => road_direction.map_or(
                        match key {
                            "cycleway:left" => reversed != left_hand,
                            "cycleway:right" => reversed == left_hand,
                            _ => true,
                        },
                        |direction| reversed == direction,
                    ),
                };
            follows.then_some((key, value))
        })
        .into_iter()
        .flatten()
}

/// Pushing follows pedestrian access, unless explicitly restricted.
/// With `routing`, an uncertain value opens its mode, and the route reports it as a closure.
pub fn access<'a>(get: impl Fn(&str) -> Option<&'a str>, defaults: u8, direction: &str, routing: bool) -> u8 {
    let opens = |value: &str| match classify(value) {
        Access::Open => true,
        Access::Uncertain(_) => routing,
        Access::Closed => false,
    };
    let restricted_road =
        get("motorroad") == Some("yes") || matches!(get("highway"), Some("motorway" | "motorway_link"));
    let mut result = if restricted_road { 0 } else { defaults };
    for (mode, bits) in [("foot", FOOT | PUSH), ("bicycle", BIKE)] {
        if let Some(value) = inherited(&get, mode, direction) {
            if opens(value) {
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
        if opens(value) {
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

/// Modes with a conditional restriction. The router keeps them open; the map shows them.
pub fn conditional_modes<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>) -> u8 {
    let tags: BTreeMap<_, _> = tags.collect();
    conditions(tags.iter().map(|(key, value)| (*key, *value)))
        .filter(|(_, _, clauses)| !clauses.is_empty())
        .fold(0, |modes, (key, bits, _)| modes | unshadowed_modes(&tags, key, bits, &["forward", "backward"]))
}

/// A more specific mode or direction overrides a conditional rule, even with an unconditional tag.
pub fn unshadowed_modes(tags: &BTreeMap<&str, &str>, key: &str, modes: u8, directions: &[&str]) -> u8 {
    effective_condition(&|key| tags.get(key).copied(), key, modes, directions)
}

/// The modes a conditional key still controls after more specific unconditional overrides.
pub fn effective_condition<'a>(
    get: &impl Fn(&str) -> Option<&'a str>,
    key: &str,
    modes: u8,
    directions: &[&str],
) -> u8 {
    if key.starts_with("oneway") {
        return if key == "oneway" && get("oneway:bicycle").is_some() { 0 } else { modes };
    }
    let (key, direction) = key
        .rsplit_once(':')
        .filter(|(_, direction)| matches!(*direction, "forward" | "backward"))
        .map_or((key, None), |(key, direction)| (key, Some(direction)));
    let primary = if key == "bicycle:pushing" {
        &[("bicycle:pushing", PUSH)][..]
    } else {
        &[("bicycle", BIKE), ("foot", FOOT | PUSH)][..]
    };
    let mut result = 0;
    for &travel in directions {
        if direction.is_some_and(|direction| direction != travel) {
            continue;
        }
        for &(mode, bits) in primary {
            let keys = access_keys(mode);
            let Some(index) = keys.iter().position(|&candidate| candidate == key) else { continue };
            let overridden =
                keys[index + 1..].iter().any(|key| get(key).is_some() || get(&format!("{key}:{travel}")).is_some())
                    || direction.is_none() && get(&format!("{key}:{travel}")).is_some();
            if !overridden {
                result |= modes & bits;
            }
        }
    }
    result
}

/// The key, modes and `(value, condition)` clauses of each conditional tag. A hazardous-load
/// clause never concerns a rider or walker, so it is left out.
pub fn conditions<'a, I: Iterator<Item = (&'a str, &'a str)>>(
    tags: I,
) -> impl Iterator<Item = (&'a str, u8, Vec<(&'a str, &'a str)>)> + use<'a, I> {
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
        let clauses = value
            .split(';')
            .map(|clause| {
                let (value, condition) = clause.split_once('@').unwrap_or((clause, ""));
                (value.trim(), condition.trim().trim_matches(['(', ')']).trim())
            })
            .filter(|(_, condition)| !hazmat(condition))
            .collect();
        Some((key, modes, clauses))
    })
}

/// A hazardous-load condition never concerns a rider or walker.
fn hazmat(condition: &str) -> bool {
    matches!(condition, "hazmat" | "hazmat:water")
}

/// The modes that pass a barrier, or `None` for a barrier the router does not know.
pub fn barrier(value: Option<&str>) -> Option<u8> {
    match value {
        None
        | Some(
            "no" | "entrance" | "bollard" | "gate" | "lift_gate" | "swing_gate" | "cycle_barrier" | "toll_booth"
            | "border_control" | "cattle_grid" | "kerb" | "block" | "chain" | "height_restrictor",
        ) => Some(BIKE | FOOT | PUSH),
        Some("stile" | "kissing_gate" | "turnstile") => Some(FOOT),
        Some(_) => None,
    }
}
