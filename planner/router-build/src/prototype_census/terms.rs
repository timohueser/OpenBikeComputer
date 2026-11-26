//! PROTOTYPE. The cost of one road under a weighted bike profile as a sum of terms, in the order
//! in which `suitability::way` and `cost::basis` multiply their factors. A later factor scales the
//! terms before it, so each term is the cost that its factor adds on top of the factors before it.
use super::variant;
use crate::country::Country;
use crate::source::Tags;
use crate::suitability::{self, Family};
use crate::{cost, road_bike::tag, traffic, Graph};
use planner_router::model::{Profile, Road, Surface, Weighting, BIKE, PUSH};

pub const N: usize = 18;
pub const NAMES: [&str; N] = [
    "distance",
    "surface_tagged",
    "surface_unknown",
    "smoothness_tracktype",
    "difficulty",
    "pushing",
    "traffic_stress",
    "class_quiet_road",
    "class_main_road",
    "class_track",
    "class_path",
    "class_other",
    "route_bonus",
    "uncertain_access",
    "climb",
    "uncertain_node",
    "rounding",
    "turn_cost",
];
pub const NODE: usize = 15;
/// The turn cost of a variant; 0 today.
pub const TURN: usize = 17;

/// The terms that the question names, each with the fine terms it sums.
pub const GROUPS: [(&str, &[usize]); 12] = [
    ("pure distance", &[0]),
    ("surface factor", &[1, 2]),
    ("smoothness/tracktype", &[3]),
    ("traffic stress", &[6]),
    ("class preference", &[7, 8, 9, 10, 11]),
    ("route bonus", &[12]),
    ("climb", &[14]),
    ("uncertain access", &[13, 15]),
    ("difficulty (mtb:scale, sac_scale)", &[4]),
    ("pushing", &[5]),
    ("rounding", &[16]),
    ("turn cost", &[17]),
];

pub type Terms = [f64; N];

pub fn add(into: &mut Terms, terms: &Terms) {
    into.iter_mut().zip(terms).for_each(|(a, b)| *a += b);
}

pub fn grouped(terms: &Terms) -> [f64; 12] {
    GROUPS.map(|(_, fine)| fine.iter().map(|&i| terms[i]).sum())
}

/// The factors of `suitability::way` one by one. Their product times `class` is its factor.
pub struct Factors {
    pub surface: f64,
    pub rolling: f64,
    pub difficulty: f64,
    pub push: f64,
    pub stress: f64,
    pub class: f64,
    /// `Suitability::minimum_factor` and the factor of its basis.
    pub floor: f64,
    pub full: f64,
    /// The road class that the class preference and the surface rule use.
    pub road_class: u8,
}

/// `None` where the profile excludes the road. The family is that of `suitability::way`: the
/// tables of `smoothness` and `tracktype` depend on it.
pub fn factors(road: &Road, tags: &Tags, country: Country, profile: &Profile) -> Option<Factors> {
    let Weighting::Weighted { surface: weights, .. } = &profile.weighting else { return None };
    let family = match profile.name.split('/').next().unwrap_or("") {
        _ if profile.walking => Family::Hiking,
        "mtb" => Family::Mtb,
        "gravel" => Family::Gravel,
        _ => Family::Touring,
    };
    let suit = suitability::way(road, tags, country, profile)?;
    let reclassed = variant::reclass(road, tags, profile);
    let road = reclassed.as_ref().unwrap_or(road);
    let (floor, full) = (suit.minimum_factor, suit.basis.factor);
    let pushing = profile.mode(road) == PUSH;
    let (surface, rolling, push, stress) = if road.class == 6 {
        (1.0, 1.0, 1.0, 1.0)
    } else {
        (
            suitability::surface_factor(road, tags, weights),
            suitability::smoothness(tag(tags, "smoothness"), family)?
                .max(suitability::tracktype(tag(tags, "tracktype"), family)),
            if pushing { 4.0 } else { 1.0 },
            if pushing || profile.walking { 1.0 } else { 1.0 + 0.6 * traffic::stress(tags, road.reversed, country) },
        )
    };
    let difficulty = floor / (surface * rolling * push * stress);
    Some(Factors {
        surface,
        rolling,
        difficulty,
        push,
        stress,
        class: full / floor,
        floor,
        full,
        road_class: road.class,
    })
}

#[derive(Clone, Copy)]
pub struct RoadTerms {
    pub terms: Terms,
    /// The cost of the road in the bake.
    pub total: u64,
    /// The cost with the whole class preference taken off a road in a signed route: the most that
    /// a route bonus can do.
    pub full_bonus: f64,
    /// The cost with the surface factor of a paved road where the surface is unknown.
    pub unknown_paved: f64,
}

/// The terms of road `id`, checked against the factor and the total of the real cost code.
pub fn road(graph: &Graph, profile: &Profile, id: usize) -> Result<Option<RoadTerms>, String> {
    let way = graph.osm.ways.row(graph.roads.way_row(id) as usize);
    let mut road = Road::default();
    graph.roads.get(id, &mut road);
    let Some(real) = cost::way_basis(&way, road.reversed, profile) else { return Ok(None) };
    let facts = way.facts;
    let plain = graph.roads.attributes(id);
    let f = factors(&plain, &way.tags, facts.country(), profile).ok_or("a road with a basis has no factors")?;
    let length = f64::from(road.length_m);
    let mut terms = [0.0; N];
    terms[0] = length;
    terms[if road.surface == Surface::Unknown { 2 } else { 1 }] = length * (f.surface - 1.0);
    let mut product = f.surface;
    for (term, factor) in [(3, f.rolling), (4, f.difficulty), (5, f.push), (6, f.stress)] {
        terms[term] = length * product * (factor - 1.0);
        product *= factor;
    }
    let class = match f.road_class {
        1 => 7,
        2 => 8,
        3 => 9,
        4 => 10,
        _ => 11,
    };
    terms[class] = length * (f.full - f.floor);
    // The route bonus is what the real factor takes off the full factor, before the doubt.
    let (mode, side) = (profile.mode(&road), usize::from(road.reversed));
    let doubt = if facts.doubts[side] & mode != 0 { 3.0 } else { 1.0 };
    let discounted = real.factor / doubt;
    if discounted > f.full * (1.0 + 1e-9) || discounted < f.floor * (1.0 - 1e-9) {
        return Err(format!("road {id}: the factor {discounted} is not from {} to {}", f.floor, f.full));
    }
    terms[12] = length * (discounted - f.full);
    terms[13] = length * discounted * (doubt - 1.0);
    let compiled = real.compile(&road, profile)?;
    terms[14] = compiled.penalties.last().map_or(0.0, |knot| knot.1);
    let total = compiled.total();
    terms[16] = total as f64 - terms.iter().sum::<f64>();
    let signed = mode == BIKE && road.class != 6 && facts.ranks[side] != 0;
    let floor = if signed { f.floor } else { discounted };
    let paved = if road.surface == Surface::Unknown { real.factor / f.surface } else { real.factor };
    Ok(Some(RoadTerms {
        terms,
        total,
        full_bonus: length * floor * doubt + terms[14],
        unknown_paved: length * paved + terms[14],
    }))
}
