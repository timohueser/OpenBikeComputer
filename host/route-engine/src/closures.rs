//! Possible closures: one small table of distinct closure sets, and a paged column that names
//! the set of each road, so a selection loads only the pages of its roads.
use crate::{
    base::{Column, Numbers},
    table::{valid_digest, Table},
};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::{Entry, HashMap};

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

/// The manifest's reference to the closures of a package.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Index {
    /// The `Vec<Set>` object of distinct sets.
    pub sets: String,
    /// One set id per directed road; the all-ones value means no possible closure.
    pub roads: Column,
}

impl Index {
    pub fn valid(&self, roads: u32) -> bool {
        valid_digest(&self.sets) && self.roads.values.len == roads && self.roads.values.valid()
    }
    pub fn tables(&self) -> [&Table; 1] {
        [&self.roads.values]
    }
}

/// The router blocks a mode only where the rider surely has no access. Roads where the rider may
/// have no access stay open, and the route reports them.
pub struct Closures {
    pub sets: Vec<Set>,
    /// The set id of each road; `u64::MAX` for none. Empty when the package has no closures.
    pub roads: Numbers,
}

impl Default for Closures {
    fn default() -> Self {
        Self { sets: Vec::new(), roads: Numbers::U8(Vec::new()) }
    }
}

impl Closures {
    /// The table from the closures of each road, in road order. A set keeps the order of its
    /// first mentions: access values before conditions, as the source lists them.
    pub fn build(roads: impl IntoIterator<Item = Set>) -> Result<Self, String> {
        let mut sets: Vec<Set> = Vec::new();
        let mut known: HashMap<Set, u64> = HashMap::new();
        let mut ids = Vec::new();
        for closures in roads {
            let mut set: Set = Vec::with_capacity(closures.len());
            for closure in closures {
                if !set.contains(&closure) {
                    set.push(closure);
                }
            }
            ids.push(if set.is_empty() {
                u64::MAX
            } else {
                match known.entry(set) {
                    Entry::Occupied(id) => *id.get(),
                    Entry::Vacant(slot) => {
                        sets.push(slot.key().clone());
                        *slot.insert(sets.len() as u64 - 1)
                    }
                }
            });
        }
        u32::try_from(sets.len()).map_err(|_| "Too many closure sets")?;
        Ok(Self { sets, roads: Numbers::U64(ids) })
    }

    pub fn valid(&self) -> bool {
        (0..self.roads.len()).all(|road| {
            let set = self.roads.get(road);
            set == u64::MAX || (set as usize) < self.sets.len()
        })
    }

    /// The closures that concern `mode` on `road`, or `None`.
    pub fn closing(&self, road: u32, mode: u8) -> Option<Vec<Closure>> {
        if road as usize >= self.roads.len() {
            return None;
        }
        let set = self.sets.get(usize::try_from(self.roads.get(road as usize)).ok()?)?;
        let closures: Vec<Closure> =
            set.iter().filter(|(modes, _)| modes & mode != 0).map(|(_, closure)| closure.clone()).collect();
        (!closures.is_empty()).then_some(closures)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BIKE, FOOT, PUSH};

    #[test]
    fn a_road_names_only_the_closures_of_the_mode_used() {
        let closure = |kind, condition: &str| Closure { kind, condition: condition.into() };
        let (permit, season, wet) =
            (closure(Kind::Permit, "permit"), closure(Kind::Seasonal, "Nov-May"), closure(Kind::Conditional, "wet"));
        let table = Closures::build([
            vec![],
            vec![(BIKE, season.clone())],
            vec![],
            vec![
                (BIKE | FOOT | PUSH, permit.clone()),
                (BIKE, season.clone()),
                (FOOT | PUSH, wet.clone()),
                (BIKE, season.clone()),
            ],
            vec![],
            vec![],
            vec![],
            vec![(BIKE, season.clone())],
        ])
        .unwrap();
        assert_eq!(table.sets.len(), 2);
        assert_eq!(table.sets[1].len(), 3);
        assert!(table.valid());
        assert_eq!(table.closing(3, BIKE), Some(vec![permit.clone(), season.clone()]));
        assert_eq!(table.closing(3, PUSH), Some(vec![permit.clone(), wet]));
        assert_eq!(table.closing(7, FOOT), None);
        assert_eq!(table.closing(5, BIKE), None);
        assert_eq!(table.closing(99, BIKE), None);
        assert_eq!(Closures::default().closing(0, BIKE), None);
        assert!(!Closures { sets: vec![], roads: Numbers::U8(vec![0]) }.valid());
    }
}
