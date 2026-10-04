//! Possible closures as a sparse side table: only the few roads with one have entries.
use serde::{Deserialize, Serialize};

/// Why the rider may have no access to a road that the router keeps open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
    /// An access value that the router does not know.
    Unclear,
}

impl Kind {
    /// An access value, not a condition: the router avoids such roads where it can. A condition
    /// depends on the date or the weather of the ride, so it costs nothing.
    pub fn avoided(self) -> bool {
        !matches!(self, Kind::Seasonal | Kind::Conditional)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Closure {
    pub kind: Kind,
    /// The OSM condition, or the OSM access value for the other kinds.
    pub condition: String,
}

/// The router blocks a mode only where the rider surely has no access. Roads where the rider may
/// have no access stay open, and the route reports them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Closures {
    /// `(road, entry)` pairs sorted by road. A road can have more than one entry.
    pub roads: Vec<(u32, u16)>,
    /// Distinct `(modes, closure)` entries, such as `(BIKE, Seasonal "Nov-May")`.
    pub entries: Vec<(u8, Closure)>,
}

impl Closures {
    /// Builds the table from roads in ascending order and the closures of each.
    pub fn build(roads: impl IntoIterator<Item = (u32, Vec<(u8, Closure)>)>) -> Result<Self, String> {
        let mut table = Self::default();
        for (road, closures) in roads {
            let mut entries = Vec::with_capacity(closures.len());
            for closure in closures {
                let entry = match table.entries.iter().position(|known| *known == closure) {
                    Some(entry) => entry,
                    None => {
                        table.entries.push(closure);
                        table.entries.len() - 1
                    }
                };
                entries.push(u16::try_from(entry).map_err(|_| "Too many closures")?);
            }
            entries.sort_unstable();
            entries.dedup();
            table.roads.extend(entries.into_iter().map(|entry| (road, entry)));
        }
        table.valid(u32::MAX).then_some(table).ok_or_else(|| "Unsorted closures".into())
    }

    pub fn valid(&self, roads: u32) -> bool {
        self.roads.windows(2).all(|pair| pair[0] < pair[1])
            && self.roads.iter().all(|&(road, entry)| road < roads && (entry as usize) < self.entries.len())
    }

    /// The closures that concern `mode` on `road`, or `None`.
    pub fn closing(&self, road: u32, mode: u8) -> Option<Vec<Closure>> {
        let start = self.roads.partition_point(|&(id, _)| id < road);
        let closures: Vec<Closure> = self.roads[start..]
            .iter()
            .take_while(|&&(id, _)| id == road)
            .map(|&(_, entry)| &self.entries[entry as usize])
            .filter(|(modes, _)| modes & mode != 0)
            .map(|(_, closure)| closure.clone())
            .collect();
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
            (1, vec![(BIKE, season.clone())]),
            (
                3,
                vec![
                    (BIKE | FOOT | PUSH, permit.clone()),
                    (BIKE, season.clone()),
                    (FOOT | PUSH, wet.clone()),
                    (BIKE, season.clone()),
                ],
            ),
            (7, vec![(BIKE, season.clone())]),
        ])
        .unwrap();
        assert_eq!(table.entries.len(), 3);
        assert_eq!(table.closing(3, BIKE), Some(vec![season.clone(), permit.clone()]));
        assert_eq!(table.closing(3, PUSH), Some(vec![permit.clone(), wet]));
        assert_eq!(table.closing(7, FOOT), None);
        assert_eq!(table.closing(5, BIKE), None);
        assert!(Closures::build([(7, vec![(BIKE, permit.clone())]), (3, vec![(BIKE, permit)])]).is_err());
    }
}
