//! Fault classification when the mounted flat store provides no readable map.

/// An empty, complete map listing reports no map. A map object or an incomplete listing reports
/// an unreadable map, so a failed catalog read cannot present a populated card as empty.
pub fn flat_boot_fault(map_objects: usize, listing_complete: bool) -> crate::BootFault {
    if map_objects == 0 && listing_complete {
        crate::BootFault::NoMap
    } else {
        crate::BootFault::BadMap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_complete_flat_catalog_distinguishes_absent_and_unreadable_maps() {
        assert_eq!(flat_boot_fault(0, true), crate::BootFault::NoMap, "no map objects is the only NO MAP");
        // A present map that failed to open is unreadable, not absent.
        assert_eq!(flat_boot_fault(1, true), crate::BootFault::BadMap);
        assert_eq!(flat_boot_fault(1_024, true), crate::BootFault::BadMap, "and a catalog full of them");
    }

    /// An incomplete listing cannot prove that the card has no map.
    #[test]
    fn a_flat_listing_that_stopped_short_is_never_no_map() {
        assert_eq!(
            flat_boot_fault(0, false),
            crate::BootFault::BadMap,
            "an incomplete listing over an apparently empty catalog is MAP UNREADABLE, not NO MAP"
        );
        assert_eq!(flat_boot_fault(4, false), crate::BootFault::BadMap);
    }
}
