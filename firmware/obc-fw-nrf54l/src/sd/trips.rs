//! Borrowed trip repository over [`Storage`].
//!
//! This view is the only mutation boundary for the resident trip catalog, its media files, and
//! `TRIPS.CRC`. It deliberately borrows the board store rather than owning another table: the app
//! and companion-link planes therefore observe the same ids, filenames, ordering, and metadata.

use super::*;
use obc_storage::{TripCatalog, TripRemoveAction, TripScanAction, TripScanRead};

const _: () = {
    assert!(core::mem::size_of::<Option<TripMeta>>() == core::mem::size_of::<TripMeta>());
    assert!(core::mem::align_of::<Option<TripMeta>>() == core::mem::align_of::<TripMeta>());
    assert!(
        core::mem::size_of::<TripCatalog<Option<TripMeta>, MAX_TRIPS, SIDELOAD_ID_BASE>>()
            == core::mem::size_of::<Vec<u16, MAX_TRIPS>>()
                + core::mem::size_of::<Vec<ShortFileName, MAX_TRIPS>>()
                + core::mem::size_of::<Vec<TripMeta, MAX_TRIPS>>()
    );
};

/// The one scoped trip repository. Its borrow must end before the shared-store lock can cross an
/// `await`; all current callers consume it synchronously.
pub(crate) struct Trips<'a> {
    storage: &'a mut Storage,
}

impl<'a> Trips<'a> {
    pub(super) fn new(storage: &'a mut Storage) -> Self {
        Self { storage }
    }

    /// Rebuild the canonical catalog in directory order. Only the exact zero-marker interrupted
    /// commit is swept; a generic read failure remains on media for a later rescan.
    pub(crate) fn scan(&mut self) {
        self.storage.trip_catalog.clear();
        let Some(dir) = self.storage.routes_dir else { return };
        let mut names: Vec<ShortFileName, MAX_TRIPS> = Vec::new();
        let mut over_cap = 0u16;
        self.storage.iter_dir_lfn(dir, |entry, long| {
            if is_trip_entry(entry, long) && names.push(entry.name.clone()).is_err() {
                over_cap = over_cap.saturating_add(1);
            }
        });
        if over_cap > 0 {
            defmt::warn!("SD: more than {=usize} trip files — {=u16} not listed", MAX_TRIPS, over_cap);
        }

        for name in names {
            let uploaded = trip_name::uploaded_id(name.base_name(), name.extension());
            let Some(id) = uploaded.or_else(|| self.storage.sideload_id(&name)) else {
                defmt::warn!("SD: trip {} has no object id — not listed", defmt::Debug2Format(&name));
                continue;
            };
            let read = match self.read_file(&name) {
                Some((_, meta, _)) => TripScanRead::Valid(Some(meta)),
                None if self.storage.is_aborted_commit(&name) => TripScanRead::ZeroMarker,
                None => TripScanRead::Unreadable,
            };
            match self.storage.trip_catalog.observe_scan(id, uploaded.is_some(), name.clone(), read) {
                TripScanAction::Cataloged => {}
                TripScanAction::Sweep => {
                    defmt::info!("store: sweeping aborted trip commit {}", defmt::Debug2Format(&name));
                    let _ = self.delete_file(&name);
                }
                TripScanAction::KeepUnlisted => {
                    defmt::warn!("SD: trip {} unreadable — kept for a later rescan", defmt::Debug2Format(&name));
                }
                TripScanAction::Full => unreachable!("directory names are capped before catalog insertion"),
            }
        }
        let listed = self.storage.trip_catalog.row_count();
        let _ = self.storage.trip_catalog.finish_scan(over_cap);
        defmt::info!("store: {=usize} trip object(s)", listed);
    }

    pub(crate) fn inputs(&self) -> Vec<TripInput<'_>, MAX_TRIPS> {
        let mut out = Vec::new();
        for (id, _, meta) in self.storage.trip_catalog.iter() {
            if let Some(meta) = meta.as_ref() {
                let _ = out.push(TripInput { id: u64::from(id), name: meta.name.as_str(), stage_ids: &meta.stage_ids });
            }
        }
        out
    }

    pub(crate) fn len(&self) -> usize {
        self.storage.trip_catalog.row_count()
    }

    pub(crate) fn candidate(&self) -> Option<u16> {
        self.storage.trip_catalog.candidate()
    }

    pub(crate) fn contains(&self, id: u16) -> bool {
        self.storage.trip_catalog.get(id).is_some()
    }

    pub(crate) fn file(&self, id: u16) -> Option<ShortFileName> {
        self.storage.trip_catalog.get(id).map(|(_, file, _)| file.clone())
    }

    pub(crate) fn stage_ids(&self, id: u16) -> Option<Vec<u64, { obc_route::MAX_TRIP_STAGES }>> {
        self.read(id).map(|(_, meta, _)| meta.stage_ids)
    }

    pub(crate) fn read(&self, id: u16) -> Option<(u32, TripMeta, u16)> {
        let file = self.file(id)?;
        self.read_file(&file)
    }

    /// Delete one cataloged trip and its sidecar fingerprint, shifting the three resident columns
    /// together only after the FAT delete succeeds.
    pub(crate) fn delete(&mut self, id: u16) -> bool {
        let Some(file) = self.file(id) else { return false };
        if !self.delete_file(&file) {
            return false;
        }
        self.forget_crc(id);
        if self.storage.trip_catalog.remove(id) == TripRemoveAction::Backfill {
            self.scan();
        }
        true
    }

    fn read_file(&self, name: &ShortFileName) -> Option<(u32, TripMeta, u16)> {
        self.storage.with_routes_object(name, |src, len| {
            let meta = TripMeta::read(src).ok()?;
            let summary = TripSummary::read(src).ok()?;
            Some((len, meta, summary.stage_count))
        })
    }

    fn delete_file(&mut self, name: &ShortFileName) -> bool {
        let Some(dir) = self.storage.routes_dir else { return false };
        self.close_object_if(name);
        match self.storage.vmgr.delete_file_in_dir(dir, name) {
            Ok(()) => true,
            Err(error) => {
                defmt::warn!("SD: delete trip {} failed: {}", defmt::Debug2Format(name), defmt::Debug2Format(&error));
                false
            }
        }
    }

    fn close_object_if(&mut self, name: &ShortFileName) {
        if matches!(&self.storage.open_object, Some((ref open_name, ..)) if open_name == name) {
            self.storage.close_object();
        }
    }

    fn load_crcs(&self) -> RouteCrcs {
        self.storage.load_crc_sidecar(TRIP_CRCS)
    }

    fn forget_crc(&mut self, id: u16) {
        let mut map = self.load_crcs();
        if map.remove(id) {
            self.write_crcs(&map);
        }
    }

    fn write_crcs(&mut self, map: &RouteCrcs) {
        if !self.storage.write_crc_sidecar(TRIP_CRCS, map) {
            defmt::warn!("SD: trip-crc sidecar not persisted — a trip may serve crc 0 next list build");
        }
    }
}
