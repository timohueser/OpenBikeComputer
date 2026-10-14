//! Road exposure preferences from mapped infrastructure, speed limits and lane counts.
use crate::{
    country::Country,
    source::{self, Tags},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Protection {
    None,
    Lane,
    Track,
}

pub fn protection(tags: &Tags, reversed: bool, left_hand: bool) -> Protection {
    source::cycleways(|key| tags.get(key).map(String::as_str), reversed, left_hand)
        .filter(|(side, _)| !conditional(tags, side))
        .map(|(_, value)| match value {
            "track" | "opposite_track" => Protection::Track,
            "lane" | "opposite_lane" => Protection::Lane,
            _ => Protection::None,
        })
        .max()
        .unwrap_or(Protection::None)
}

/// Without a departure time, conditional infrastructure cannot promise protection.
fn conditional(tags: &Tags, side: &str) -> bool {
    let (facility, direction) = match side {
        "cycleway:left" => (
            ("cycleway:left", "cycleway:left:conditional"),
            ("cycleway:left:oneway", "cycleway:left:oneway:conditional"),
        ),
        _ => (
            ("cycleway:right", "cycleway:right:conditional"),
            ("cycleway:right:oneway", "cycleway:right:oneway:conditional"),
        ),
    };
    let unresolved = |keys: &[(&str, &str)]| {
        for (key, condition) in keys {
            if tags.get(*condition).is_some_and(|value| !value.trim().is_empty()) {
                return true;
            }
            if tags.contains_key(*key) {
                return false;
            }
        }
        false
    };
    unresolved(&[facility, ("cycleway:both", "cycleway:both:conditional"), ("cycleway", "cycleway:conditional")])
        || unresolved(&[
            direction,
            ("cycleway:both:oneway", "cycleway:both:oneway:conditional"),
            ("cycleway:oneway", "cycleway:oneway:conditional"),
        ])
}

/// A bounded preference estimate, not a measured traffic volume or collision probability.
pub fn stress(tags: &Tags, reversed: bool, country: Country) -> f64 {
    let get = |key: &str| tags.get(key).map(String::as_str).unwrap_or("");
    let highway = get("highway");
    let (default_speed, context) = match highway.strip_suffix("_link").unwrap_or(highway) {
        "motorway" | "trunk" => (90.0, 0.2),
        "primary" => (70.0, 0.15),
        "secondary" => (60.0, 0.1),
        "tertiary" => (50.0, 0.05),
        "unclassified" => (50.0, 0.0),
        "residential" => (30.0, 0.0),
        "service" => (20.0, 0.0),
        "living_street" => (10.0, 0.0),
        _ => return 0.0,
    };
    if get("route") == "ferry" {
        return 0.0;
    }
    let (speed_key, lanes_key) =
        if reversed { ("maxspeed:backward", "lanes:backward") } else { ("maxspeed:forward", "lanes:forward") };
    let speed = speed(get(speed_key)).or_else(|| speed(get("maxspeed"))).unwrap_or(default_speed);
    let directional_lanes = get(lanes_key).parse::<u8>().ok();
    let lanes = match directional_lanes {
        Some(3..) => 0.2,
        Some(2) => 0.15,
        Some(_) => 0.0,
        None => match get("lanes").parse::<u8>() {
            Ok(4..) => 0.15,
            Ok(3) => 0.08,
            _ => 0.0,
        },
    };
    let exposure = ((speed - 20.0).max(0.0) / 100.0 + context + lanes).clamp(0.0, 1.0);
    exposure
        * match protection(tags, reversed, country.left_hand()) {
            Protection::None => 1.0,
            Protection::Lane => 0.7,
            Protection::Track => 0.25,
        }
}

fn speed(value: &str) -> Option<f64> {
    let value = value.trim();
    let (number, scale) = if let Some(value) = value.strip_suffix("mph") {
        (value, 1.609_344)
    } else {
        (value.strip_suffix("km/h").unwrap_or(value), 1.0)
    };
    let speed = number.trim().parse::<f64>().ok()? * scale;
    (speed.is_finite() && speed > 0.0 && speed <= 300.0).then_some(speed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> Tags {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn protection_follows_side_direction_and_explicit_overrides() {
        let right = tags(&[("cycleway:right", "track")]);
        for left_hand in [false, true] {
            assert_eq!(protection(&right, left_hand, left_hand), Protection::Track);
            assert_eq!(protection(&right, !left_hand, left_hand), Protection::None);
        }
        let mixed = tags(&[("cycleway", "track"), ("cycleway:right", "lane"), ("cycleway:left", "no")]);
        assert_eq!(protection(&mixed, false, false), Protection::Lane);
        assert_eq!(protection(&mixed, true, false), Protection::None);
        let bidirectional = tags(&[("cycleway:left", "track"), ("cycleway:left:oneway", "no")]);
        for reversed in [false, true] {
            assert_eq!(protection(&bidirectional, reversed, false), Protection::Track);
        }
        for (oneway, reversed) in [("yes", false), ("-1", true)] {
            let lane = tags(&[("oneway", oneway), ("cycleway:left", "lane")]);
            assert_eq!(protection(&lane, reversed, false), Protection::Lane);
            assert_eq!(protection(&lane, !reversed, false), Protection::None);
            let opposite = tags(&[("oneway", oneway), ("cycleway", "opposite_track")]);
            assert_eq!(protection(&opposite, reversed, false), Protection::None);
            assert_eq!(protection(&opposite, !reversed, false), Protection::Track);
        }
        let explicit = tags(&[("cycleway:right", "lane"), ("cycleway:right:oneway", "-1")]);
        assert_eq!(protection(&explicit, false, false), Protection::None);
        assert_eq!(protection(&explicit, true, false), Protection::Lane);
        for value in ["yes", "shared_lane", "share_busway", "shoulder", "separate"] {
            assert_eq!(protection(&tags(&[("cycleway", value)]), false, false), Protection::None);
        }
    }

    #[test]
    fn conditional_facilities_do_not_promise_protection_or_change_access() {
        let mut source = tags(&[
            ("oneway", "yes"),
            ("cycleway:right", "track"),
            ("cycleway:right:oneway", "no"),
            ("cycleway:right:conditional", "no @ (wet)"),
        ]);
        for reversed in [false, true] {
            assert_eq!(protection(&source, reversed, false), Protection::None);
        }
        assert_eq!(source::oneway(|key| source.get(key).map(String::as_str), "bicycle"), source::Oneway::Both);
        source.remove("cycleway:right:conditional");
        source.insert("cycleway:right:oneway:conditional".into(), "yes @ (Mo-Fr)".into());
        assert_eq!(protection(&source, true, false), Protection::None);
        source.remove("cycleway:right:oneway:conditional");
        source.insert("cycleway:conditional".into(), "no @ (wet)".into());
        source.insert("cycleway:oneway:conditional".into(), "yes @ (Mo-Fr)".into());
        // The unconditional side-specific tags override less-specific conditional values.
        assert_eq!(protection(&source, true, false), Protection::Track);
        source.insert("oneway:bicycle".into(), "-1".into());
        assert_eq!(source::oneway(|key| source.get(key).map(String::as_str), "bicycle"), source::Oneway::Backward);
    }

    #[test]
    fn exposure_uses_speed_lanes_and_direction_without_guessing_unknown_values() {
        let mut source = tags(&[("highway", "primary"), ("maxspeed", "30")]);
        let low = stress(&source, false, Country::default());
        source.insert("maxspeed".into(), "60 mph".into());
        let fast = stress(&source, false, Country::default());
        assert!(fast > low);
        source.insert("lanes".into(), "4".into());
        let wide = stress(&source, false, Country::default());
        assert!(wide > fast);
        source.insert("cycleway:right".into(), "lane".into());
        let lane = stress(&source, false, Country::default());
        source.insert("cycleway:right".into(), "track".into());
        assert!(stress(&source, false, Country::default()) < lane && lane < wide);
        assert_eq!(stress(&source, true, Country::default()), wide);
        source.insert("maxspeed:backward".into(), "20".into());
        assert!(stress(&source, true, Country::default()) < wide);
        assert_eq!(speed("50 km/h"), Some(50.0));
        assert_eq!(speed("30 mph"), Some(48.28032));
        for unknown in ["signals", "DE:urban", "50;70", "NaN", "inf", "-30", "0"] {
            assert_eq!(speed(unknown), None, "{unknown}");
        }
        for highway in ["track", "path", "cycleway", "steps"] {
            assert_eq!(stress(&tags(&[("highway", highway), ("maxspeed", "90")]), false, Country::default()), 0.0);
        }
    }
}
