//! The summit crosses of the TrailView feature tiles: a cross within SUMMIT_M of a peak is a summit
//! cross, and a named peak with no cross within SUMMIT_M gets one at its top. The peaks are no features.

use rstar::RTree;

use crate::{metres, Tags};

/// A cross mapped beside its peak's node still stands on the summit.
pub const SUMMIT_M: f64 = 50.0;

/// A point feature: its tags and its place.
type Point = (Tags, [f64; 2]);

/// Whether this module places the features of a kind.
pub fn solves(kind: &str) -> bool {
    matches!(kind, "cross" | "peak")
}

/// The crosses with their tag `summit:cross` set near a peak, and a summit cross per named peak that
/// has none near, in the order of `peaks`. A peak tagged `summit:cross` `no` gets none.
pub fn crosses(mut crosses: Vec<Point>, peaks: &[Point]) -> (Vec<Point>, Vec<Point>) {
    let tops = RTree::bulk_load(peaks.iter().map(|(_, p)| *p).collect::<Vec<_>>());
    let near = |tree: &RTree<[f64; 2]>, p: [f64; 2]| {
        let reach = SUMMIT_M / metres(p[1]);
        tree.locate_within_distance(p, reach * reach).next().is_some()
    };
    for (tags, point) in &mut crosses {
        if near(&tops, *point) {
            tags.insert("summit:cross".into(), "yes".into());
        }
    }
    let standing = RTree::bulk_load(crosses.iter().map(|(_, p)| *p).collect::<Vec<_>>());
    let added = peaks
        .iter()
        .filter(|(tags, point)| {
            tags.contains_key("name") && tags.get("summit:cross").is_none_or(|v| v != "no") && !near(&standing, *point)
        })
        .map(|(_, point)| (Tags::from([("summit:cross".into(), "yes".into())]), *point))
        .collect();
    (crosses, added)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(east: f64) -> [f64; 2] {
        let y = 0.36;
        [0.52 + east / metres(y), y]
    }

    fn tags(pairs: &[(&str, &str)]) -> Tags {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn a_cross_near_a_peak_is_a_summit_cross_and_a_named_peak_without_one_gets_one() {
        let named = tags(&[("name", "Titlis")]);
        let peaks = [
            (named.clone(), at(0.0)),
            (named.clone(), at(1000.0)),
            (tags(&[]), at(2000.0)),
            (tags(&[("name", "Stotzig"), ("summit:cross", "no")]), at(3000.0)),
        ];
        let wayside = tags(&[("material", "wood")]);
        let crosses = vec![(wayside.clone(), at(30.0)), (wayside.clone(), at(500.0))];
        let summit = tags(&[("summit:cross", "yes")]);
        assert_eq!(
            super::crosses(crosses, &peaks),
            (
                vec![(tags(&[("material", "wood"), ("summit:cross", "yes")]), at(30.0)), (wayside, at(500.0))],
                vec![(summit, at(1000.0))]
            )
        );
    }
}
