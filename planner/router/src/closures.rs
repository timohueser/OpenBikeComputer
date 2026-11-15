//! Possible closures: why a rider may have no access to a road that the router keeps open.
use serde::{Deserialize, Serialize};

/// Why the rider may have no access to a road that the router keeps open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// `access=permit`.
    Permit,
    /// `access=private`.
    Private,
    /// `agricultural` or `forestry`.
    Farm,
    /// `use_sidepath`: the mode should use a parallel path.
    Sidepath,
    /// `discouraged`.
    Discouraged,
    /// Access for another group, such as `destination`, `customers` or `psv`.
    Limited,
    /// A conditional restriction that names only months, days or seasons.
    Seasonal,
    /// Any other conditional restriction, such as `wet` or `Mo-Fr 07:00-19:00`.
    Conditional,
    /// An access value or a barrier that the router does not know, or a reversible one-way.
    Unclear,
}

impl Kind {
    /// An access value, not a condition: the router avoids such roads where it can. A condition
    /// depends on the date or the weather of the ride, so it costs nothing.
    pub fn avoided(self) -> bool {
        !matches!(self, Kind::Seasonal | Kind::Conditional)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Closure {
    pub kind: Kind,
    /// The OSM condition, or the OSM access value or tag for the other kinds.
    pub condition: String,
}

/// The closures of one road: `(modes, closure)` entries, such as `(BIKE, Seasonal "Nov-May")`.
pub type Set = Vec<(u8, Closure)>;
