//! Fixed-capacity stored-object catalog policy.
//!
//! Routes and trips keep different media behavior, sidecars, and repository APIs, but share this
//! bounded owner for aligned rows, durable upload ids, and per-session side-load names. Each typed
//! catalog owns its own registry, so one object class can never exhaust the other's ids.

use crate::ObjectIdSequence;
use embedded_sdmmc::ShortFileName;
use heapless::Vec;

/// What a typed media reader found for one admitted filename.
///
/// Only the device-owned zero marker is sweepable. A generic failure stays on media and out of the
/// resident catalog so a transient card error cannot become destructive cleanup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanRead<M> {
    /// A fully validated object and the metadata its catalog row retains.
    Valid(M),
    /// The held validity marker is still zero: an inert interrupted commit.
    ZeroMarker,
    /// Validation or media reading failed without the owned interrupted-commit signature.
    Unreadable,
}

/// Catalog mutation selected for one [`ScanRead`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanAction {
    /// A validated row was appended and is now visible.
    Cataloged,
    /// The exact zero-marker file may be removed by the media owner.
    Sweep,
    /// Keep the file on media but do not publish it in this scan.
    KeepUnlisted,
    /// The bounded resident columns cannot admit another validated row.
    Full,
}

/// Result of removing one canonical row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveAction {
    /// No row carried that id; the catalog was not mutated.
    NotFound,
    /// The row was removed and no hidden media entry needs admission.
    Removed,
    /// The row was removed from an over-cap catalog; the caller must rescan before publication.
    Backfill,
}

/// One fixed-capacity structure-of-arrays catalog shared by a single stored-object class.
///
/// Filename length is the authoritative resident row count; ids and metadata are mutated only with
/// the corresponding filename. Durable upload sequencing and the per-type, append-only session-name
/// registry live here as the same ownership boundary, while file I/O and sidecar formats remain with
/// the typed board repository.
#[repr(C)]
pub struct Catalog<M, const N: usize, const ID_LIMIT: u16> {
    ids: [u16; N],
    total: u16,
    sequence: ObjectIdSequence<ID_LIMIT>,
    files: Vec<ShortFileName, N>,
    metas: Vec<M, N>,
    sideload_names: Vec<ShortFileName, N>,
    sidebands_bound: u8,
}

impl<M, const N: usize, const ID_LIMIT: u16> Catalog<M, N, ID_LIMIT> {
    pub const fn new() -> Self {
        assert!(N <= u16::MAX as usize);
        Self {
            ids: [0; N],
            total: 0,
            sequence: ObjectIdSequence::new(),
            files: Vec::new(),
            metas: Vec::new(),
            sideload_names: Vec::new(),
            sidebands_bound: 0,
        }
    }

    /// Clear media-derived rows without rewinding durable ids or this session's side-load names.
    pub fn clear(&mut self) {
        self.total = 0;
        self.files.clear();
        self.metas.clear();
    }

    #[inline(always)]
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_full(&self) -> bool {
        self.files.is_full()
    }

    pub const fn total(&self) -> u16 {
        self.total
    }

    pub fn observe_floor(&mut self, floor: u16) {
        self.sequence.observe_floor(floor);
    }

    /// Return the next durable upload id without reserving or burning it.
    pub const fn candidate(&self) -> Option<u16> {
        self.sequence.candidate()
    }

    /// Commit the current durable candidate and persist its successor exactly once.
    pub fn commit(&mut self, persist_floor: impl FnOnce(u16)) -> u16 {
        self.sequence.commit(persist_floor)
    }

    /// Generated ids are durable only below the reserved session band. Every other admitted name
    /// receives this catalog's stable session id in `ID_LIMIT..=0xfffe`; `0xffff` is never minted.
    pub fn id_for_scan(&mut self, parsed: Option<u16>, file: &ShortFileName) -> Option<(u16, bool)> {
        if let Some(id) = parsed.filter(|id| *id < ID_LIMIT) {
            return Some((id, true));
        }
        if let Some(index) = self.sideload_names.iter().position(|name| name == file) {
            return ID_LIMIT.checked_add(index as u16).filter(|id| *id < u16::MAX).map(|id| (id, false));
        }
        let id = ID_LIMIT.checked_add(self.sideload_names.len() as u16)?;
        if id == u16::MAX || self.sideload_names.is_full() {
            return None;
        }
        let _ = self.sideload_names.push(file.clone());
        Some((id, false))
    }

    /// Whether a typed side-band namespace was scrubbed and durably rewritten this session.
    pub const fn sideband_bound(&self, bit: u8) -> bool {
        assert!(bit < u8::BITS as u8);
        self.sidebands_bound & (1 << bit) != 0
    }

    /// Record a side-band scrub only after its full rewrite succeeds; failure leaves it masked.
    pub fn record_sideband_rewrite(&mut self, bit: u8, persisted: bool) {
        assert!(bit < u8::BITS as u8);
        if persisted {
            self.sidebands_bound |= 1 << bit;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (u16, &ShortFileName, &M)> {
        self.ids[..self.len()]
            .iter()
            .zip(self.files.iter())
            .zip(self.metas.iter())
            .map(|((&id, file), meta)| (id, file, meta))
    }

    pub fn get(&self, id: u16) -> Option<(u16, &ShortFileName, &M)> {
        let i = self.index(id)?;
        Some((self.ids[i], &self.files[i], &self.metas[i]))
    }

    /// Observe one admitted filename. Refusals and unreadable rows do not burn durable ids.
    pub fn observe_scan(&mut self, id: u16, uploaded: bool, file: ShortFileName, read: ScanRead<M>) -> ScanAction {
        let action = match read {
            ScanRead::Valid(meta) => self.push_row(id, file, meta).map_or(ScanAction::Full, |_| ScanAction::Cataloged),
            ScanRead::ZeroMarker => ScanAction::Sweep,
            ScanRead::Unreadable => ScanAction::KeepUnlisted,
        };
        if uploaded && action == ScanAction::Cataloged {
            self.sequence.observe_committed(id);
        }
        action
    }

    /// Finish a scan with the count of raw names hidden beyond resident capacity.
    pub fn finish_scan(&mut self, over_capacity: u16) {
        self.total = (self.len() as u16).saturating_add(over_capacity);
    }

    /// Append a complete row or return its owned values without mutation when full.
    pub fn insert(&mut self, id: u16, file: ShortFileName, meta: M) -> Result<(), (ShortFileName, M)> {
        self.push_row(id, file, meta)?;
        self.total = self.total.saturating_add(1);
        Ok(())
    }

    /// Atomically publish the current fresh candidate, committing/persisting it only after the row
    /// is known to fit. Capacity or id exhaustion returns the owned values without mutation.
    pub fn insert_committed(
        &mut self,
        file: ShortFileName,
        meta: M,
        persist_floor: impl FnOnce(u16),
    ) -> Result<u16, (ShortFileName, M)> {
        if self.is_full() || self.candidate().is_none() {
            return Err((file, meta));
        }
        let id = self.sequence.commit(persist_floor);
        let inserted = self.push_row(id, file, meta);
        debug_assert!(inserted.is_ok());
        self.total = self.total.saturating_add(1);
        Ok(id)
    }

    /// Replace one complete row in place, preserving order and total; unknown ids do not mutate.
    pub fn replace(&mut self, id: u16, file: ShortFileName, meta: M) -> Result<(), (ShortFileName, M)> {
        let Some(i) = self.index(id) else { return Err((file, meta)) };
        self.files[i] = file;
        self.metas[i] = meta;
        Ok(())
    }

    /// Adopt one locally-created file without rescanning unrelated rows.
    pub fn adopt_visible(
        &mut self,
        id: u16,
        uploaded: bool,
        file: ShortFileName,
        meta: M,
        already_counted: bool,
    ) -> Result<bool, (ShortFileName, M)> {
        if self.index(id).is_some() {
            self.replace(id, file, meta)?;
            return Ok(false);
        }
        self.push_row(id, file, meta)?;
        if !already_counted {
            self.total = self.total.saturating_add(1);
        }
        if uploaded {
            self.sequence.observe_committed(id);
        }
        Ok(true)
    }

    /// Remove an aligned row and report whether over-cap media must be rescanned for backfill.
    pub fn remove(&mut self, id: u16) -> RemoveAction {
        let Some(i) = self.index(id) else { return RemoveAction::NotFound };
        let backfill = self.total > self.len() as u16;
        let len = self.len();
        self.ids.copy_within(i + 1..len, i);
        self.files.remove(i);
        self.metas.remove(i);
        self.total = self.total.saturating_sub(1);
        if backfill {
            RemoveAction::Backfill
        } else {
            RemoveAction::Removed
        }
    }

    pub fn find_by_content(
        &self,
        crc: u32,
        byte_len: u32,
        mut crc_for: impl FnMut(u16) -> Option<u32>,
        mut len_for: impl FnMut(&ShortFileName, &M) -> Option<u32>,
    ) -> Option<u16> {
        self.iter()
            .find(|(id, file, meta)| crc_for(*id) == Some(crc) && len_for(file, meta) == Some(byte_len))
            .map(|(id, _, _)| id)
    }

    fn push_row(&mut self, id: u16, file: ShortFileName, meta: M) -> Result<(), (ShortFileName, M)> {
        if self.is_full() {
            return Err((file, meta));
        }
        self.ids[self.len()] = id;
        let _ = self.files.push(file);
        let _ = self.metas.push(meta);
        Ok(())
    }

    fn index(&self, id: u16) -> Option<usize> {
        self.ids[..self.len()].iter().position(|candidate| *candidate == id)
    }
}

impl<M, const N: usize, const ID_LIMIT: u16> Default for Catalog<M, N, ID_LIMIT> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::{String, ToString};

    const LIMIT: u16 = 0xff00;
    fn name(text: &str) -> ShortFileName {
        ShortFileName::create_from_str(text).unwrap()
    }
    fn rows<const N: usize>(c: &Catalog<u32, N, LIMIT>) -> std::vec::Vec<(u16, String, u32)> {
        c.iter().map(|(id, file, meta)| (id, file.to_string(), *meta)).collect()
    }

    #[test]
    fn scan_alignment_backfill_and_non_burning_failures() {
        let mut c = Catalog::<u32, 2, LIMIT>::new();
        assert_eq!(c.observe_scan(5, true, name("RT5.OBR"), ScanRead::Unreadable), ScanAction::KeepUnlisted);
        assert_eq!(c.observe_scan(6, true, name("RT6.OBR"), ScanRead::ZeroMarker), ScanAction::Sweep);
        assert_eq!(c.candidate(), Some(0));
        assert_eq!(c.observe_scan(5, true, name("RT5.OBR"), ScanRead::Valid(50)), ScanAction::Cataloged);
        assert_eq!(c.observe_scan(6, true, name("RT6.OBR"), ScanRead::Valid(60)), ScanAction::Cataloged);
        c.finish_scan(1);
        assert_eq!(c.candidate(), Some(7));
        assert_eq!(c.remove(5), RemoveAction::Backfill);
        assert_eq!(rows(&c), [(6, "RT6.OBR".into(), 60)]);
        assert_eq!(c.total(), 2);
    }

    #[test]
    fn inserts_replacements_and_refusals_keep_columns_aligned() {
        let mut c = Catalog::<u32, 2, LIMIT>::new();
        let mut floors = std::vec::Vec::new();
        assert_eq!(c.insert_committed(name("RT0.OBR"), 10, |v| floors.push(v)), Ok(0));
        assert_eq!(c.replace(0, name("RT0.OBR"), 11), Ok(()));
        c.insert(7, name("RT7.OBR"), 70).unwrap();
        let before = rows(&c);
        assert!(c.insert_committed(name("RT1.OBR"), 20, |v| floors.push(v)).is_err());
        assert_eq!(rows(&c), before);
        assert_eq!(floors, [1]);

        let mut exhausted = Catalog::<u8, 2, 1>::new();
        exhausted.observe_floor(1);
        let mut persisted = false;
        assert!(exhausted.insert_committed(name("RT1.OBR"), 1, |_| persisted = true).is_err());
        assert_eq!((exhausted.len(), exhausted.total(), exhausted.candidate()), (0, 0, None));
        assert!(!persisted, "id exhaustion must not persist or publish");
    }

    #[test]
    fn typed_registries_are_independent_and_never_mint_ffff() {
        let mut routes = Catalog::<u32, 2, LIMIT>::new();
        let mut trips = Catalog::<u8, 2, LIMIT>::new();
        assert_eq!(routes.id_for_scan(None, &name("A.OBR")), Some((0xff00, false)));
        assert_eq!(routes.id_for_scan(None, &name("B.OBR")), Some((0xff01, false)));
        assert_eq!(routes.id_for_scan(None, &name("C.OBR")), None);
        routes.clear();
        assert_eq!(routes.id_for_scan(None, &name("A.OBR")), Some((0xff00, false)));
        assert_eq!(routes.id_for_scan(None, &name("C.OBR")), None, "session tombstones are never reused");
        assert_eq!(trips.id_for_scan(None, &name("A.OBT")), Some((0xff00, false)));
        assert_eq!(trips.id_for_scan(Some(7), &name("TP7.OBT")), Some((7, true)));

        let mut edge = Catalog::<u8, 2, 0xfffe>::new();
        assert_eq!(edge.id_for_scan(Some(u16::MAX), &name("TP65535.OBT")), Some((0xfffe, false)));
        assert_eq!(edge.id_for_scan(None, &name("B.OBT")), None);
        assert_eq!(edge.id_for_scan(Some(u16::MAX), &name("TP65535.OBT")), Some((0xfffe, false)));
    }

    #[test]
    fn sideband_binding_requires_successful_rewrite_and_resets_per_session() {
        let mut first = Catalog::<u8, 2, LIMIT>::new();
        first.record_sideband_rewrite(0, false);
        assert!(!first.sideband_bound(0));
        first.record_sideband_rewrite(0, true);
        assert!(first.sideband_bound(0));
        assert!(!first.sideband_bound(1), "CRC and retention bind independently");
        first.record_sideband_rewrite(1, true);
        assert!(first.sideband_bound(1));
        assert_eq!(first.id_for_scan(None, &name("A.OBT")), Some((0xff00, false)));
        assert_eq!(first.id_for_scan(None, &name("B.OBT")), Some((0xff01, false)));

        let mut second = Catalog::<u8, 2, LIMIT>::new();
        assert_eq!(second.id_for_scan(None, &name("B.OBT")), Some((0xff00, false)));
        assert_eq!(second.id_for_scan(None, &name("A.OBT")), Some((0xff01, false)));
        assert!(!second.sideband_bound(0), "stale ff00 CRC/retention rows stay masked until rewrite");
        assert!(!second.sideband_bound(1));
    }

    #[test]
    #[should_panic]
    fn sideband_bit_must_fit_the_binding_mask() {
        let c = Catalog::<u8, 1, LIMIT>::new();
        let _ = c.sideband_bound(u8::BITS as u8);
    }

    #[test]
    fn local_count_and_content_match_pin_route_helpers() {
        let mut c = Catalog::<u32, 3, LIMIT>::new();
        c.insert(1, name("RT1.OBR"), 10).unwrap();
        c.adopt_visible(0xff00, false, name("_NAV.OBR"), 20, false).unwrap();
        assert_eq!(c.find_by_content(9, 20, |id| (id == 0xff00).then_some(9), |_, len| Some(*len)), Some(0xff00));
        assert_eq!(c.find_by_content(9, 21, |_| Some(9), |_, len| Some(*len)), None);
    }
}
