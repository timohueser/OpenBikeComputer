//! Travel direction and barrier rules that the planner and the device graph share.

/// The direction that a one-way rule leaves open for a mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Oneway {
    Both,
    Forward,
    Backward,
    /// The direction changes over the day; the planner reports it as a closure.
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
    let road_direction = match road_oneway(&get) {
        Oneway::Forward => Some(false),
        Oneway::Backward => Some(true),
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
                    "cycleway:left" => reversed != left_hand,
                    "cycleway:right" => reversed == left_hand,
                    _ => true,
                },
                |direction| reversed == direction,
            ),
        }
    })
}

/// What a `barrier=*` value lets through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Barrier {
    /// No barrier, or one that every mode passes.
    Open,
    /// A walker passes; a bike must be lifted over or through.
    FootOnly,
    /// A value the rules do not know.
    Unknown,
}

pub fn barrier(value: Option<&str>) -> Barrier {
    match value {
        None
        | Some(
            "no" | "entrance" | "bollard" | "gate" | "lift_gate" | "swing_gate" | "cycle_barrier" | "toll_booth"
            | "border_control" | "cattle_grid" | "kerb" | "block" | "chain" | "height_restrictor",
        ) => Barrier::Open,
        Some("stile" | "kissing_gate" | "turnstile") => Barrier::FootOnly,
        Some(_) => Barrier::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oneway_binds_riders_unless_a_lane_or_tag_frees_them() {
        let rule =
            |tags: &[(&str, &str)], mode| oneway(|key| tags.iter().find(|(k, _)| *k == key).map(|(_, v)| *v), mode);
        assert_eq!(rule(&[("oneway", "yes")], "bicycle"), Oneway::Forward);
        assert_eq!(rule(&[("oneway", "yes")], "foot"), Oneway::Both);
        assert_eq!(rule(&[("junction", "roundabout")], "bicycle"), Oneway::Forward);
        assert_eq!(rule(&[("junction", "roundabout"), ("oneway", "no")], "bicycle"), Oneway::Both);
        assert_eq!(rule(&[("junction", "circular")], "bicycle"), Oneway::Both);
        assert_eq!(rule(&[("junction", "circular"), ("oneway", "yes")], "bicycle"), Oneway::Forward);
        assert_eq!(rule(&[("junction", "circular"), ("oneway", "no")], "bicycle"), Oneway::Both);
        assert_eq!(rule(&[("oneway", "alternating")], "bicycle"), Oneway::Both);
        assert_eq!(rule(&[("oneway", "-1"), ("oneway:bicycle", "no")], "bicycle"), Oneway::Both);
        assert_eq!(rule(&[("oneway", "yes"), ("cycleway:left", "opposite_lane")], "bicycle"), Oneway::Both);
        assert_eq!(rule(&[("oneway", "yes"), ("cycleway:left", "lane")], "bicycle"), Oneway::Forward);
    }

    #[test]
    fn a_lane_without_a_direction_runs_with_the_traffic_on_its_side() {
        let get = |key: &str| (key == "cycleway:left").then_some("lane");
        assert_eq!([false, true].map(|reversed| cycleway(get, reversed, false)), [false, true]);
        assert_eq!([false, true].map(|reversed| cycleway(get, reversed, true)), [true, false]);
    }
}
