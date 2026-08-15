//! Fixed-capacity trip catalog state.
//!
//! The board keeps one decoded trip catalog for the on-device folders and the companion-link
//! repository. This type owns the three parallel columns as one mutation boundary, so an insert,
//! replacement, or removal cannot leave an id pointing at another file's metadata. The columns
//! deliberately retain their compact structure-of-arrays layout: on the device it is byte-for-byte
//! the same allocation as the three `heapless::Vec`s it replaces. The id column's old `usize`
//! length word is represented as two `u16`s: one full list total and the monotonic upload sequence;
//! the aligned filename column already carries the resident-row length.

use crate::ObjectIdSequence;
use embedded_sdmmc::ShortFileName;
use heapless::Vec;

/// What the media reader found while scanning one admitted trip filename.
///
/// Only the exact held-marker signature is sweepable. A generic read failure stays on the card and
/// merely remains out of the catalog, because it may be a transient media error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TripScanRead<M> {
    /// A fully validated trip and its decoded resident metadata.
    Valid(M),
    /// The first four bytes are zero: an inert interrupted commit owned by the device.
    ZeroMarker,
    /// The entry could not be validated, without the device's interrupted-commit signature.
    Unreadable,
}

/// The media action produced by [`TripCatalog::observe_scan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TripScanAction {
    /// The entry was appended to the resident catalog.
    Cataloged,
    /// Delete the inert zero-marker file.
    Sweep,
    /// Keep the file on media but leave it out of the catalog.
    KeepUnlisted,
    /// The resident catalog was full; no column changed.
    Full,
}

/// The catalog action after a committed file was removed from media.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TripRemoveAction {
    /// No resident row carried that id; no state changed.
    NotFound,
    /// The row was removed and every admitted file still fits resident capacity.
    Removed,
    /// The row was removed from a truncated catalog; rescan media to admit a hidden row.
    Backfill,
}

impl<M, const N: usize, const ID_LIMIT: u16> TripCatalog<M, N, ID_LIMIT> {
    /// Observe one admitted scan result. The upload sequence advances past only a cataloged uploaded
    /// object, so zero-marker, unreadable, full, and side-load entries never reserve an id.
    pub fn observe_scan(
        &mut self,
        id: u16,
        uploaded: bool,
        file: ShortFileName,
        read: TripScanRead<M>,
    ) -> TripScanAction {
        let action = self.apply_scan(id, file, read);
        if uploaded && action == TripScanAction::Cataloged {
            self.sequence.observe_committed(id);
        }
        action
    }
}

/// Resolve one list row's lazy CRC as `(served, freshly_computed_to_persist)`.
pub const fn trip_crc(stored: Option<u32>, computed: Option<u32>) -> (u32, Option<u32>) {
    match stored {
        Some(crc) => (crc, None),
        None => match computed {
            Some(crc) => (crc, Some(crc)),
            None => (0, None),
        },
    }
}

/// A bounded trip catalog with aligned id, filename, and metadata columns.
///
/// `M` is the format owner's decoded metadata (`obc_route::TripMeta` on the board). Keeping it
/// generic avoids an upward `obc-storage -> obc-route` dependency while this type still owns the
/// platform-independent alignment and mutation policy.
#[repr(C)]
pub struct TripCatalog<M, const N: usize, const ID_LIMIT: u16> {
    ids: [u16; N],
    total: u16,
    sequence: ObjectIdSequence<ID_LIMIT>,
    files: Vec<ShortFileName, N>,
    metas: Vec<M, N>,
}

impl<M, const N: usize, const ID_LIMIT: u16> TripCatalog<M, N, ID_LIMIT> {
    /// An empty catalog with no initialized payload slots.
    pub const fn new() -> Self {
        assert!(N <= u16::MAX as usize);
        Self { ids: [0; N], total: 0, sequence: ObjectIdSequence::new(), files: Vec::new(), metas: Vec::new() }
    }

    /// Remove every resident row and its full count without rewinding the monotonic upload sequence.
    pub fn clear(&mut self) {
        self.total = 0;
        self.files.clear();
        self.metas.clear();
    }

    #[inline(always)]
    pub fn row_count(&self) -> usize {
        self.files.len()
    }

    /// Full admitted trip count for the list header, including rows beyond resident capacity.
    pub const fn total(&self) -> u16 {
        self.total
    }

    /// Raise the next upload candidate to a persisted exclusive floor without writing it again.
    pub fn observe_floor(&mut self, floor: u16) {
        self.sequence.observe_floor(floor);
    }

    /// Reserve the current upload id without consuming it.
    pub const fn candidate(&self) -> Option<u16> {
        self.sequence.candidate()
    }

    /// Advance after the reserved id became visible and hand its new exclusive floor to persistence.
    pub fn commit(&mut self, persist_floor: impl FnOnce(u16)) -> u16 {
        self.sequence.commit(persist_floor)
    }

    #[inline(always)]
    pub fn is_full(&self) -> bool {
        self.row_count() == N
    }

    /// Iterate the aligned rows in directory/commit order.
    pub fn iter(&self) -> impl Iterator<Item = (u16, &ShortFileName, &M)> {
        self.ids[..self.row_count()]
            .iter()
            .zip(self.files.iter())
            .zip(self.metas.iter())
            .map(|((&id, file), meta)| (id, file, meta))
    }

    /// The first row carrying `id`, matching the pre-repository lookup rule.
    pub fn get(&self, id: u16) -> Option<(u16, &ShortFileName, &M)> {
        self.index(id).map(|i| (self.ids[i], &self.files[i], &self.metas[i]))
    }

    /// Find the first row with matching whole-object CRC and byte length. Unknown CRCs cannot
    /// deduplicate, and length is queried only after the cheap CRC match.
    pub fn find_by_content(
        &self,
        crc: u32,
        byte_len: u32,
        mut crc_for: impl FnMut(u16) -> Option<u32>,
        mut len_for: impl FnMut(&ShortFileName) -> Option<u32>,
    ) -> Option<u16> {
        self.iter()
            .find(|(id, file, _)| crc_for(*id) == Some(crc) && len_for(file) == Some(byte_len))
            .map(|(id, _, _)| id)
    }

    /// The row at `index`, for bounded consumers that must release the catalog borrow before
    /// performing media I/O.
    pub fn entry_at(&self, index: usize) -> Option<(u16, &ShortFileName, &M)> {
        if index >= self.row_count() {
            return None;
        }
        Some((self.ids[index], self.files.get(index)?, self.metas.get(index)?))
    }

    /// Publish one newly committed row and advance the full catalog count. Refusal is mutation-free.
    pub fn insert(&mut self, id: u16, file: ShortFileName, meta: M) -> Result<(), (ShortFileName, M)> {
        self.push_row(id, file, meta)?;
        self.total = self.total.saturating_add(1);
        Ok(())
    }

    fn push_row(&mut self, id: u16, file: ShortFileName, meta: M) -> Result<(), (ShortFileName, M)> {
        if self.is_full() {
            return Err((file, meta));
        }
        // The shared fullness precondition makes all three pushes infallible and keeps the columns
        // aligned without rollback code.
        self.ids[self.row_count()] = id;
        let _ = self.files.push(file);
        let _ = self.metas.push(meta);
        Ok(())
    }

    /// Finish a directory scan and atomically replace the list header's full count.
    pub fn finish_scan(&mut self, over_capacity: u16) -> u16 {
        self.total = (self.row_count() as u16).saturating_add(over_capacity);
        self.total
    }

    /// Fold one scan verdict into the catalog and return the media action. A zero-marker or
    /// transiently unreadable entry never mutates resident state.
    fn apply_scan(&mut self, id: u16, file: ShortFileName, read: TripScanRead<M>) -> TripScanAction {
        match read {
            TripScanRead::Valid(meta) => {
                if self.push_row(id, file, meta).is_ok() {
                    TripScanAction::Cataloged
                } else {
                    TripScanAction::Full
                }
            }
            TripScanRead::ZeroMarker => TripScanAction::Sweep,
            TripScanRead::Unreadable => TripScanAction::KeepUnlisted,
        }
    }

    /// Replace the first row carrying `id` without moving its ordering slot.
    pub fn replace(&mut self, id: u16, file: ShortFileName, meta: M) -> Result<(), (ShortFileName, M)> {
        let Some(i) = self.index(id) else { return Err((file, meta)) };
        self.files[i] = file;
        self.metas[i] = meta;
        Ok(())
    }

    /// Remove the first row carrying `id`, shifting every column together and reporting whether an
    /// over-capacity media entry must be backfilled. Unknown ids are mutation-free.
    pub fn remove(&mut self, id: u16) -> TripRemoveAction {
        let Some(i) = self.index(id) else { return TripRemoveAction::NotFound };
        let needs_backfill = self.total > self.row_count() as u16;
        let len = self.row_count();
        self.ids.copy_within(i + 1..len, i);
        self.files.remove(i);
        self.metas.remove(i);
        self.total = self.total.saturating_sub(1);
        if needs_backfill {
            TripRemoveAction::Backfill
        } else {
            TripRemoveAction::Removed
        }
    }

    fn index(&self, id: u16) -> Option<usize> {
        self.ids[..self.row_count()].iter().position(|candidate| *candidate == id)
    }
}

impl<M, const N: usize, const ID_LIMIT: u16> Default for TripCatalog<M, N, ID_LIMIT> {
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

    const ID_LIMIT: u16 = 0xff00;

    fn rows<const N: usize>(catalog: &TripCatalog<u8, N, ID_LIMIT>) -> std::vec::Vec<(u16, std::string::String, u8)> {
        catalog.iter().map(|(id, file, meta)| (id, file.to_string(), *meta)).collect()
    }

    #[test]
    fn layout_is_exactly_the_three_columns_it_replaces() {
        type Catalog = TripCatalog<[u32; 3], 16, ID_LIMIT>;
        assert_eq!(core::mem::size_of::<ObjectIdSequence<ID_LIMIT>>(), core::mem::size_of::<u16>());
        let columns = core::mem::size_of::<Vec<u16, 16>>()
            + core::mem::size_of::<Vec<ShortFileName, 16>>()
            + core::mem::size_of::<Vec<[u32; 3], 16>>();
        assert_eq!(core::mem::size_of::<Catalog>(), columns);
        assert_eq!(core::mem::align_of::<Catalog>(), core::mem::align_of::<Vec<[u32; 3], 16>>());
    }

    #[test]
    fn successive_scans_keep_rows_total_and_candidate_coherent() {
        let mut catalog = TripCatalog::<u8, 2, ID_LIMIT>::new();
        assert_eq!(
            catalog.observe_scan(5, true, name("TP5.OBT"), TripScanRead::Unreadable),
            TripScanAction::KeepUnlisted
        );
        assert_eq!(catalog.finish_scan(0), 0);
        assert_eq!(catalog.total(), 0);
        assert_eq!(catalog.row_count(), 0);
        assert_eq!(catalog.candidate(), Some(0));

        catalog.clear();
        assert_eq!(catalog.observe_scan(5, true, name("TP5.OBT"), TripScanRead::Valid(5)), TripScanAction::Cataloged);
        assert_eq!(catalog.finish_scan(3), 4);
        assert_eq!(catalog.total(), 4);
        assert_eq!(catalog.row_count(), 1);
        assert_eq!(rows(&catalog), [(5, "TP5.OBT".into(), 5)]);
        assert_eq!(catalog.candidate(), Some(6), "a recovered valid filename must raise the in-memory floor");

        catalog.clear();
        assert_eq!(
            catalog.observe_scan(5, true, name("TP5.OBT"), TripScanRead::Unreadable),
            TripScanAction::KeepUnlisted
        );
        assert_eq!(catalog.finish_scan(0), 0);
        assert_eq!(catalog.total(), 0);
        assert_eq!(catalog.row_count(), 0);
        assert_eq!(catalog.candidate(), Some(6), "a transient failure must never rewind the sequence");
    }

    #[test]
    fn mixed_uploaded_and_sideload_rows_keep_scan_order() {
        let mut catalog = TripCatalog::<u8, 4, ID_LIMIT>::new();
        assert_eq!(catalog.observe_scan(7, true, name("TP7.OBT"), TripScanRead::Valid(1)), TripScanAction::Cataloged);
        assert_eq!(
            catalog.observe_scan(0xff00, false, name("TOUR.OBT"), TripScanRead::Valid(2)),
            TripScanAction::Cataloged
        );
        assert_eq!(catalog.candidate(), Some(8));
        assert_eq!(rows(&catalog), [(7, "TP7.OBT".into(), 1), (0xff00, "TOUR.OBT".into(), 2)]);
    }

    #[test]
    fn zero_marker_is_swept_but_transient_unreadable_is_kept_without_mutation() {
        let mut catalog = TripCatalog::<u8, 4, ID_LIMIT>::new();
        catalog.insert(3, name("TP3.OBT"), 9).unwrap();
        let before = rows(&catalog);

        assert_eq!(catalog.observe_scan(4, true, name("TP4.OBT"), TripScanRead::ZeroMarker), TripScanAction::Sweep);
        assert_eq!(rows(&catalog), before);
        assert_eq!(
            catalog.observe_scan(5, true, name("TP5.OBT"), TripScanRead::Unreadable),
            TripScanAction::KeepUnlisted
        );
        assert_eq!(rows(&catalog), before);
        assert_eq!(catalog.candidate(), Some(0), "failed reads must not burn the id candidate");
    }

    #[test]
    fn full_refusal_does_not_mutate_any_column() {
        let mut catalog = TripCatalog::<u8, 2, ID_LIMIT>::new();
        catalog.insert(1, name("TP1.OBT"), 1).unwrap();
        catalog.insert(2, name("TP2.OBT"), 2).unwrap();
        let before = rows(&catalog);

        assert_eq!(catalog.observe_scan(3, true, name("TP3.OBT"), TripScanRead::Valid(3)), TripScanAction::Full);
        assert_eq!(rows(&catalog), before);
        assert_eq!(catalog.candidate(), Some(0));
    }

    #[test]
    fn replace_and_remove_keep_columns_aligned() {
        let mut catalog = TripCatalog::<u8, 4, ID_LIMIT>::new();
        catalog.insert(1, name("TP1.OBT"), 10).unwrap();
        catalog.insert(2, name("TP2.OBT"), 20).unwrap();
        catalog.insert(3, name("TP3.OBT"), 30).unwrap();

        catalog.replace(2, name("EDIT.OBT"), 21).unwrap();
        assert_eq!(catalog.remove(1), TripRemoveAction::Removed);
        assert_eq!(rows(&catalog), [(2, "EDIT.OBT".into(), 21), (3, "TP3.OBT".into(), 30)]);
        assert_eq!(catalog.total(), 2);
    }

    #[test]
    fn unknown_replace_and_remove_are_mutation_free() {
        let mut catalog = TripCatalog::<u8, 2, ID_LIMIT>::new();
        catalog.insert(1, name("TP1.OBT"), 1).unwrap();
        let before = rows(&catalog);

        assert!(catalog.replace(9, name("TP9.OBT"), 9).is_err());
        assert_eq!(catalog.remove(9), TripRemoveAction::NotFound);
        assert_eq!(rows(&catalog), before);
    }

    #[test]
    fn removing_from_a_truncated_catalog_requires_media_backfill() {
        let mut catalog = TripCatalog::<u8, 2, ID_LIMIT>::new();
        catalog.insert(1, name("TP1.OBT"), 1).unwrap();
        catalog.insert(2, name("TP2.OBT"), 2).unwrap();
        assert_eq!(catalog.finish_scan(3), 5);

        assert_eq!(catalog.remove(1), TripRemoveAction::Backfill);
        assert_eq!(catalog.row_count(), 1);
        assert_eq!(catalog.total(), 4);
        assert_eq!(rows(&catalog), [(2, "TP2.OBT".into(), 2)]);
    }

    #[test]
    fn dedup_requires_known_crc_and_equal_length() {
        let mut catalog = TripCatalog::<u8, 3, ID_LIMIT>::new();
        catalog.insert(4, name("TP4.OBT"), 0).unwrap();
        catalog.insert(5, name("TP5.OBT"), 0).unwrap();
        let crc = |id| (id == 5).then_some(0x1234_5678);
        assert_eq!(catalog.find_by_content(0x1234_5678, 80, crc, |_| Some(80)), Some(5));
        assert_eq!(catalog.find_by_content(0x1234_5678, 81, crc, |_| Some(80)), None);
        assert_eq!(catalog.find_by_content(0x9999_9999, 80, |_| None, |_| Some(80)), None);
    }

    #[test]
    fn crc_unknown_and_lazy_fill_decisions_are_explicit() {
        assert_eq!(trip_crc(Some(7), Some(8)), (7, None));
        assert_eq!(trip_crc(None, Some(8)), (8, Some(8)));
        assert_eq!(trip_crc(None, None), (0, None));
    }
}
