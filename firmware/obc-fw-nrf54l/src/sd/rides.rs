//! Borrowed stored-ride repository over [`Storage`].
//!
//! The companion link needs every admitted `RD{id}.ORD` row (up to [`MAX_RIDES`]); the on-device
//! menu needs only the newest [`UI_RIDES_CAP`]. Both are projections of this one media-backed
//! catalog, rather than independent scans and retained filename/id tables.

use super::*;
use obc_storage::{RideCatalog, RideRemoveAction, RideScanAction, RideScanRead};

const _: () = {
    assert!(
        core::mem::size_of::<RideCatalog<RIDE_CATALOG_CAP>>()
            == core::mem::size_of::<Vec<u16, RIDE_CATALOG_CAP>>()
                + core::mem::size_of::<Vec<ShortFileName, RIDE_CATALOG_CAP>>()
    );
};

/// One scoped ride repository. Its borrow ends before the shared-store lock can cross an `await`.
pub(crate) struct Rides<'a> {
    storage: &'a mut Storage,
    catalog: &'a mut StoredRideCatalog,
}

impl<'a> Rides<'a> {
    pub(super) fn new(storage: &'a mut Storage, catalog: &'a mut StoredRideCatalog) -> Self {
        Self { storage, catalog }
    }

    /// Rebuild the canonical companion catalog in directory order. Only the exact held-version
    /// interrupted-save signature is swept; a generic read failure stays on media.
    pub(crate) fn scan(&mut self) {
        self.catalog.clear();
        let Some(dir) = self.storage.tracks_dir else { return };
        let mut entries: Vec<(u16, ShortFileName), MAX_RIDES> = Vec::new();
        let mut over_cap = 0u16;
        self.storage.iter_dir_lfn(dir, |entry, _| {
            let Some(id) = stored_ride_id(&entry.name) else { return };
            if entries.push((id, entry.name.clone())).is_err() {
                over_cap = over_cap.saturating_add(1);
            }
        });
        if over_cap > 0 {
            defmt::warn!("store: more than {=usize} ride objects — {=u16} not listed", MAX_RIDES, over_cap);
        }

        for (id, name) in entries {
            let read = match self.read_file(&name) {
                Some(_) => RideScanRead::Valid,
                None if self.storage.is_aborted_ride_object(&name) => RideScanRead::ZeroMarker,
                None => RideScanRead::Unreadable,
            };
            match self.catalog.observe_scan(id, name.clone(), read) {
                RideScanAction::Cataloged => {}
                RideScanAction::Sweep => {
                    defmt::info!("store: sweeping interrupted ride save {}", defmt::Debug2Format(&name));
                    let _ = self.delete_file(&name);
                }
                RideScanAction::KeepUnlisted => {
                    defmt::warn!("SD: ride {} unreadable — kept for a later rescan", defmt::Debug2Format(&name));
                }
                RideScanAction::Full => over_cap = over_cap.saturating_add(1),
            }
        }
        let listed = self.catalog.len();
        let _ = self.catalog.finish_scan(over_cap);
        defmt::info!("store: {=usize} ride object(s)", listed);
    }

    /// Project the canonical rows into the newest-first on-device menu without rescanning media or
    /// mutating companion-visible catalog state.
    pub(crate) fn snapshot_into(
        &self,
        summaries: &mut Vec<obc_app::RideSummary, UI_RIDES_CAP>,
        ids: &mut Vec<u16, UI_RIDES_CAP>,
    ) {
        summaries.clear();
        ids.clear();
        let synced = self.storage.load_synced_set();
        for (id, file) in self.catalog.iter() {
            let Some((_, info)) = self.read_file(file) else {
                defmt::warn!("SD: ride {} unreadable during menu projection", defmt::Debug2Format(file));
                continue;
            };
            let summary = obc_app::RideSummary::from_info(&info, synced.contains(id), synced.synced_at(id));
            let pos = summaries
                .iter()
                .position(|candidate| summary.start_time > candidate.start_time)
                .unwrap_or(summaries.len());
            if summaries.is_full() && pos < summaries.len() {
                let _ = summaries.pop();
                let _ = ids.pop();
            }
            if pos <= summaries.len() && !summaries.is_full() {
                let _ = summaries.insert(pos, summary);
                let _ = ids.insert(pos, id);
            }
        }
        if self.catalog.len() > summaries.len() {
            defmt::info!(
                "SD: rides menu lists the newest {=usize} of {=usize} stored",
                summaries.len(),
                self.catalog.len()
            );
        }
        defmt::info!("SD: {=usize} ride(s) in /tracks", summaries.len());
    }

    pub(crate) fn len(&self) -> usize {
        self.catalog.len()
    }

    pub(crate) fn total(&self) -> u16 {
        self.catalog.total()
    }

    pub(crate) fn file(&self, id: u16) -> Option<ShortFileName> {
        self.catalog.get(id).map(|(_, file)| file.clone())
    }

    pub(crate) fn contains(&self, id: u16) -> bool {
        self.catalog.get(id).is_some()
    }

    /// Delete one cataloged ride and its synced marker. A truncated catalog is backfilled before
    /// the caller publishes its single revision edge.
    pub(crate) fn delete(&mut self, id: u16) -> bool {
        let Some(file) = self.file(id) else { return false };
        if !self.delete_file(&file) {
            return false;
        }
        self.storage.forget_ride_synced(id);
        if self.catalog.remove(id) == RideRemoveAction::Backfill {
            self.scan();
        }
        true
    }

    /// Read every companion `rideList` row, failing the whole list on a transient header error.
    pub(crate) fn for_each_list_row(&self, mut emit: impl FnMut(u16, u32, &RideInfo)) -> Option<u16> {
        let mut count = 0u16;
        for (id, file) in self.catalog.iter() {
            let (byte_len, info) = self.read_file(file)?;
            emit(id, byte_len, &info);
            count += 1;
        }
        Some(count)
    }

    /// Stream one stored ride into the detail elevation profile builder.
    pub(crate) fn profile(&self, id: u16) -> Option<Profile> {
        let file = self.file(id)?;
        self.with_file(&file, |source, _| ride_elevation_profile(source).ok()).or_else(|| {
            defmt::warn!("SD: ride profile: cannot read {} — band stays empty", defmt::Debug2Format(&file));
            None
        })
    }

    /// Stream one stored ride into the bounded detail preview polyline.
    pub(crate) fn preview(&self, id: u16) -> heapless::Vec<(i32, i32), { obc_app::NAV_PREVIEW_MAX }> {
        let Some(file) = self.file(id) else { return heapless::Vec::new() };
        self.with_file(&file, |source, _| Some(ride_preview_polyline(source).unwrap_or_default())).unwrap_or_default()
    }

    fn read_file(&self, name: &ShortFileName) -> Option<(u32, RideInfo)> {
        self.with_file(name, |source, len| Some((len, RideInfo::read(source).ok()?)))
    }

    fn with_file<T>(&self, name: &ShortFileName, f: impl FnOnce(&dyn ByteSource, u32) -> Option<T>) -> Option<T> {
        let dir = self.storage.tracks_dir?;
        let (file, len, borrowed) = match self.storage.vmgr.open_file_in_dir(dir, name, Mode::ReadOnly) {
            Ok(file) => (file, self.storage.vmgr.file_length(file).unwrap_or(0), false),
            Err(_) => match &self.storage.open_object {
                Some(open) if &open.name == name => (open.file, open.len, true),
                _ => return None,
            },
        };
        let source = SdByteSource::new(&self.storage.vmgr, file, len);
        let out = f(&source, len);
        if !borrowed {
            let _ = self.storage.vmgr.close_file(file);
        }
        out
    }

    fn delete_file(&mut self, name: &ShortFileName) -> bool {
        let Some(dir) = self.storage.tracks_dir else { return false };
        // Preserve the retained-download contract: embedded-sdmmc refuses deletion while this
        // exact file is open, so an on-device delete fails without mutating the catalog/revision.
        // Closing here would tear down the data plane behind an in-flight chunk and could let its
        // completion recreate the synced sidecar after the delete.
        match self.storage.vmgr.delete_file_in_dir(dir, name) {
            Ok(()) => true,
            Err(error) => {
                defmt::warn!("SD: delete ride {} failed: {}", defmt::Debug2Format(name), defmt::Debug2Format(&error));
                false
            }
        }
    }
}
