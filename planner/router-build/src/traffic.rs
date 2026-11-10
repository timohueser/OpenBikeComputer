//! Road exposure preferences from mapped cycle facilities, speed limits and lane counts.
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

/// The best cycle facility in the travel direction.
pub fn protection(tags: &Tags, reversed: bool, country: Country) -> Protection {
    // `source::cycleway` sees only the facility values of one protection level.
    let runs = |values: &[&str]| {
        source::cycleway(
            |key| {
                let value = tags.get(key);
                match key {
                    "cycleway" | "cycleway:left" | "cycleway:right" | "cycleway:both" => {
                        value.filter(|value| values.contains(value))
                    }
                    _ => value,
                }
            },
            reversed,
            country.left_hand(),
        )
    };
    if runs(&["track", "opposite_track"]) {
        Protection::Track
    } else if runs(&["lane", "opposite_lane"]) {
        Protection::Lane
    } else {
        Protection::None
    }
}

/// A bounded preference from 0 to 1, not a measured traffic volume or collision risk.
pub fn stress(tags: &Tags, reversed: bool, country: Country) -> f64 {
    let get = |key: &str| tags.get(key).unwrap_or("");
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
    let lanes = match get(lanes_key).parse::<u8>() {
        Ok(3..) => 0.2,
        Ok(2) => 0.15,
        Ok(_) => 0.0,
        Err(_) => match get("lanes").parse::<u8>() {
            Ok(4..) => 0.15,
            Ok(3) => 0.08,
            _ => 0.0,
        },
    };
    let exposure = ((speed - 20.0).max(0.0) / 100.0 + context + lanes).clamp(0.0, 1.0);
    exposure
        * match protection(tags, reversed, country) {
            Protection::None => 1.0,
            Protection::Lane => 0.7,
            Protection::Track => 0.25,
        }
}

/// A numeric limit in km/h or mph; a zone code such as `DE:urban` is unknown.
fn speed(value: &str) -> Option<f64> {
    let value = value.trim();
    let (number, scale) = match value.strip_suffix("mph") {
        Some(number) => (number, 1.609_344),
        None => (value.strip_suffix("km/h").unwrap_or(value), 1.0),
    };
    let speed = number.trim().parse::<f64>().ok()? * scale;
    (speed.is_finite() && speed > 0.0 && speed <= 300.0).then_some(speed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> Tags<'static> {
        Tags::from_pairs(pairs.iter().copied())
    }

    #[test]
    fn exposure_follows_speed_lanes_facilities_and_direction() {
        let world = Country::default();
        // A later pair replaces the value of its key.
        let mut pairs = vec![("highway", "primary"), ("maxspeed", "30")];
        let low = stress(&tags(&pairs), false, world);
        pairs.push(("maxspeed", "60 mph"));
        let fast = stress(&tags(&pairs), false, world);
        assert!(fast > low);
        pairs.push(("lanes", "4"));
        let wide = stress(&tags(&pairs), false, world);
        assert!(wide > fast);
        pairs.push(("cycleway:right", "lane"));
        let lane = stress(&tags(&pairs), false, world);
        pairs.push(("cycleway:right", "track"));
        assert!(stress(&tags(&pairs), false, world) < lane && lane < wide);
        // The track runs on the right, so it does not protect the opposite direction.
        assert_eq!(stress(&tags(&pairs), true, world), wide);
        pairs.push(("maxspeed:backward", "20"));
        assert!(stress(&tags(&pairs), true, world) < wide);
        assert_eq!(speed("30 mph"), Some(48.28032));
        for unknown in ["signals", "DE:urban", "50;70", "NaN", "inf", "-30", "0"] {
            assert_eq!(speed(unknown), None, "{unknown}");
        }
        for highway in ["track", "path", "cycleway"] {
            assert_eq!(stress(&tags(&[("highway", highway), ("maxspeed", "90")]), false, world), 0.0);
        }
    }
}
