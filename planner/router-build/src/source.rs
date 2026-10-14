//! The source OSM model and the tag rules that the import, the overlay index and the route
//! catalog share. No source object enters the routing package.
use crate::country::Country;
use planner_router::closures::{Closure, Kind};
use planner_router::model::{Point, BIKE, FOOT, PUSH};
use std::collections::BTreeMap;

pub type Tags = BTreeMap<String, String>;

pub use obc_places::routing::access::{
    access, conditional_modes, cycleway, cycleways, granted, inherited, oneway, Oneway,
};
use obc_places::routing::access::{conditions, unshadowed_modes};

/// A source access restriction represented by the planner's displayed closure type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    Open,
    Uncertain(Kind),
    Closed,
}

pub fn classify(value: &str) -> Access {
    use obc_places::routing::access::{Access as Shared, Restriction as R};
    match obc_places::routing::access::classify(value) {
        Shared::Open => Access::Open,
        Shared::Closed => Access::Closed,
        Shared::Uncertain(reason) => Access::Uncertain(match reason {
            R::Permit => Kind::Permit,
            R::Private => Kind::Private,
            R::Farm => Kind::Farm,
            R::Sidepath => Kind::Sidepath,
            R::Discouraged => Kind::Discouraged,
            R::Limited => Kind::Limited,
            R::Unclear => Kind::Unclear,
        }),
    }
}

/// Country defaults specialize the shared worldwide highway table.
pub fn highway_access(highway: &str, country: Country) -> Option<(u8, u8)> {
    obc_places::routing::access::highway_access(highway, country.defaults(highway))
}

/// What may close a road that the router keeps open: an uncertain access value, a reversible
/// one-way, or a conditional restriction. Only a restricting clause counts: `yes @ (May-Oct)`
/// names the open season. A conditional one-way closes one direction only, so it is no closure of
/// the road. Only the tags of the given `directions` of travel count.
pub fn closures<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>, directions: &[&str]) -> Vec<(u8, Closure)> {
    let tags: BTreeMap<&str, &str> = tags.collect();
    let get = |key: &str| tags.get(key).copied();
    let mut found = Vec::new();
    for (mode, bits) in [("foot", FOOT | PUSH), ("bicycle", BIKE)] {
        for &direction in directions {
            if let Some(value) = inherited(&get, mode, direction) {
                if let Access::Uncertain(kind) = classify(value) {
                    found.push((bits, kind, value));
                }
            }
        }
    }
    for (mode, bits) in [("foot", FOOT | PUSH), ("bicycle", BIKE)] {
        if oneway(get, mode) == Oneway::Reversible {
            found.push((bits, Kind::Unclear, "oneway=reversible"));
        }
    }
    for (key, modes, clauses) in conditions(tags.iter().map(|(key, value)| (*key, *value))) {
        let modes = unshadowed_modes(&tags, key, modes, directions);
        if modes == 0 || key.starts_with("oneway") {
            continue;
        }
        for (_, condition) in clauses.into_iter().filter(|(value, _)| !granted(value)) {
            found.push((modes, if seasonal(condition) { Kind::Seasonal } else { Kind::Conditional }, condition));
        }
    }
    let mut closures: Vec<(u8, Closure)> = Vec::new();
    for (modes, kind, condition) in found {
        match closures.iter_mut().find(|(_, known)| known.kind == kind && known.condition == condition) {
            Some((known, _)) => *known |= modes,
            None => closures.push((modes, Closure { kind, condition: condition.to_owned() })),
        }
    }
    closures
}

/// A closure such as `Nov-May` moves with the snow each year; the snow layer tells the rider
/// when the road is usually clear. A dated closure with a year is not seasonal.
fn seasonal(condition: &str) -> bool {
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    const SEASONS: [&str; 4] = ["winter", "spring", "summer", "autumn"];
    let condition = condition.to_ascii_lowercase();
    let mut named = false;
    let dates = condition.split([' ', '-', ',']).filter(|token| !token.is_empty()).all(|token| {
        let day = token.trim_end_matches(char::is_alphabetic);
        if !day.is_empty() {
            let suffix = &token[day.len()..];
            return matches!(suffix, "" | "st" | "nd" | "rd" | "th")
                && day.parse::<u8>().is_ok_and(|d| (1..=31).contains(&d));
        }
        let name = SEASONS.contains(&token) || MONTHS.iter().any(|m| token.starts_with(m));
        named |= name;
        name
    });
    dates && named
}

#[derive(Clone, Debug)]
pub struct Node {
    pub id: i64,
    pub point: Point,
    pub tags: Tags,
}

#[derive(Clone, Debug)]
pub struct Way {
    pub id: i64,
    pub nodes: Vec<i64>,
    pub tags: Tags,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Id {
    Node(i64),
    Way(i64),
    Relation(i64),
}

#[derive(Clone, Debug)]
pub struct Relation {
    pub id: i64,
    pub tags: Tags,
    pub members: Vec<(Id, String)>,
}

#[derive(Clone, Debug, Default)]
pub struct Data {
    pub nodes: BTreeMap<i64, Node>,
    pub ways: BTreeMap<i64, Way>,
    pub relations: BTreeMap<i64, Relation>,
}

impl Data {
    pub fn country(&self, way: &Way) -> Country {
        Country::of_way(&way.nodes, |id| self.nodes.get(&id).map(|node| node.point))
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
        let reversible = closures([("oneway", "reversible")].into_iter(), &["forward"]);
        assert_eq!(reversible, vec![(BIKE, Closure { kind: Kind::Unclear, condition: "oneway=reversible".into() })]);
    }

    #[test]
    fn a_lane_without_a_direction_runs_with_the_traffic_on_its_side() {
        let get = |key: &str| (key == "cycleway:left").then_some("lane");
        assert_eq!([false, true].map(|reversed| cycleway(get, reversed, false)), [false, true]);
        assert_eq!([false, true].map(|reversed| cycleway(get, reversed, true)), [true, false]);
    }

    #[test]
    fn conditional_warnings_follow_mode_and_direction_precedence() {
        let all = BIKE | FOOT | PUSH;
        let examples = [
            (&[("access:conditional", "no @ (wet)"), ("bicycle", "yes")], [FOOT | PUSH; 2]),
            (&[("access:conditional", "no @ (wet)"), ("vehicle", "yes")], [FOOT | PUSH; 2]),
            (&[("access:conditional", "no @ (wet)"), ("foot", "yes")], [BIKE; 2]),
            (&[("access:conditional", "no @ (wet)"), ("bicycle:forward", "yes")], [FOOT | PUSH, all]),
            (&[("access:conditional", "no @ (wet)"), ("access:backward", "yes")], [all, 0]),
            (&[("access:forward:conditional", "no @ (wet)"), ("bicycle", "yes")], [FOOT | PUSH, 0]),
            (&[("bicycle:conditional", "no @ (wet)"), ("bicycle", "yes")], [BIKE; 2]),
            (&[("foot:conditional", "no @ (wet)"), ("bicycle:pushing", "yes")], [FOOT | PUSH; 2]),
            (&[("bicycle:pushing:conditional", "no @ (wet)"), ("bicycle:pushing:forward", "yes")], [0, PUSH]),
        ];
        for (tags, expected) in examples {
            assert_eq!(conditional_modes(tags.iter().copied()), expected[0] | expected[1], "{tags:?}");
            for (direction, bits) in ["forward", "backward"].into_iter().zip(expected) {
                let expected = if bits == 0 {
                    vec![]
                } else {
                    vec![(bits, Closure { kind: Kind::Conditional, condition: "wet".into() })]
                };
                assert_eq!(closures(tags.iter().copied(), &[direction]), expected, "{tags:?}, {direction}");
            }
        }
        assert_eq!(conditional_modes([("oneway:conditional", "yes @ (Su)"), ("oneway:bicycle", "no")].into_iter()), 0);
    }

    #[test]
    fn only_no_closes_and_every_other_restriction_becomes_a_closure() {
        use Access::{Closed, Open, Uncertain};
        for (value, expected) in [
            ("yes;designated", Open),
            ("mtb", Open),
            ("optional_sidepath", Open),
            ("no", Closed),
            ("dismount", Closed),
            ("permit", Uncertain(Kind::Permit)),
            ("private", Uncertain(Kind::Private)),
            ("agricultural;forestry", Uncertain(Kind::Farm)),
            ("use_sidepath", Uncertain(Kind::Sidepath)),
            ("discouraged", Uncertain(Kind::Discouraged)),
            ("customers", Uncertain(Kind::Limited)),
            ("motor_vehicle;emergency", Uncertain(Kind::Limited)),
            ("military", Uncertain(Kind::Limited)),
            ("no;private", Uncertain(Kind::Private)),
            ("service", Uncertain(Kind::Unclear)),
        ] {
            assert_eq!(classify(value), expected, "{value}");
        }
        let permit = |key: &str| (key == "access").then_some("permit");
        assert_eq!(access(permit, BIKE | FOOT | PUSH, "forward", true), BIKE | FOOT | PUSH);
        assert_eq!(access(permit, BIKE | FOOT | PUSH, "forward", false), 0);

        let closures = |tags: &[(&'static str, &'static str)]| closures(tags.iter().copied(), &["forward", "backward"]);
        let closure = |kind, condition: &str| Closure { kind, condition: condition.into() };
        let all = BIKE | FOOT | PUSH;
        assert_eq!(
            closures(&[
                ("access", "permit"),
                ("vehicle", "permit"),
                ("access:conditional", "no @ Oct 14th - May 31st")
            ]),
            vec![(all, closure(Kind::Permit, "permit")), (FOOT | PUSH, closure(Kind::Seasonal, "Oct 14th - May 31st"))]
        );
        assert_eq!(
            closures(&[("access", "destination"), ("bicycle", "yes"), ("bicycle:conditional", "no @ (wet)")]),
            vec![(FOOT | PUSH, closure(Kind::Limited, "destination")), (BIKE, closure(Kind::Conditional, "wet"))]
        );
        assert_eq!(
            closures(&[("access", "private"), ("bicycle", "use_sidepath")]),
            vec![(FOOT | PUSH, closure(Kind::Private, "private")), (BIKE, closure(Kind::Sidepath, "use_sidepath"))]
        );
        let one_way = [("bicycle:backward", "use_sidepath"), ("bicycle:backward:conditional", "no @ (wet)")];
        assert_eq!(super::closures(one_way.into_iter(), &["forward"]), vec![]);
        assert_eq!(super::closures(one_way.into_iter(), &["backward"]).len(), 2);
        assert_eq!(
            closures(&[
                ("access:conditional", "no @ (Nov-May)"),
                ("foot:conditional", "no @ (2026 Mar 1-2026 Jul 31)")
            ]),
            vec![
                (all, closure(Kind::Seasonal, "Nov-May")),
                (FOOT | PUSH, closure(Kind::Conditional, "2026 Mar 1-2026 Jul 31"))
            ]
        );
        // An open season, a one-way, a motor-vehicle rule and a freight rule close nothing.
        for tag in [
            ("access:conditional", "yes @ (May-Oct)"),
            ("oneway:conditional", "yes @ (Mo-Fr 07:00-09:00)"),
            ("motor_vehicle:conditional", "no @ (wet)"),
            ("access:conditional", "agricultural @ hazmat:water"),
        ] {
            assert_eq!(closures(&[tag]), vec![], "{tag:?}");
        }
    }
}
