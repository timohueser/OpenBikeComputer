//! Conservative access for the four device bike profiles, which cannot display access warnings.
use super::access::{self, Oneway, BIKE, FOOT, PUSH};
use super::country::Country;

/// A bit per `BikeType` wire value. `pushing` is a subset of `allowed`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProfileAccess {
    pub allowed: u8,
    pub pushing: u8,
}

impl ProfileAccess {
    pub const OPEN: Self = Self { allowed: 0x0f, pushing: 0 };
    pub const CLOSED: Self = Self { allowed: 0, pushing: 0 };

    fn from_modes(modes: u8, profiles: u8) -> Self {
        if modes & BIKE != 0 {
            Self { allowed: profiles, pushing: 0 }
        } else if modes & PUSH != 0 {
            Self { allowed: profiles, pushing: profiles }
        } else {
            Self::CLOSED
        }
    }
}

const CONDITIONAL: &[&str] = &[
    "access:conditional",
    "access:forward:conditional",
    "access:backward:conditional",
    "vehicle:conditional",
    "vehicle:forward:conditional",
    "vehicle:backward:conditional",
    "bicycle:conditional",
    "bicycle:forward:conditional",
    "bicycle:backward:conditional",
    "foot:conditional",
    "foot:forward:conditional",
    "foot:backward:conditional",
    "bicycle:pushing:conditional",
    "bicycle:pushing:forward:conditional",
    "bicycle:pushing:backward:conditional",
    "oneway:conditional",
    "oneway:bicycle:conditional",
    "oneway:foot:conditional",
];

fn strict_modes<'a>(get: &impl Fn(&str) -> Option<&'a str>, defaults: u8, direction: &str) -> u8 {
    let mut modes = access::access(get, defaults, direction, false);
    for (key, bits, clauses) in access::conditions(CONDITIONAL.iter().filter_map(|key| get(key).map(|v| (*key, v)))) {
        if !clauses.is_empty() {
            modes &= !access::effective_condition(get, key, bits, &[direction]);
        }
    }
    if modes & FOOT == 0 {
        modes &= !PUSH;
    }
    modes
}

/// Directed source-way access, in OSM node order: forward then backward. Country defaults use
/// the first known source coordinate; explicit tags still override those defaults.
pub fn way_access<'a>(
    get: impl Fn(&str) -> Option<&'a str>,
    first_lon_lat: (f64, f64),
    kind: u8,
) -> [ProfileAccess; 2] {
    if kind & 31 == 4 || get("area") == Some("yes") || get("smoothness") == Some("impassable") {
        return [ProfileAccess::CLOSED; 2];
    }
    let highway = get("highway").unwrap_or("");
    let country = Country::at(first_lon_lat);
    let highway = highway.strip_suffix("_link").unwrap_or(highway);
    let Some((_, defaults)) = access::highway_access(highway, country.defaults(highway)) else {
        return [ProfileAccess::CLOSED; 2];
    };
    // The ordinary device profiles allow S0 on road bikes and up to S1 on the other bikes.
    let profiles = match get("mtb:scale") {
        None => 0x0f,
        Some(value) => match value.trim().trim_end_matches(['+', '-']).parse::<u8>() {
            Ok(0) => 0x0f,
            Ok(1) => 0x0e,
            _ => 0,
        },
    };
    if profiles == 0 || !matches!(get("sac_scale"), None | Some("strolling" | "hiking" | "mountain_hiking")) {
        return [ProfileAccess::CLOSED; 2];
    }
    let mut modes = [strict_modes(&get, defaults, "forward"), strict_modes(&get, defaults, "backward")];
    // A conditional cycle facility cannot establish a permanent contraflow exception.
    let conditional_lane = ["cycleway", "cycleway:left", "cycleway:right", "cycleway:both"]
        .iter()
        .any(|key| get(&format!("{key}:conditional")).is_some() || get(&format!("{key}:oneway:conditional")).is_some());
    let direction_tag = |key: &str| if conditional_lane && key.starts_with("cycleway") { None } else { get(key) };
    for (mode, mask) in [("bicycle", BIKE), ("foot", FOOT | PUSH)] {
        match access::oneway(direction_tag, mode) {
            Oneway::Forward => modes[1] &= !mask,
            Oneway::Backward => modes[0] &= !mask,
            Oneway::Both => {}
            Oneway::Reversible | Oneway::Unknown => {
                modes[0] &= !mask;
                modes[1] &= !mask;
            }
        }
    }
    modes.map(|modes| ProfileAccess::from_modes(modes, profiles))
}

/// Access through a source node. Unknown barriers stay closed because the device cannot show
/// the planner's warning; directional permissions must permit both sides of the crossing.
pub fn node_access<'a>(get: impl Fn(&str) -> Option<&'a str>) -> ProfileAccess {
    let Some(defaults) = access::barrier(get("barrier")) else { return ProfileAccess::CLOSED };
    ProfileAccess::from_modes(strict_modes(&get, defaults, "forward") & strict_modes(&get, defaults, "backward"), 0x0f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn way(tags: &[(&str, &str)], coordinate: (f64, f64)) -> [ProfileAccess; 2] {
        let get = |key: &str| tags.iter().find(|(k, _)| *k == key).map(|(_, value)| *value);
        way_access(get, coordinate, super::super::classify(tags.iter().copied()).unwrap())
    }

    #[test]
    fn specific_permissions_override_general_rules_and_pushing_obeys_foot_access() {
        let open = ProfileAccess::OPEN;
        let push = ProfileAccess { allowed: 15, pushing: 15 };
        let closed = ProfileAccess::CLOSED;
        for (tags, expected) in [
            (vec![("access", "no"), ("bicycle", "yes")], open),
            (vec![("bicycle", "no"), ("foot", "yes")], push),
            (vec![("bicycle", "dismount"), ("foot", "no")], closed),
            (vec![("bicycle", "no"), ("foot", "yes"), ("bicycle:pushing", "no")], closed),
            (vec![("access", "private")], closed),
        ] {
            let tags = [("highway", "residential")].into_iter().chain(tags).collect::<Vec<_>>();
            assert_eq!(way(&tags, (8.0, 48.0)), [expected; 2], "{tags:?}");
        }
    }

    #[test]
    fn one_way_riding_and_walking_are_independent_and_contraflow_is_explicit() {
        let open = ProfileAccess::OPEN;
        let push = ProfileAccess { allowed: 15, pushing: 15 };
        let closed = ProfileAccess::CLOSED;
        assert_eq!(way(&[("highway", "residential"), ("oneway", "yes")], (8.0, 48.0)), [open, push]);
        assert_eq!(
            way(&[("highway", "residential"), ("oneway", "-1"), ("oneway:foot", "yes")], (8.0, 48.0)),
            [push, open]
        );
        assert_eq!(way(&[("highway", "cycleway"), ("oneway", "yes")], (8.0, 48.0)), [open, closed]);
        assert_eq!(
            way(&[("highway", "cycleway"), ("oneway", "yes"), ("cycleway", "opposite_lane")], (8.0, 48.0)),
            [open; 2]
        );
        assert_eq!(way(&[("highway", "cycleway"), ("oneway", "reversible")], (8.0, 48.0)), [closed; 2]);
    }

    #[test]
    fn conditional_access_is_closed_only_for_the_unshadowed_modes_and_directions() {
        let open = ProfileAccess::OPEN;
        let closed = ProfileAccess::CLOSED;
        assert_eq!(
            way(&[("highway", "residential"), ("access:conditional", "no @ (wet)"), ("bicycle", "yes")], (8.0, 48.0)),
            [open; 2]
        );
        assert_eq!(
            way(&[("highway", "residential"), ("access:forward:conditional", "no @ (wet)")], (8.0, 48.0)),
            [closed, open]
        );
        assert_eq!(
            way(&[("highway", "residential"), ("access:conditional", "yes @ (May-Oct)")], (8.0, 48.0)),
            [closed; 2]
        );
        assert_eq!(
            way(
                &[
                    ("highway", "cycleway"),
                    ("oneway", "yes"),
                    ("cycleway", "opposite_lane"),
                    ("cycleway:conditional", "no @ (wet)")
                ],
                (8.0, 48.0)
            ),
            [open, closed]
        );
    }

    #[test]
    fn country_and_terrain_caps_apply_before_bike_profiles_reach_the_device() {
        let path = [("highway", "path")];
        assert_eq!(way(&path, (11.4, 47.27)), [ProfileAccess { allowed: 15, pushing: 15 }; 2]);
        assert_eq!(way(&path, (8.0, 48.0)), [ProfileAccess::OPEN; 2]);
        assert_eq!(
            way(&[("highway", "path"), ("mtb:scale", "1+")], (8.0, 48.0)),
            [ProfileAccess { allowed: 14, pushing: 0 }; 2]
        );
        for tag in [("mtb:scale", "2"), ("mtb:scale", "unknown"), ("sac_scale", "demanding_mountain_hiking")] {
            assert_eq!(way(&[("highway", "path"), tag], (8.0, 48.0)), [ProfileAccess::CLOSED; 2]);
        }
        assert_eq!(way(&[("highway", "steps"), ("bicycle", "yes")], (8.0, 48.0)), [ProfileAccess::CLOSED; 2]);
    }

    #[test]
    fn node_barriers_preserve_mode_overrides_but_unknown_or_conditional_crossings_close() {
        for (tags, expected) in [
            (vec![("barrier", "gate"), ("access", "no"), ("bicycle", "yes")], ProfileAccess::OPEN),
            (vec![("barrier", "stile")], ProfileAccess::CLOSED),
            (vec![("barrier", "unknown"), ("bicycle", "yes")], ProfileAccess::CLOSED),
            (vec![("barrier", "gate"), ("access:conditional", "no @ (wet)")], ProfileAccess::CLOSED),
        ] {
            assert_eq!(node_access(|key| tags.iter().find(|(k, _)| *k == key).map(|(_, value)| *value)), expected);
        }
    }
}
