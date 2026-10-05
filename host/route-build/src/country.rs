//! The rules of the country a way is in: its default access and its side of the road. Borders
//! come from the OSM-derived boundaries of the JOSM project, embedded in `country-boundaries`.
use country_boundaries::{CountryBoundaries, LatLon, BOUNDARIES_ODBL_360X180};
use route_engine::model::{Point, BIKE, FOOT, PUSH};
use std::sync::LazyLock;

const ALL: u8 = BIKE | FOOT | PUSH;
const WALK: u8 = FOOT | PUSH;

/// The default modes that differ from the worldwide table (`source::highway_access`), by ISO
/// 3166-1 code. A class also covers its `_link`. They follow the country tables of
/// <https://wiki.openstreetmap.org/wiki/OSM_tags_for_routing/Access_restrictions>; a cell that
/// the wiki gives only as a condition or a footnote keeps the worldwide value.
const DEFAULTS: &[(&str, &[(&str, u8)])] = &[
    ("AT", &[("trunk", 0), ("path", WALK)]),
    ("BE", &[("trunk", 0), ("cycleway", ALL), ("bridleway", WALK)]),
    ("BR", &[("bridleway", ALL)]),
    ("BY", &[("footway", ALL), ("pedestrian", ALL)]),
    ("CH", &[("trunk", 0)]),
    ("CN", &[("cycleway", ALL), ("pedestrian", ALL)]),
    ("DK", &[("trunk", 0), ("cycleway", ALL)]),
    ("FI", &[("cycleway", ALL), ("pedestrian", ALL)]),
    ("FR", &[("trunk", 0), ("pedestrian", ALL)]),
    ("GB", &[("cycleway", ALL), ("bridleway", ALL)]),
    ("GR", &[("cycleway", ALL), ("bridleway", ALL)]),
    ("HR", &[("trunk", 0)]),
    ("HU", &[("trunk", 0), ("cycleway", ALL)]),
    ("IE", &[("bridleway", ALL)]),
    ("IS", &[("cycleway", ALL), ("footway", ALL), ("pedestrian", ALL)]),
    ("IT", &[("pedestrian", ALL)]),
    ("NL", &[("cycleway", ALL)]),
    ("NO", &[("cycleway", ALL), ("footway", ALL), ("pedestrian", ALL)]),
    ("PH", &[("cycleway", ALL), ("pedestrian", ALL), ("bridleway", ALL)]),
    ("PL", &[("bridleway", ALL)]),
    ("RO", &[("bridleway", ALL)]),
    ("SE", &[("cycleway", ALL), ("pedestrian", ALL), ("bridleway", WALK)]),
    ("SK", &[("trunk", 0)]),
    ("TH", &[("cycleway", ALL), ("pedestrian", ALL), ("bridleway", ALL)]),
    ("TR", &[("cycleway", ALL), ("bridleway", WALK)]),
    ("US", &[("cycleway", ALL), ("pedestrian", ALL), ("bridleway", ALL)]),
];

/// The `driving_side` tags of the same JOSM boundaries. A territory without one, such as
/// Jersey, takes the side of its parent; the most specific tagged id wins.
const LEFT_HAND: &[&str] = &[
    "AG", "AU", "BB", "BD", "BN", "BS", "BT", "BW", "CY", "DM", "FJ", "GB", "GD", "GY", "HK", "ID", "IE", "IN", "JM",
    "JP", "KE", "KI", "KN", "LC", "LK", "LS", "MO", "MT", "MU", "MV", "MW", "MY", "MZ", "NA", "NP", "NR", "NZ", "PG",
    "PK", "SB", "SC", "SG", "SR", "SZ", "TH", "TL", "TO", "TT", "TV", "TZ", "UG", "VC", "VI", "WS", "ZA", "ZM", "ZW",
];
const RIGHT_HAND: &[&str] = &["GI", "IO"];

static BOUNDARIES: LazyLock<CountryBoundaries> =
    LazyLock::new(|| CountryBoundaries::from_reader(BOUNDARIES_ODBL_360X180).expect("embedded country boundaries"));

/// The country at a point. Outside every country, the worldwide rules apply.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Country {
    code: Option<&'static str>,
    left_hand: bool,
}

impl Country {
    pub fn at(point: Point) -> Self {
        let Ok(position) = LatLon::new(point.lat as f64 * 1e-6, point.lon as f64 * 1e-6) else {
            return Self::default();
        };
        // The ids run from the most specific, such as `["JE", "GB"]` or `["GB-ENG", "GB"]`.
        let ids = BOUNDARIES.ids(position);
        let side = |id: &&str| {
            if LEFT_HAND.contains(id) {
                Some(true)
            } else {
                RIGHT_HAND.contains(id).then_some(false)
            }
        };
        Country {
            code: ids.iter().copied().find(|id| !id.contains('-')),
            left_hand: ids.iter().find_map(side).unwrap_or(false),
        }
    }

    /// The country of a way: that of its first node with a known point.
    pub fn of_way(nodes: &[i64], point: impl Fn(i64) -> Option<Point>) -> Self {
        nodes.iter().find_map(|&id| point(id)).map_or_else(Self::default, Self::at)
    }

    /// The default modes of `highway` where this country differs from the worldwide table.
    pub fn defaults(self, highway: &str) -> Option<u8> {
        let (_, classes) = DEFAULTS.iter().find(|(code, _)| Some(*code) == self.code)?;
        let class = highway.strip_suffix("_link").unwrap_or(highway);
        classes.iter().find(|(name, _)| *name == class).map(|(_, modes)| *modes)
    }

    pub fn left_hand(self) -> bool {
        self.left_hand
    }

    #[cfg(test)]
    pub fn of(code: &'static str) -> Self {
        Country { code: Some(code), left_hand: LEFT_HAND.contains(&code) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_takes_its_country_and_its_rules() {
        let at = |lat: f64, lon: f64| {
            Country::at(Point { lat: (lat * 1e6) as i32, lon: (lon * 1e6) as i32, elevation: 0.0 })
        };
        let of = Country::of;
        // Büsingen am Hochrhein is a German exclave inside Switzerland.
        assert_eq!(at(47.6973, 8.6910), of("DE"));
        assert_eq!(at(47.37, 8.54), of("CH"));
        assert_eq!(at(39.74, -104.99), of("US"));
        assert_eq!(at(35.0, -40.0), Country::default());
        assert_eq!(of("CH").defaults("trunk_link"), Some(0));
        assert_eq!(of("DE").defaults("trunk"), None);
        assert!(of("GB").left_hand() && !of("DE").left_hand() && !Country::default().left_hand());
        // Jersey drives on the left like the United Kingdom; Gibraltar drives on the right.
        assert!(at(49.21, -2.13).left_hand() && !at(36.14, -5.35).left_hand());
    }
}
