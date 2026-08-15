//! Fixed-capacity stored-ride catalog state.
//!
//! The board exposes the same `/tracks/RD{id}.ORD` files to two consumers: the companion object
//! store lists every admitted ride, while the on-device menu projects only the newest subset. This
//! type owns the shared id/filename rows and full count so those consumers cannot drift or retain
//! duplicate tables. Media I/O and newest-first projection stay in the borrowed board repository.

use embedded_sdmmc::ShortFileName;
use heapless::Vec;

/// What the media reader found while scanning one admitted ride filename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RideScanRead {
    /// A complete, validated stored-ride object.
    Valid,
    /// The held-back version byte is still zero: an interrupted device-owned save.
    ZeroMarker,
    /// The entry could not be validated without the interrupted-save signature.
    Unreadable,
}

/// The media action produced by [`RideCatalog::observe_scan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RideScanAction {
    /// The entry was appended to the resident catalog.
    Cataloged,
    /// Delete the inert interrupted-save file.
    Sweep,
    /// Keep the file on media but leave it out of the catalog.
    KeepUnlisted,
    /// The resident catalog was full; no row changed.
    Full,
}

/// The catalog action after a committed ride file was removed from media.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RideRemoveAction {
    /// No resident row carried that id; no state changed.
    NotFound,
    /// The row was removed and every admitted file still fits resident capacity.
    Removed,
    /// The row was removed from a truncated catalog; rescan media to admit a hidden row.
    Backfill,
}

/// One bounded, aligned id/filename catalog for every stored ride object.
///
/// Row count comes from the filename vector; ids occupy a fixed array so the two columns cannot
/// acquire independent lengths. `total` is the pre-cap count carried by the wire list header.
#[repr(C)]
pub struct RideCatalog<const N: usize> {
    ids: [u16; N],
    total: u16,
    files: Vec<ShortFileName, N>,
}

impl<const N: usize> RideCatalog<N> {
    /// An empty catalog with no initialized filename rows.
    pub const fn new() -> Self {
        assert!(N <= u16::MAX as usize);
        Self { ids: [0; N], total: 0, files: Vec::new() }
    }

    /// Remove every resident row and reset the full count.
    pub fn clear(&mut self) {
        self.total = 0;
        self.files.clear();
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    #[inline(always)]
    fn is_full(&self) -> bool {
        self.files.is_full()
    }

    /// Full admitted ride count for the list header, including rows beyond resident capacity.
    pub const fn total(&self) -> u16 {
        self.total
    }

    /// Iterate admitted rows in directory order.
    pub fn iter(&self) -> impl Iterator<Item = (u16, &ShortFileName)> {
        self.ids[..self.len()].iter().zip(self.files.iter()).map(|(&id, file)| (id, file))
    }

    /// The first row carrying `id`.
    pub fn get(&self, id: u16) -> Option<(u16, &ShortFileName)> {
        self.index(id).map(|i| (self.ids[i], &self.files[i]))
    }

    /// Collect each cataloged id that satisfies `requested`, at most once and in catalog order.
    ///
    /// The companion's `ackRides` request may contain more entries than the catalog and may repeat
    /// an id. Walking canonical rows rather than request entries prevents duplicates from filling a
    /// bounded temporary before later valid ids are considered.
    pub fn matching_ids(&self, mut requested: impl FnMut(u16) -> bool) -> Vec<u16, N> {
        let mut out = Vec::new();
        for &id in &self.ids[..self.len()] {
            if requested(id) {
                out.push(id).expect("output capacity equals catalog capacity");
            }
        }
        out
    }

    /// Fold one scan verdict into the catalog and return the required media action.
    pub fn observe_scan(&mut self, id: u16, file: ShortFileName, read: RideScanRead) -> RideScanAction {
        match read {
            RideScanRead::Valid => {
                if self.push_row(id, file).is_ok() {
                    RideScanAction::Cataloged
                } else {
                    RideScanAction::Full
                }
            }
            RideScanRead::ZeroMarker => RideScanAction::Sweep,
            RideScanRead::Unreadable => RideScanAction::KeepUnlisted,
        }
    }

    /// Finish a directory scan and atomically replace the list header's full count.
    pub fn finish_scan(&mut self, over_capacity: u16) -> u16 {
        self.total = (self.len() as u16).saturating_add(over_capacity);
        self.total
    }

    /// Remove the first row carrying `id`, reporting whether a hidden media row must be admitted.
    pub fn remove(&mut self, id: u16) -> RideRemoveAction {
        let Some(i) = self.index(id) else { return RideRemoveAction::NotFound };
        let needs_backfill = self.total > self.len() as u16;
        let len = self.len();
        self.ids.copy_within(i + 1..len, i);
        self.files.remove(i);
        self.total = self.total.saturating_sub(1);
        if needs_backfill {
            RideRemoveAction::Backfill
        } else {
            RideRemoveAction::Removed
        }
    }

    fn push_row(&mut self, id: u16, file: ShortFileName) -> Result<(), ShortFileName> {
        if self.is_full() {
            return Err(file);
        }
        self.ids[self.len()] = id;
        let _ = self.files.push(file);
        Ok(())
    }

    fn index(&self, id: u16) -> Option<usize> {
        self.ids[..self.len()].iter().position(|candidate| *candidate == id)
    }
}

impl<const N: usize> Default for RideCatalog<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::ToString;

    fn name(text: &str) -> ShortFileName {
        ShortFileName::create_from_str(text).unwrap()
    }

    fn rows<const N: usize>(catalog: &RideCatalog<N>) -> std::vec::Vec<(u16, std::string::String)> {
        catalog.iter().map(|(id, file)| (id, file.to_string())).collect()
    }

    #[test]
    fn layout_matches_the_independent_id_and_filename_columns() {
        type Catalog = RideCatalog<128>;
        let old_columns = core::mem::size_of::<Vec<u16, 128>>() + core::mem::size_of::<Vec<ShortFileName, 128>>();
        assert_eq!(core::mem::size_of::<Catalog>(), old_columns);
        assert_eq!(core::mem::align_of::<Catalog>(), core::mem::align_of::<Vec<ShortFileName, 128>>());
    }

    #[test]
    fn scan_actions_keep_rows_aligned_and_total_explicit() {
        let mut catalog = RideCatalog::<3>::new();
        assert_eq!(catalog.observe_scan(4, name("RD4.ORD"), RideScanRead::Valid), RideScanAction::Cataloged);
        assert_eq!(catalog.observe_scan(5, name("RD5.ORD"), RideScanRead::Unreadable), RideScanAction::KeepUnlisted);
        assert_eq!(catalog.observe_scan(6, name("RD6.ORD"), RideScanRead::ZeroMarker), RideScanAction::Sweep);
        assert_eq!(catalog.finish_scan(2), 3);
        assert_eq!(rows(&catalog), [(4, "RD4.ORD".into())]);
        assert_eq!(catalog.total(), 3);
    }

    #[test]
    fn full_refusal_and_unknown_remove_are_mutation_free() {
        let mut catalog = RideCatalog::<2>::new();
        assert_eq!(catalog.observe_scan(1, name("RD1.ORD"), RideScanRead::Valid), RideScanAction::Cataloged);
        assert_eq!(catalog.observe_scan(2, name("RD2.ORD"), RideScanRead::Valid), RideScanAction::Cataloged);
        let before = rows(&catalog);
        assert_eq!(catalog.observe_scan(3, name("RD3.ORD"), RideScanRead::Valid), RideScanAction::Full);
        assert_eq!(catalog.remove(9), RideRemoveAction::NotFound);
        assert_eq!(rows(&catalog), before);
    }

    #[test]
    fn removing_from_a_truncated_catalog_requests_backfill() {
        let mut catalog = RideCatalog::<2>::new();
        assert_eq!(catalog.observe_scan(1, name("RD1.ORD"), RideScanRead::Valid), RideScanAction::Cataloged);
        assert_eq!(catalog.observe_scan(2, name("RD2.ORD"), RideScanRead::Valid), RideScanAction::Cataloged);
        assert_eq!(catalog.finish_scan(3), 5);
        assert_eq!(catalog.remove(1), RideRemoveAction::Backfill);
        assert_eq!(rows(&catalog), [(2, "RD2.ORD".into())]);
        assert_eq!(catalog.total(), 4);
    }

    #[test]
    fn removing_a_middle_row_keeps_ids_and_files_aligned() {
        let mut catalog = RideCatalog::<3>::new();
        for id in 7..=9 {
            assert_eq!(
                catalog.observe_scan(id, name(&std::format!("RD{id}.ORD")), RideScanRead::Valid),
                RideScanAction::Cataloged
            );
        }
        assert_eq!(catalog.finish_scan(0), 3);
        assert_eq!(catalog.remove(8), RideRemoveAction::Removed);
        assert_eq!(rows(&catalog), [(7, "RD7.ORD".into()), (9, "RD9.ORD".into())]);
        assert_eq!(catalog.total(), 2);
    }

    #[test]
    fn successive_scans_replace_rows_and_total_without_stale_state() {
        let mut catalog = RideCatalog::<2>::new();
        assert_eq!(catalog.observe_scan(3, name("RD3.ORD"), RideScanRead::Unreadable), RideScanAction::KeepUnlisted);
        assert_eq!(catalog.observe_scan(4, name("RD4.ORD"), RideScanRead::Valid), RideScanAction::Cataloged);
        assert_eq!(catalog.finish_scan(1), 2);
        assert_eq!(rows(&catalog), [(4, "RD4.ORD".into())]);

        catalog.clear();
        assert_eq!(catalog.observe_scan(3, name("RD3.ORD"), RideScanRead::Valid), RideScanAction::Cataloged);
        assert_eq!(catalog.observe_scan(4, name("RD4.ORD"), RideScanRead::Unreadable), RideScanAction::KeepUnlisted);
        assert_eq!(catalog.finish_scan(0), 1);
        assert_eq!(rows(&catalog), [(3, "RD3.ORD".into())]);
        assert_eq!(catalog.total(), catalog.len() as u16);
    }

    #[test]
    fn duplicate_requests_cannot_hide_later_catalog_ids() {
        let mut catalog = RideCatalog::<3>::new();
        for id in 1..=3 {
            assert_eq!(
                catalog.observe_scan(id, name(&std::format!("RD{id}.ORD")), RideScanRead::Valid),
                RideScanAction::Cataloged
            );
        }
        let requests = [1, 1, 1, 1, 2, 3];
        assert_eq!(catalog.matching_ids(|id| requests.contains(&id)).as_slice(), &[1, 2, 3]);
    }

    #[test]
    fn clear_resets_rows_and_total() {
        let mut catalog = RideCatalog::<2>::new();
        assert_eq!(catalog.observe_scan(1, name("RD1.ORD"), RideScanRead::Valid), RideScanAction::Cataloged);
        let _ = catalog.finish_scan(1);
        catalog.clear();
        assert_eq!(catalog.len(), 0);
        assert_eq!(catalog.total(), 0);
    }
}
