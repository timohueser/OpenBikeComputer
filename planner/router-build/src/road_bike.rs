//! OSM preferences adapted from BRouter fastbike, trekking and gravel (see LICENSE.brouter).
use crate::source::Tags;
use planner_router::{
    cost::CostBasis,
    model::{Road, RoadBike},
};

pub fn tag<'a>(tags: &'a Tags, key: &str) -> &'a str {
    tags.get(key).map(String::as_str).unwrap_or("")
}

pub fn way(road: &Road, tags: &Tags, variant: RoadBike, pushing: bool) -> Option<CostBasis> {
    let highway = tag(tags, "highway");
    let ferry = tag(tags, "route") == "ferry";
    if matches!(highway, "motorway" | "motorway_link" | "construction" | "proposed" | "abandoned") {
        return None;
    }
    let surface = tag(tags, "surface");
    let smoothness = tag(tags, "smoothness");
    let paved = matches!(
        surface,
        "paved" | "asphalt" | "concrete" | "concrete:plates" | "concrete:lanes" | "paving_stones" | "sett"
    ) || matches!(smoothness, "excellent" | "good");
    let explicit_unpaved = matches!(
        surface,
        "compacted"
            | "gravel"
            | "pebblestone"
            | "ground"
            | "dirt"
            | "earth"
            | "grass"
            | "unpaved"
            | "sand"
            | "mud"
            | "rock"
            | "stone"
            | "wood"
            | "woodchips"
            | "grass_paver"
            | "metal"
    );
    let rough = matches!(smoothness, "bad" | "very_bad" | "horrible" | "very_horrible" | "impassable");
    let unpaved = !(paved || matches!(surface, "fine_gravel" | "cobblestone") || smoothness == "intermediate")
        && (explicit_unpaved || rough);
    let cycleway = crate::source::cycleway(|key| tags.get(key).map(String::as_str), road.reversed);
    let designated = tag(tags, "bicycle") == "designated" || tag(tags, "bicycle_road") == "yes";
    let mut factor: f64 = match highway {
        "trunk" | "trunk_link" => 10.0,
        "primary" | "primary_link" => 1.2,
        "secondary" | "secondary_link" => 1.1,
        "tertiary" | "tertiary_link" => 1.0,
        "unclassified" => {
            if unpaved {
                10.0
            } else {
                1.1
            }
        }
        "pedestrian" => {
            if cycleway {
                1.3
            } else {
                5.0
            }
        }
        "steps" => 120.0,
        _ if ferry => 5.67,
        "bridleway" => 5.0,
        "cycleway" => 1.3,
        "residential" | "living_street" | "service" => {
            if unpaved {
                10.0
            } else {
                1.2
            }
        }
        "track" | "road" | "path" | "footway" => match tag(tags, "tracktype") {
            "grade1" => {
                if unpaved {
                    3.0
                } else {
                    1.2
                }
            }
            "grade2" => {
                if unpaved {
                    10.0
                } else {
                    3.0
                }
            }
            "grade3" => 10.0,
            "grade4" => 20.0,
            "grade5" => 30.0,
            _ if designated => {
                if unpaved {
                    3.0
                } else {
                    1.2
                }
            }
            _ if paved => 2.0,
            _ if unpaved => 15.0,
            _ => 5.0,
        },
        _ => 10.0,
    };
    // Explicit poor surfaces also matter on roads whose highway class implies paving.
    let surface_floor: f64 = match surface {
        "compacted" | "fine_gravel" => 3.0,
        "gravel" | "pebblestone" => 10.0,
        "ground" | "dirt" | "earth" | "grass" | "unpaved" => 15.0,
        "sand" | "mud" | "rock" | "stone" => 30.0,
        _ => 1.0,
    };
    let roughness: f64 = match smoothness {
        "excellent" | "" | "unknown" => 1.0,
        "good" => 1.1,
        "intermediate" => 1.3,
        "bad" => 1.5,
        "very_bad" => 3.0,
        "horrible" => 8.0,
        "very_horrible" => 9.4,
        "impassable" => return None,
        _ => 1.0,
    };
    if !ferry {
        factor = factor.max(surface_floor) * roughness;
    }
    if pushing {
        let oneway = tags
            .get("oneway:bicycle")
            .or_else(|| tags.get("oneway"))
            .map(String::as_str)
            .unwrap_or(if tag(tags, "junction") == "roundabout" { "yes" } else { "no" });
        let wrong_way = (road.reversed && matches!(oneway, "yes" | "true" | "1")) || (!road.reversed && oneway == "-1");
        factor += if wrong_way {
            match highway {
                _ if matches!(tag(tags, "junction"), "roundabout" | "circular") => 60.0,
                "primary" | "primary_link" => 50.0,
                "secondary" | "secondary_link" => 30.0,
                "tertiary" | "tertiary_link" => 20.0,
                _ => 6.0,
            }
        } else {
            5.0
        };
    }
    Some(CostBasis {
        factor,
        turn: if tag(tags, "junction") == "roundabout" {
            0.0
        } else if variant == RoadBike::Shorter {
            30.0
        } else {
            90.0
        },
        ferry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use planner_router::model::{Point, Surface, BIKE, FOOT, PUSH};

    fn road() -> Road {
        Road {
            from: 0,
            to: 1,
            way: 1,
            reversed: false,
            length_m: 1000,
            ascent_m: 0,
            descent_m: 0,
            surface: Surface::Unknown,
            class: 1,
            access: BIKE | FOOT | PUSH,
            difficulty: 255,
            hiking_difficulty: None,
            structure: false,
            shape: vec![Point::default(); 2],
        }
    }
    fn tags(pairs: &[(&str, &str)]) -> Tags {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn brouter_highway_and_track_preferences_keep_missing_surface_context() {
        // BRouter fastbike base factors, before terrain, access and traffic penalties.
        for (highway, factor) in [
            ("trunk", 10.0),
            ("primary", 1.2),
            ("secondary", 1.1),
            ("tertiary", 1.0),
            ("unclassified", 1.1),
            ("residential", 1.2),
            ("service", 1.2),
            ("cycleway", 1.3),
            ("path", 5.0),
            ("steps", 120.0),
        ] {
            let source = tags(&[("highway", highway)]);
            assert_eq!(way(&road(), &source, RoadBike::Balanced, false).unwrap().factor, factor, "{highway}");
        }
        for (tracktype, factor) in
            [("grade1", 1.2), ("grade2", 3.0), ("grade3", 10.0), ("grade4", 20.0), ("grade5", 30.0)]
        {
            let source = tags(&[("highway", "track"), ("tracktype", tracktype)]);
            assert_eq!(way(&road(), &source, RoadBike::Balanced, false).unwrap().factor, factor);
        }
        for variant in [RoadBike::Balanced, RoadBike::Shorter, RoadBike::LessClimbing] {
            let unknown = tags(&[("highway", "residential")]);
            let asphalt = tags(&[("highway", "residential"), ("surface", "asphalt")]);
            assert_eq!(
                way(&road(), &unknown, variant, false).unwrap().factor,
                way(&road(), &asphalt, variant, false).unwrap().factor
            );
            let unrecognized = tags(&[("highway", "residential"), ("surface", "unrecognized")]);
            assert_eq!(
                way(&road(), &unknown, variant, false).unwrap().factor,
                way(&road(), &unrecognized, variant, false).unwrap().factor
            );
            let gravel = tags(&[("highway", "tertiary"), ("surface", "gravel")]);
            assert!(way(&road(), &gravel, variant, false).unwrap().factor >= 10.0);
        }
        assert_eq!(
            way(&road(), &tags(&[("highway", "residential"), ("surface", "sett")]), RoadBike::Balanced, false)
                .unwrap()
                .factor,
            1.2
        );
        assert!(way(&road(), &tags(&[("highway", "path"), ("smoothness", "impassable")]), RoadBike::Balanced, false)
            .is_none());
    }

    #[test]
    fn pushing_costs_a_fixed_charge_and_more_against_a_one_way() {
        let primary = tags(&[("highway", "primary"), ("oneway", "yes")]);
        assert_eq!(way(&road(), &primary, RoadBike::Balanced, false).unwrap().factor, 1.2);
        assert_eq!(way(&road(), &primary, RoadBike::Balanced, true).unwrap().factor, 6.2);
        let reverse = Road { reversed: true, ..road() };
        assert_eq!(way(&reverse, &primary, RoadBike::Balanced, true).unwrap().factor, 51.2);
        assert_eq!(way(&road(), &primary, RoadBike::Shorter, false).unwrap().turn, 30.0);
    }
}
