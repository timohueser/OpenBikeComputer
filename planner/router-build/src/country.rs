//! Planner coordinates adapted to the shared host country policy.
use planner_router::model::Point;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Country(obc_places::routing::country::Country);

impl Country {
    /// Use the first source node with a known coordinate.
    pub fn of_way(nodes: &[i64], point: impl Fn(i64) -> Option<Point>) -> Self {
        let country = nodes
            .iter()
            .find_map(|&id| point(id))
            .map(|point| obc_places::routing::country::Country::at((point.lon as f64 * 1e-6, point.lat as f64 * 1e-6)));
        Self(country.unwrap_or_default())
    }

    pub fn defaults(self, highway: &str) -> Option<u8> {
        self.0.defaults(highway)
    }
    pub fn left_hand(self) -> bool {
        self.0.left_hand()
    }
}
