//! Physical suitability of a way for the weighted profiles, from its OSM surface, smoothness,
//! tracktype, difficulty and traffic. The coefficients are preferences, not measured speeds.
use crate::{country::Country, road_bike::tag, source::Tags, traffic};
use planner_router::{
    cost::CostBasis,
    model::{Profile, Road, Surface, Weighting, PUSH},
};

pub struct Suitability {
    pub basis: CostBasis,
    /// The physical and traffic part of `basis.factor`; a signed route cannot discount it.
    pub minimum_factor: f64,
}

#[derive(Clone, Copy)]
enum Family {
    Touring,
    Gravel,
    Mtb,
    Hiking,
}

pub fn way(road: &Road, tags: &Tags, country: Country, profile: &Profile) -> Option<Suitability> {
    let Weighting::Weighted { surface, road: weights, .. } = &profile.weighting else {
        return None;
    };
    if requires_equipment(tags) {
        return None;
    }
    let family = match profile.name.split('/').next().unwrap_or("") {
        _ if profile.walking => Family::Hiking,
        "mtb" => Family::Mtb,
        "gravel" => Family::Gravel,
        _ => Family::Touring,
    };
    let ferry = road.class == 6;
    let pushing = profile.mode(road) == PUSH;
    let mut minimum_factor = 1.0;
    if !ferry {
        let rolling = smoothness(tag(tags, "smoothness"), family)?.max(tracktype(tag(tags, "tracktype"), family));
        let difficulty = if profile.walking {
            let visibility = match tag(tags, "trail_visibility") {
                "intermediate" => 1.1,
                "bad" => 1.4,
                "horrible" => 1.8,
                "no" => 2.5,
                _ => 1.0,
            };
            [1.0, 1.0, 1.3, 1.8, 2.5, 3.5, 5.0][usize::from(road.hiking_difficulty.unwrap_or(0).min(6))] * visibility
        } else {
            let grade = usize::from(road.mtb_scale().unwrap_or(0).min(6));
            let mtb: f64 = match family {
                Family::Touring => [1.0, 1.5, 2.0, 3.0, 4.0, 5.0, 6.0][grade],
                Family::Gravel => [1.0, 1.35, 1.8, 2.5, 3.5, 4.5, 6.0][grade],
                _ => [1.0, 1.15, 1.5, 2.0, 3.0, 4.0, 5.0][grade],
            };
            // SAC grades foot travel, but a mountain hiking path still warns of hard bike passage.
            mtb.max(if road.hiking_difficulty.is_some_and(|d| d >= 2) { 1.5 } else { 1.0 })
        };
        minimum_factor = surface_factor(road, tags, surface) * rolling * difficulty * if pushing { 4.0 } else { 1.0 };
    }
    let mut preference = *weights.get(road.class as usize)?;
    if !profile.walking && !pushing && !ferry {
        // A cycle track or lane takes part of the dislike of a main road away.
        if road.class == 2 {
            let exposure = match traffic::protection(tags, road.reversed, country) {
                traffic::Protection::None => 1.0,
                traffic::Protection::Lane => 0.65,
                traffic::Protection::Track => 0.25,
            };
            preference = 1.0 + (preference - 1.0) * exposure;
        }
        minimum_factor *= 1.0 + 0.6 * traffic::stress(tags, road.reversed, country);
    }
    Some(Suitability {
        basis: CostBasis { factor: minimum_factor * preference.max(1.0), turn: 0.0, ferry },
        minimum_factor,
    })
}

/// A via ferrata needs equipment that no profile carries.
pub fn requires_equipment(tags: &Tags) -> bool {
    tag(tags, "highway") == "via_ferrata"
        || matches!(tag(tags, "via_ferrata"), "yes" | "designated")
        || !matches!(tag(tags, "via_ferrata_scale"), "" | "0" | "no")
}

fn surface_factor(road: &Road, tags: &Tags, weights: &[f64; 6]) -> f64 {
    if road.surface != Surface::Unknown {
        return weights[road.surface as usize];
    }
    use Surface::*;
    // A track grade describes firmness, not material, so the less favourable material counts.
    let plausible: &[Surface] = match tag(tags, "tracktype") {
        "grade1" => &[Paved, Compacted],
        "grade2" => &[Compacted, Gravel],
        "grade3" => &[Gravel, Dirt],
        "grade4" => &[Dirt],
        "grade5" => &[Dirt, Rough],
        _ => match road.class {
            0..=2 => &[Unknown, Paved],
            3 => &[Unknown, Gravel, Dirt],
            // S0 is solid gravel or earth without expected rocks, roots or steps.
            4 if road.mtb_scale() == Some(0) => &[Unknown, Gravel, Dirt],
            4 => &[Unknown, Dirt, Rough],
            _ => &[Unknown],
        },
    };
    plausible.iter().map(|surface| weights[*surface as usize]).fold(0.0, f64::max)
}

/// `None` for an impassable way. Smoothness grades wheels, so it never concerns a walker.
fn smoothness(value: &str, family: Family) -> Option<f64> {
    if matches!(family, Family::Hiking) {
        return Some(1.0);
    }
    let grade = match value {
        "intermediate" => 1,
        "bad" => 2,
        "very_bad" => 3,
        "horrible" => 4,
        "very_horrible" => 5,
        "impassable" => return None,
        _ => 0,
    };
    Some(match family {
        Family::Touring => [1.0, 1.15, 1.6, 2.5, 4.0, 6.0][grade],
        Family::Gravel => [1.0, 1.05, 1.25, 1.8, 3.0, 5.0][grade],
        _ => [1.0, 1.0, 1.1, 1.35, 1.75, 2.5][grade],
    })
}

fn tracktype(value: &str, family: Family) -> f64 {
    let grade = match value {
        "grade2" => 1,
        "grade3" => 2,
        "grade4" => 3,
        "grade5" => 4,
        _ => 0,
    };
    match family {
        Family::Touring => [1.0, 1.05, 1.2, 1.5, 2.0][grade],
        Family::Gravel => [1.0, 1.0, 1.1, 1.3, 1.6][grade],
        Family::Mtb => [1.0, 1.0, 1.05, 1.1, 1.2][grade],
        Family::Hiking => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use planner_router::model::{BIKE, FOOT};

    fn road() -> Road {
        Road {
            from: 0,
            to: 1,
            way: 1,
            reversed: false,
            length_m: 100,
            ascent_m: 0,
            descent_m: 0,
            surface: Surface::Compacted,
            class: 3,
            access: BIKE | FOOT | PUSH,
            difficulty: 255,
            hiking_difficulty: None,
            structure: false,
            shape: vec![],
        }
    }

    fn tags(pairs: &[(&str, &str)]) -> Tags {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn floor(road: &Road, tags: &Tags, name: &str) -> Option<f64> {
        let profile = Profile::presets().into_iter().find(|p| p.name == name).unwrap();
        way(road, tags, Country::default(), &profile).map(|s| s.minimum_factor)
    }

    #[test]
    fn rolling_evidence_is_progressive_and_good_tags_do_not_erase_bad_evidence() {
        for name in ["touring", "gravel", "mtb"] {
            let base = floor(&road(), &Tags::new(), name).unwrap();
            let mut previous = base;
            for smoothness in ["intermediate", "bad", "very_bad", "horrible", "very_horrible"] {
                let next = floor(&road(), &tags(&[("smoothness", smoothness)]), name).unwrap();
                assert!(next >= previous);
                previous = next;
            }
            assert!(floor(&road(), &tags(&[("smoothness", "impassable")]), name).is_none());
            let poor = tags(&[("tracktype", "grade5"), ("smoothness", "excellent")]);
            assert!(floor(&road(), &poor, name).unwrap() > base);
            // Variants keep the physical penalties.
            for variant in ["shorter", "less-climbing"] {
                assert_eq!(floor(&road(), &poor, &format!("{name}/{variant}")), floor(&road(), &poor, name));
            }
        }
    }

    #[test]
    fn unknown_material_uses_track_grade_without_inventing_pavement() {
        for name in ["touring", "gravel", "mtb"] {
            let unknown = Road { surface: Surface::Unknown, ..road() };
            let firm = floor(&unknown, &tags(&[("tracktype", "grade1")]), name).unwrap();
            assert!(floor(&unknown, &tags(&[("tracktype", "grade5")]), name).unwrap() > firm);
            assert!(floor(&unknown, &Tags::new(), name).unwrap() >= firm);
            assert!(firm >= floor(&Road { surface: Surface::Paved, ..road() }, &Tags::new(), name).unwrap());
        }
    }

    #[test]
    fn walking_uses_grade_visibility_and_equipment_instead_of_smoothness() {
        let path = Road { surface: Surface::Dirt, class: 4, ..road() };
        let base = floor(&path, &Tags::new(), "hiking").unwrap();
        assert_eq!(floor(&path, &tags(&[("smoothness", "impassable")]), "hiking"), Some(base));
        assert!(floor(&Road { hiking_difficulty: Some(2), ..path.clone() }, &Tags::new(), "hiking").unwrap() > base);
        assert!(floor(&path, &tags(&[("trail_visibility", "no")]), "hiking").unwrap() > base);
        assert!(floor(&path, &tags(&[("via_ferrata_scale", "2")]), "hiking/alpine").is_none());
        assert!(floor(&path, &tags(&[("via_ferrata_scale", "0")]), "hiking").is_some());
    }

    #[test]
    fn each_permitted_grade_costs_more_than_the_one_below() {
        for (name, top) in [("touring", 1), ("gravel", 1), ("mtb/technical", 6), ("hiking/alpine", 4)] {
            let walking = name.starts_with("hiking");
            let mut previous = 0.0;
            for grade in u8::from(walking)..=top {
                let graded = if walking {
                    Road { hiking_difficulty: Some(grade), ..road() }
                } else {
                    Road { difficulty: grade, ..road() }
                };
                let factor = floor(&graded, &Tags::new(), name).unwrap();
                assert!(factor > previous, "{name} {grade}");
                previous = factor;
            }
        }
    }

    #[test]
    fn a_cycle_track_takes_part_of_the_main_road_dislike_away() {
        let main = Road { surface: Surface::Paved, class: 2, ..road() };
        let mut primary = tags(&[("highway", "primary"), ("maxspeed", "80")]);
        let factor = |tags: &Tags| {
            let profile = Profile::presets().into_iter().find(|p| p.name == "touring").unwrap();
            way(&main, tags, Country::default(), &profile).unwrap()
        };
        let exposed = factor(&primary);
        primary.insert("cycleway:right".into(), "track".into());
        let protected = factor(&primary);
        assert!(protected.basis.factor < exposed.basis.factor);
        assert!(protected.minimum_factor < exposed.minimum_factor && protected.minimum_factor > 1.0);
    }
}
