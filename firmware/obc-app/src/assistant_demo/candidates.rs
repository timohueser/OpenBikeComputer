//! Provisional shop selection for the interaction study. Costs describe a complete visit.

use super::Stop;

pub const MAX_RESULTS: usize = 4;
pub const ON_WAY_EXTRA_M: u32 = 400;
const DISTANCE_ADVANTAGE_M: u32 = 500;
const CLIMB_ADVANTAGE_M: u32 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidates {
    pub indices: [u8; MAX_RESULTS],
    pub len: u8,
}

pub(super) fn select(stops: &[Stop], available: impl Iterator<Item = usize> + Clone) -> Candidates {
    let mut on_way = heapless::Vec::<u8, MAX_RESULTS>::new();
    let mut detours = heapless::Vec::<u8, MAX_RESULTS>::new();
    for id in available.clone() {
        if stops.get(id).is_some_and(Stop::on_way) {
            insert(&mut on_way, stops, id);
        }
    }
    for id in available.clone() {
        let Some(stop) = stops.get(id).filter(|stop| !stop.on_way()) else { continue };
        let useful = on_way.first().is_none_or(|&first| {
            let next = &stops[first as usize];
            let sooner = stop.distance_m.saturating_add(DISTANCE_ADVANTAGE_M) <= next.distance_m;
            let easier =
                stop.distance_m <= next.distance_m && stop.climb_m.saturating_add(CLIMB_ADVANTAGE_M) <= next.climb_m;
            let dominated = available
                .clone()
                .filter_map(|i| stops.get(i))
                .any(|other| other.on_way() && other.distance_m <= stop.distance_m && other.climb_m <= stop.climb_m);
            (sooner || easier) && !dominated
        });
        if useful {
            insert(&mut detours, stops, id);
        }
    }
    let mut result = Candidates { indices: [0; MAX_RESULTS], len: 0 };
    let on_way_slots = MAX_RESULTS - usize::from(!detours.is_empty());
    for &id in on_way.iter().take(on_way_slots).chain(detours.iter()).take(MAX_RESULTS) {
        result.indices[result.len as usize] = id;
        result.len += 1;
    }
    result
}

fn insert(list: &mut heapless::Vec<u8, MAX_RESULTS>, stops: &[Stop], id: usize) {
    let Ok(id) = u8::try_from(id) else { return };
    if list.contains(&id) {
        return;
    }
    let key = |i: u8| (stops[i as usize].distance_m, stops[i as usize].climb_m, i);
    let at = list.iter().position(|&other| key(id) < key(other)).unwrap_or(list.len());
    if at < MAX_RESULTS {
        if list.is_full() {
            list.pop();
        }
        let _ = list.insert(at, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(distance_m: u32, climb_m: u32, extra_m: u32) -> Stop {
        Stop {
            name: "Shop",
            approach: &[(0, 0)],
            distance_m,
            climb_m,
            extra_m,
            extra_climb_m: 0,
            return_m: extra_m / 2,
            return_climb_m: 0,
            outbound: 0,
            continuation: 0,
        }
    }

    #[test]
    fn keeps_distinct_shops_on_the_way_and_removes_a_worse_detour() {
        let stops = [stop(2_000, 50, 100), stop(3_000, 80, 200), stop(4_000, 100, 900)];
        let result = select(&stops, [1, 0, 2, 0, 99].into_iter());
        assert_eq!(result.len, 2);
        assert_eq!(&result.indices[..2], &[0, 1]);
    }

    #[test]
    fn reserves_a_place_for_a_useful_detour_but_not_a_tiny_saving() {
        let stops = [
            stop(2_000, 100, 100),
            stop(3_000, 100, 100),
            stop(4_000, 100, 100),
            stop(5_000, 100, 100),
            stop(1_000, 180, 900),
            stop(1_990, 100, 800),
        ];
        assert_eq!(select(&stops, 0..stops.len()).indices, [0, 1, 2, 4]);
        let easier = [stop(2_000, 100, 100), stop(2_000, 30, 900)];
        assert_eq!(select(&easier, 0..2).len, 2);
    }

    #[test]
    fn empty_and_detour_only_searches_do_not_invent_an_on_way_result() {
        let stops = [stop(900, 100, 800), stop(600, 50, 900)];
        assert_eq!(select(&stops, core::iter::empty()).len, 0);
        assert_eq!(&select(&stops, 0..2).indices[..2], &[1, 0]);
    }
}
