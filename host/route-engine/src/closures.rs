//! Seasonal closures as a sparse side table: only the few roads with a closure have entries.
use serde::{Deserialize, Serialize};

/// Closures whose conditions name only months, days or seasons. The router keeps such roads open,
/// because only the rider's date decides whether a closure applies; the route reports them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Closures {
    /// `(road, entry)` pairs sorted by road. A road can have more than one entry.
    pub roads: Vec<(u32, u16)>,
    /// Distinct `(modes, OSM condition)` entries, such as `(BIKE, "Nov-May")`.
    pub entries: Vec<(u8, String)>,
}

impl Closures {
    /// Builds the table from roads in ascending order and the closures of each.
    pub fn build(roads: impl IntoIterator<Item = (u32, Vec<(u8, String)>)>) -> Result<Self, String> {
        let mut table = Self::default();
        for (road, closures) in roads {
            for closure in closures {
                let entry = match table.entries.iter().position(|known| *known == closure) {
                    Some(entry) => entry,
                    None => {
                        table.entries.push(closure);
                        table.entries.len() - 1
                    }
                };
                table.roads.push((road, u16::try_from(entry).map_err(|_| "Too many seasonal closures")?));
            }
        }
        table.valid(u32::MAX).then_some(table).ok_or_else(|| "Unsorted seasonal closures".into())
    }

    pub fn valid(&self, roads: u32) -> bool {
        self.roads.windows(2).all(|pair| pair[0] < pair[1])
            && self.roads.iter().all(|&(road, entry)| road < roads && (entry as usize) < self.entries.len())
    }

    /// The conditions that close `mode` on `road`, or `None`.
    pub fn closing(&self, road: u32, mode: u8) -> Option<String> {
        let start = self.roads.partition_point(|&(id, _)| id < road);
        let conditions: Vec<&str> = self.roads[start..]
            .iter()
            .take_while(|&&(id, _)| id == road)
            .map(|&(_, entry)| &self.entries[entry as usize])
            .filter(|(modes, _)| modes & mode != 0)
            .map(|(_, condition)| condition.as_str())
            .collect();
        (!conditions.is_empty()).then(|| conditions.join("; "))
    }

    /// The table for new road IDs, where `ids[new]` is the old road ID.
    pub fn select(&self, ids: impl IntoIterator<Item = u32>) -> Self {
        let mut roads: Vec<(u32, u16)> = ids
            .into_iter()
            .enumerate()
            .flat_map(|(new, old)| {
                let new = new as u32;
                let start = self.roads.partition_point(|&(id, _)| id < old);
                self.roads[start..].iter().take_while(move |&&(id, _)| id == old).map(move |&(_, entry)| (new, entry))
            })
            .collect();
        roads.sort_unstable();
        Self { roads, entries: self.entries.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BIKE, FOOT, PUSH};

    #[test]
    fn a_road_names_only_the_conditions_of_the_mode_used() {
        let table = Closures::build([
            (3, vec![(BIKE, "Nov-May".into()), (FOOT | PUSH, "Mar 1-Jul 31".into())]),
            (7, vec![(BIKE, "Nov-May".into())]),
        ])
        .unwrap();
        assert_eq!(table.entries.len(), 2);
        assert_eq!(table.closing(3, BIKE).as_deref(), Some("Nov-May"));
        assert_eq!(table.closing(3, PUSH).as_deref(), Some("Mar 1-Jul 31"));
        assert_eq!(table.closing(7, FOOT), None);
        assert_eq!(table.closing(5, BIKE), None);
        let selected = table.select([7, 5, 3]);
        assert_eq!(
            (selected.closing(0, BIKE).as_deref(), selected.closing(2, FOOT).as_deref()),
            (Some("Nov-May"), Some("Mar 1-Jul 31"))
        );
        assert!(Closures::build([(7, vec![(BIKE, "Nov-May".into())]), (3, vec![(BIKE, "Nov-May".into())])]).is_err());
    }
}
