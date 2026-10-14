//! Physical suitability from OSM surface, smoothness, tracktype and difficulty tags.
use crate::{country::Country, road_bike::tag, source::Tags, traffic};
use planner_router::{
    cost::CostBasis,
    model::{Profile, Road, Surface, Weighting, NO_ELEVATION, PUSH},
};

pub struct Suitability {
    pub basis: CostBasis,
    /// Network preferences cannot discount physical difficulty, pushing or traffic exposure.
    pub minimum_factor: f64,
}

#[derive(Clone, Copy)]
enum Family {
    Touring,
    Gravel,
    Mtb,
    Hiking,
}

/// The coefficients express conservative preferences, not measured speed or accident risk.
pub fn way(road: &Road, tags: &Tags, country: Country, profile: &Profile) -> Option<Suitability> {
    let Weighting::Weighted { surface, road: weights, .. } = &profile.weighting else {
        return None;
    };
    if !profile.permits(road) || requires_equipment(tags) {
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
    let mut minimum_factor = if ferry {
        1.0
    } else {
        let rolling = smoothness(tag(tags, "smoothness"), family)?.max(tracktype(tag(tags, "tracktype"), family));
        let difficulty = if profile.walking {
            [1.0, 1.0, 1.3, 1.8, 2.5, 3.5, 5.0][usize::from(road.hiking_difficulty.unwrap_or(0).min(6))]
        } else {
            let grade = usize::from(road.mtb_scale().unwrap_or(0).min(6));
            let mtb: f64 = match family {
                Family::Touring => [1.0, 1.5, 2.0, 3.0, 4.0, 5.0, 6.0][grade],
                Family::Gravel => [1.0, 1.35, 1.8, 2.5, 3.5, 4.5, 6.0][grade],
                _ => [1.0, 1.15, 1.5, 2.0, 3.0, 4.0, 5.0][grade],
            };
            // SAC describes foot travel; mountain hiking still warns of difficult bike passage.
            mtb.max(if road.hiking_difficulty.is_some_and(|d| d >= 2) { 1.5 } else { 1.0 })
                .max(uphill(road, tags, family))
        };
        let visibility = if profile.walking {
            match tag(tags, "trail_visibility") {
                "intermediate" => 1.1,
                "bad" => 1.4,
                "horrible" => 1.8,
                "no" => 2.5,
                _ => 1.0,
            }
        } else {
            1.0
        };
        surface_factor(road, tags, surface) * rolling * difficulty * visibility * if pushing { 4.0 } else { 1.0 }
    };
    let mut preference = *weights.get(road.class as usize)?;
    if !profile.walking && !pushing && !ferry {
        if road.class == 2 {
            let exposure = match traffic::protection(tags, road.reversed, country.left_hand()) {
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

/// A via ferrata requires equipment beyond the walking or cycling profiles.
pub fn requires_equipment(tags: &Tags) -> bool {
    tag(tags, "highway") == "via_ferrata"
        || tag(tags, "sport") == "via_ferrata"
        || matches!(tag(tags, "via_ferrata"), "yes" | "designated")
        || !matches!(tag(tags, "via_ferrata_scale"), "" | "0" | "no")
}

fn surface_factor(road: &Road, tags: &Tags, weights: &[f64; 6]) -> f64 {
    if road.surface != Surface::Unknown {
        return weights[road.surface as usize];
    }
    use Surface::*;
    // Track grades describe firmness, not material. Retain the less favourable plausible material.
    let plausible: &[Surface] = match tag(tags, "tracktype") {
        "grade1" => &[Paved, Compacted],
        "grade2" => &[Compacted, Gravel],
        "grade3" => &[Gravel, Dirt],
        "grade4" => &[Dirt],
        "grade5" => &[Dirt, Rough],
        _ => match road.class {
            0..=2 => &[Unknown, Paved],
            3 => &[Unknown, Gravel, Dirt],
            // S0 describes solid gravel or earth without expected rocks, roots or steps.
            4 if road.mtb_scale() == Some(0) => &[Unknown, Gravel, Dirt],
            4 => &[Unknown, Dirt, Rough],
            _ => &[Unknown],
        },
    };
    plausible.iter().map(|surface| weights[*surface as usize]).fold(0.0, f64::max)
}

fn smoothness(value: &str, family: Family) -> Option<f64> {
    // Smoothness describes wheeled vehicles; even "impassable" does not exclude pedestrians.
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

fn uphill(road: &Road, tags: &Tags, family: Family) -> f64 {
    let Some(grade) = tag(tags, "mtb:scale:uphill").parse::<usize>().ok().filter(|&grade| grade <= 5) else {
        return 1.0;
    };
    let incline = match tag(tags, "incline").trim() {
        "up" => Some(1.0),
        "down" => Some(-1.0),
        value => value.trim_end_matches(['%', '°']).trim().parse::<f64>().ok().filter(|v| v.is_finite()),
    };
    let climbs = incline.map_or_else(
        || {
            road.shape.first().zip(road.shape.last()).is_some_and(|(a, b)| {
                a.elevation != NO_ELEVATION && b.elevation != NO_ELEVATION && b.elevation - a.elevation > 1.0
            })
        },
        |slope| if road.reversed { slope < 0.0 } else { slope > 0.0 },
    );
    if !climbs {
        return 1.0;
    }
    // Uphill grades describe climbing effort and skill, not the descent-scale eligibility cap.
    match family {
        Family::Touring => [1.0, 1.15, 1.4, 1.8, 2.5, 4.0][grade],
        Family::Gravel => [1.0, 1.1, 1.25, 1.6, 2.2, 3.0][grade],
        _ => [1.0, 1.05, 1.15, 1.35, 1.6, 2.0][grade],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use planner_router::model::{Point, BIKE, FOOT};

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

    fn profile(name: &str) -> Profile {
        Profile::presets().into_iter().find(|p| p.name == name).unwrap()
    }

    fn tags(pairs: &[(&str, &str)]) -> Tags {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn cost(road: &Road, tags: &Tags, name: &str) -> Option<Suitability> {
        way(road, tags, Country::default(), &profile(name))
    }

    #[test]
    fn rolling_evidence_is_progressive_and_good_tags_do_not_erase_bad_evidence() {
        for name in ["touring", "gravel", "mtb"] {
            let base = cost(&road(), &Tags::new(), name).unwrap().minimum_factor;
            let mut previous = base;
            for smoothness in ["intermediate", "bad", "very_bad", "horrible", "very_horrible"] {
                let next = cost(&road(), &tags(&[("smoothness", smoothness)]), name).unwrap().minimum_factor;
                assert!(next >= previous);
                previous = next;
            }
            assert!(cost(&road(), &tags(&[("smoothness", "impassable")]), name).is_none());
            let poor = tags(&[("tracktype", "grade5"), ("smoothness", "excellent")]);
            assert!(cost(&road(), &poor, name).unwrap().minimum_factor > base);
            let rough = Road { surface: Surface::Rough, ..road() };
            assert!(cost(&rough, &tags(&[("smoothness", "excellent")]), name).unwrap().minimum_factor > base);
        }
    }

    #[test]
    fn unknown_material_uses_road_and_track_context_without_inventing_pavement() {
        for name in ["touring", "gravel", "mtb"] {
            let unknown = Road { surface: Surface::Unknown, ..road() };
            let firm = cost(&unknown, &tags(&[("tracktype", "grade1")]), name).unwrap().minimum_factor;
            let soft = cost(&unknown, &tags(&[("tracktype", "grade5")]), name).unwrap().minimum_factor;
            assert!(soft > firm);
            assert!(cost(&unknown, &Tags::new(), name).unwrap().minimum_factor >= firm);
            let paved = Road { surface: Surface::Paved, ..road() };
            assert!(firm >= cost(&paved, &Tags::new(), name).unwrap().minimum_factor);
        }
    }

    #[test]
    fn easy_trail_evidence_reduces_unknown_surface_detours_without_erasing_bad_tags() {
        let unknown = Road { class: 4, surface: Surface::Unknown, ..road() };
        let easy = Road { difficulty: 0, ..unknown.clone() };
        for name in ["touring", "gravel", "mtb"] {
            let ungraded = cost(&unknown, &Tags::new(), name).unwrap();
            let known_easy = cost(&easy, &Tags::new(), name).unwrap();
            assert!(known_easy.basis.factor < ungraded.basis.factor);
            for surface in [Surface::Gravel, Surface::Dirt] {
                let mapped = Road { surface, ..easy.clone() };
                assert!(known_easy.minimum_factor >= cost(&mapped, &Tags::new(), name).unwrap().minimum_factor);
            }
            let rough = Road { surface: Surface::Rough, ..easy.clone() };
            assert!(cost(&rough, &Tags::new(), name).unwrap().minimum_factor > known_easy.minimum_factor);
            for evidence in [[("smoothness", "very_bad")], [("tracktype", "grade5")]] {
                assert!(cost(&easy, &tags(&evidence), name).unwrap().minimum_factor > known_easy.minimum_factor);
            }
            assert!(cost(&easy, &tags(&[("smoothness", "impassable")]), name).is_none());
            assert!(cost(&Road { access: 0, ..easy.clone() }, &Tags::new(), name).is_none());
        }
    }

    #[test]
    fn walking_uses_foot_difficulty_visibility_and_equipment_instead_of_wheel_smoothness() {
        let path = Road { surface: Surface::Dirt, class: 4, ..road() };
        let base = cost(&path, &Tags::new(), "hiking").unwrap().minimum_factor;
        let impassable = tags(&[("smoothness", "impassable")]);
        assert_eq!(cost(&path, &impassable, "hiking").unwrap().minimum_factor, base);
        let mountain = Road { hiking_difficulty: Some(2), ..path.clone() };
        assert!(cost(&mountain, &Tags::new(), "hiking").unwrap().minimum_factor > base);
        assert!(cost(&path, &tags(&[("trail_visibility", "no")]), "hiking").unwrap().minimum_factor > base);
        assert!(cost(&path, &tags(&[("via_ferrata_scale", "2")]), "hiking").is_none());
        assert!(cost(&path, &tags(&[("via_ferrata_scale", "0")]), "hiking").is_some());
    }

    #[test]
    fn permitted_difficulty_increases_cost_and_variants_keep_physical_penalties() {
        for name in ["touring", "gravel", "mtb", "hiking"] {
            let mut selected = profile(name);
            selected.max_difficulty = if selected.walking { 4 } else { 3 };
            let mut previous = 0.0;
            for grade in 0..=selected.max_difficulty {
                let graded = if selected.walking {
                    Road { hiking_difficulty: Some(grade), ..road() }
                } else {
                    Road { difficulty: grade, ..road() }
                };
                let factor = way(&graded, &Tags::new(), Country::default(), &selected).unwrap().minimum_factor;
                assert!(factor >= previous);
                if grade >= 2 || grade == 1 && !selected.walking {
                    assert!(factor > previous);
                }
                previous = factor;
            }
            let source = tags(&[("smoothness", "very_bad"), ("tracktype", "grade3")]);
            let floor = cost(&road(), &source, name).unwrap().minimum_factor;
            for variant in ["shorter", "less-climbing"] {
                assert_eq!(cost(&road(), &source, &format!("{name}/{variant}")).unwrap().minimum_factor, floor);
            }
        }
    }

    #[test]
    fn uphill_scale_penalizes_only_an_established_climbing_direction() {
        let road = Road {
            shape: vec![
                Point { elevation: 100.0, ..Point::default() },
                Point { lon: 1000, elevation: 120.0, ..Point::default() },
            ],
            ..road()
        };
        let reverse = Road { reversed: true, shape: road.shape.iter().rev().copied().collect(), ..road.clone() };
        for name in ["touring", "gravel", "mtb"] {
            let base = cost(&road, &Tags::new(), name).unwrap().minimum_factor;
            let mut previous = base;
            for grade in 1..=5 {
                let source = tags(&[("mtb:scale:uphill", &grade.to_string())]);
                let factor = cost(&road, &source, name).unwrap().minimum_factor;
                assert!(factor > previous);
                assert_eq!(cost(&reverse, &source, name).unwrap().minimum_factor, base);
                previous = factor;
            }
            for incline in ["up", "8%", "+8°"] {
                let source = tags(&[("mtb:scale:uphill", "4"), ("incline", incline)]);
                assert!(cost(&road, &source, name).unwrap().minimum_factor > base);
                assert_eq!(cost(&reverse, &source, name).unwrap().minimum_factor, base);
            }
            let down = tags(&[("mtb:scale:uphill", "4"), ("incline", "down")]);
            assert_eq!(cost(&road, &down, name).unwrap().minimum_factor, base);
            assert!(cost(&reverse, &down, name).unwrap().minimum_factor > base);
            let mut unknown = road.clone();
            unknown.shape[0].elevation = NO_ELEVATION;
            assert_eq!(cost(&unknown, &tags(&[("mtb:scale:uphill", "5")]), name).unwrap().minimum_factor, base);
        }
    }
}
