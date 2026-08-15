//! Session-stable ids for FAT files whose names carry no durable object id.

use embedded_sdmmc::ShortFileName;
use heapless::Vec;

/// First id in the session-scoped band reserved for side-loaded route and trip files.
///
/// Device-minted durable ids stop before this floor, so the two namespaces cannot collide.
pub const SIDELOAD_ID_BASE: u16 = 0xFF00;

/// Append-only filename-to-id registry shared by every catalog scan in one mounted session.
///
/// A filename keeps its first id even if the file temporarily disappears from later scans. New
/// names receive monotonically increasing ids in directory-observation order. Refusal never
/// mutates the registry or aliases a previously assigned id.
pub struct SideloadIdRegistry<const N: usize> {
    entries: Vec<(ShortFileName, u16), N>,
    // Wider than the public id so exhaustion is represented as 65_536, never wrapped/saturated.
    next: u32,
}

impl<const N: usize> SideloadIdRegistry<N> {
    /// An empty registry whose first assignment is [`SIDELOAD_ID_BASE`].
    #[inline(always)]
    pub const fn new() -> Self {
        Self { entries: Vec::new(), next: SIDELOAD_ID_BASE as u32 }
    }

    /// Return the stable id for `name`, registering it when first observed.
    #[inline(always)]
    pub fn id_for(&mut self, name: &ShortFileName) -> Option<u16> {
        if let Some((_, id)) = self.entries.iter().find(|(registered, _)| registered == name) {
            return Some(*id);
        }
        if self.next > u16::MAX as u32 {
            return None;
        }
        let id = self.next as u16;
        if self.entries.push((name.clone(), id)).is_err() {
            return None;
        }
        self.next += 1;
        Some(id)
    }
}

impl<const N: usize> Default for SideloadIdRegistry<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::format;

    use super::*;
    use crate::ObjectIdSequence;

    fn short(name: &str) -> ShortFileName {
        ShortFileName::create_from_str(name).unwrap()
    }

    #[test]
    fn repeat_name_keeps_its_first_id_and_order() {
        let mut ids = SideloadIdRegistry::<4>::new();
        let alpine = short("ALPINE.OBR");
        let coast = short("COAST.OBR");

        assert_eq!(ids.id_for(&alpine), Some(SIDELOAD_ID_BASE));
        assert_eq!(ids.id_for(&coast), Some(SIDELOAD_ID_BASE + 1));
        assert_eq!(ids.id_for(&alpine), Some(SIDELOAD_ID_BASE));
        assert_eq!(ids.entries.as_slice(), &[(alpine, SIDELOAD_ID_BASE), (coast, SIDELOAD_ID_BASE + 1)]);
    }

    #[test]
    fn route_and_trip_names_share_one_session_namespace() {
        let mut ids = SideloadIdRegistry::<4>::new();
        let route = short("ROUTE.OBR");
        let trip = short("TRIP.OBT");

        assert_eq!(ids.id_for(&route), Some(SIDELOAD_ID_BASE));
        assert_eq!(ids.id_for(&trip), Some(SIDELOAD_ID_BASE + 1));
        assert_eq!(ids.id_for(&route), Some(SIDELOAD_ID_BASE));
        assert_eq!(ids.id_for(&trip), Some(SIDELOAD_ID_BASE + 1));
    }

    #[test]
    fn full_registry_refuses_without_mutating_or_forgetting_existing_names() {
        let mut ids = SideloadIdRegistry::<2>::new();
        let first = short("FIRST.OBR");
        let second = short("SECOND.OBT");
        let refused = short("THIRD.OBR");

        assert_eq!(ids.id_for(&first), Some(SIDELOAD_ID_BASE));
        assert_eq!(ids.id_for(&second), Some(SIDELOAD_ID_BASE + 1));
        let next = ids.next;
        assert_eq!(ids.id_for(&refused), None);
        assert_eq!(ids.next, next);
        assert_eq!(ids.entries.len(), 2);
        assert_eq!(ids.id_for(&first), Some(SIDELOAD_ID_BASE));
    }

    #[test]
    fn id_band_exhaustion_refuses_without_wraparound_or_mutation() {
        let mut ids = SideloadIdRegistry::<257>::new();
        for offset in 0..=u16::MAX - SIDELOAD_ID_BASE {
            let name = short(&format!("F{offset:07}.OBR"));
            assert_eq!(ids.id_for(&name), Some(SIDELOAD_ID_BASE + offset));
        }

        let len = ids.entries.len();
        let next = ids.next;
        assert_eq!(ids.id_for(&short("REFUSED.OBR")), None);
        assert_eq!(ids.entries.len(), len);
        assert_eq!(ids.next, next);
        assert_eq!(ids.id_for(&short("F0000255.OBR")), Some(u16::MAX));
    }

    #[test]
    fn durable_sequence_stops_before_the_sideload_floor() {
        let mut durable = ObjectIdSequence::<SIDELOAD_ID_BASE>::new();
        durable.observe_committed(SIDELOAD_ID_BASE - 2);
        assert_eq!(durable.candidate(), Some(SIDELOAD_ID_BASE - 1));
        durable.observe_committed(SIDELOAD_ID_BASE - 1);
        assert_eq!(durable.candidate(), None);

        let mut side = SideloadIdRegistry::<1>::new();
        assert_eq!(side.id_for(&short("SIDE.OBR")), Some(SIDELOAD_ID_BASE));
    }
}
