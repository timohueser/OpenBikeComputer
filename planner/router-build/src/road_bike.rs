//! OSM preferences adapted from BRouter fastbike, trekking and gravel (see LICENSE.brouter).
use crate::country::Country;
use crate::source::{self, Oneway, Tags};
use crate::{suitability::Suitability, traffic};
use planner_router::{
    cost::CostBasis,
    model::{Road, RoadBike},
};

pub fn tag<'a>(tags: &'a Tags, key: &str) -> &'a str {
    tags.get(key).map(String::as_str).unwrap_or("")
}

pub fn way(road: &Road, tags: &Tags, country: Country, variant: RoadBike, pushing: bool) -> Option<Suitability> {
    let highway = tag(tags, "highway");
    let ferry = tag(tags, "route") == "ferry";
    if matches!(highway, "motorway" | "motorway_link" | "construction" | "proposed" | "abandoned")
        || crate::suitability::requires_equipment(tags)
    {
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
    let get = |key: &str| tags.get(key).map(String::as_str);
    let cycleway = source::cycleway(get, road.reversed, country.left_hand());
    let designated = tag(tags, "bicycle") == "designated" || tag(tags, "bicycle_road") == "yes";
    let mut factor: f64 = match highway {
        "trunk" | "trunk_link" | "primary" | "primary_link" | "secondary" | "secondary_link" | "tertiary"
        | "tertiary_link" => 1.0,
        "unclassified" => {
            if unpaved {
                10.0
            } else {
                1.0
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
        "cycleway" => 1.0,
        "residential" | "living_street" | "service" => {
            if unpaved {
                10.0
            } else {
                1.0
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
        "sett" => 1.5,
        "cobblestone" | "unhewn_cobblestone" => 2.0,
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
        if road.hiking_difficulty.is_some_and(|difficulty| difficulty >= 2) {
            factor *= 1.5;
        }
    }
    if pushing {
        let wrong_way = match source::oneway(get, "bicycle") {
            Oneway::Forward => road.reversed,
            Oneway::Backward => !road.reversed,
            _ => false,
        };
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
    let mut minimum_factor = factor;
    if !pushing && !ferry {
        let stress = traffic::stress(tags, road.reversed, country);
        minimum_factor *= 1.0 + stress;
        factor *= 1.0 + 2.0 * stress;
    }
    Some(Suitability {
        basis: CostBasis {
            factor,
            turn: if tag(tags, "junction") == "roundabout" {
                0.0
            } else if variant == RoadBike::Shorter {
                30.0
            } else {
                90.0
            },
            ferry,
        },
        minimum_factor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use planner_router::model::{Point, Surface, BIKE, FOOT, PUSH};

    /// Tests use the worldwide rules.
    fn way(road: &Road, tags: &Tags, variant: RoadBike, pushing: bool) -> Option<CostBasis> {
        super::way(road, tags, Country::default(), variant, pushing).map(|suitability| suitability.basis)
    }

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
    fn surface_context_and_track_quality_survive_traffic_preferences() {
        for (highway, factor) in [
            ("trunk", 2.8),
            ("primary", 2.3),
            ("secondary", 2.0),
            ("tertiary", 1.7),
            ("unclassified", 1.6),
            ("residential", 1.2),
            ("service", 1.0),
            ("cycleway", 1.0),
            ("path", 5.0),
            ("steps", 120.0),
        ] {
            let source = tags(&[("highway", highway)]);
            assert!(
                (way(&road(), &source, RoadBike::Balanced, false).unwrap().factor - factor).abs() < 1e-9,
                "{highway}"
            );
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
        assert!(
            (way(&road(), &tags(&[("highway", "residential"), ("surface", "sett")]), RoadBike::Balanced, false)
                .unwrap()
                .factor
                - 1.8)
                .abs()
                < 1e-9
        );
        assert!(way(&road(), &tags(&[("highway", "path"), ("smoothness", "impassable")]), RoadBike::Balanced, false)
            .is_none());
    }

    #[test]
    fn pushing_costs_a_fixed_charge_and_more_against_a_one_way() {
        let primary = tags(&[("highway", "primary"), ("oneway", "yes")]);
        assert_eq!(way(&road(), &primary, RoadBike::Balanced, false).unwrap().factor, 2.3);
        assert_eq!(way(&road(), &primary, RoadBike::Balanced, true).unwrap().factor, 6.0);
        let reverse = Road { reversed: true, ..road() };
        assert_eq!(way(&reverse, &primary, RoadBike::Balanced, true).unwrap().factor, 51.0);
        assert_eq!(way(&road(), &primary, RoadBike::Shorter, false).unwrap().turn, 30.0);
    }

    #[test]
    fn protected_paved_road_beats_exposed_road_without_erasing_roughness() {
        let mut primary = tags(&[("highway", "primary"), ("maxspeed", "80"), ("surface", "asphalt")]);
        let exposed = way(&road(), &primary, RoadBike::Balanced, false).unwrap().factor;
        let cycleway =
            way(&road(), &tags(&[("highway", "cycleway"), ("surface", "asphalt")]), RoadBike::Balanced, false)
                .unwrap()
                .factor;
        primary.insert("cycleway:right".into(), "track".into());
        let protected = way(&road(), &primary, RoadBike::Balanced, false).unwrap().factor;
        assert!(cycleway < protected && protected < exposed);
        let reverse = Road { reversed: true, ..road() };
        assert_eq!(way(&reverse, &primary, RoadBike::Balanced, false).unwrap().factor, exposed);
        primary.insert("surface".into(), "gravel".into());
        let rough = super::way(&road(), &primary, Country::default(), RoadBike::Balanced, false).unwrap();
        assert!(rough.minimum_factor >= 10.0 && rough.basis.factor >= rough.minimum_factor);
        let mountain = Road { hiking_difficulty: Some(2), ..road() };
        assert!(
            super::way(&mountain, &primary, Country::default(), RoadBike::Balanced, false).unwrap().minimum_factor
                > rough.minimum_factor
        );
        let cobbles = tags(&[("highway", "cycleway"), ("surface", "cobblestone")]);
        assert!(way(&road(), &cobbles, RoadBike::Balanced, false).unwrap().factor > cycleway);
    }
}
