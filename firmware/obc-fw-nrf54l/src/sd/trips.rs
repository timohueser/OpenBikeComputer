//! Borrowed trip repository over [`Storage`].
//!
//! This view is the only mutation boundary for the resident trip catalog, its media files, and
//! `TRIPS.CRC`. It deliberately borrows the board store rather than owning another table: the app
//! and companion-link planes therefore observe the same ids, filenames, ordering, and metadata.

use super::*;
use obc_storage::{Catalog, RemoveAction, ScanAction, ScanRead};

const CRC_BINDING: u8 = 0;

const _: () = {
    assert!(core::mem::size_of::<Option<TripMeta>>() == core::mem::size_of::<TripMeta>());
    assert!(core::mem::align_of::<Option<TripMeta>>() == core::mem::align_of::<TripMeta>());
    assert!(
        core::mem::size_of::<Catalog<Option<TripMeta>, MAX_TRIPS, SIDELOAD_ID_BASE>>()
            == core::mem::size_of::<Vec<u16, MAX_TRIPS>>()
                + core::mem::size_of::<Vec<ShortFileName, MAX_TRIPS>>()
                + core::mem::size_of::<Vec<TripMeta, MAX_TRIPS>>()
                + core::mem::size_of::<Vec<ShortFileName, MAX_TRIPS>>()
                + core::mem::size_of::<u32>()
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
        self.rebind_sideload_crcs();
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
            let parsed = trip_name::uploaded_id(name.base_name(), name.extension());
            let Some((id, uploaded)) = self.storage.trip_catalog.id_for_scan(parsed, &name) else {
                defmt::warn!("SD: trip {} has no object id — not listed", defmt::Debug2Format(&name));
                continue;
            };
            let read = match self.read_file(&name) {
                Some((_, meta, _)) => ScanRead::Valid(Some(meta)),
                None if self.storage.is_aborted_commit(&name) => ScanRead::ZeroMarker,
                None => ScanRead::Unreadable,
            };
            match self.storage.trip_catalog.observe_scan(id, uploaded, name.clone(), read) {
                ScanAction::Cataloged => {}
                ScanAction::Sweep => {
                    defmt::info!("store: sweeping aborted trip commit {}", defmt::Debug2Format(&name));
                    let _ = self.delete_file(&name);
                }
                ScanAction::KeepUnlisted => {
                    defmt::warn!("SD: trip {} unreadable — kept for a later rescan", defmt::Debug2Format(&name));
                }
                ScanAction::Full => unreachable!("directory names are capped before catalog insertion"),
            }
        }
        let listed = self.storage.trip_catalog.len();
        self.storage.trip_catalog.finish_scan(over_cap);
        defmt::info!("store: {=usize} trip object(s)", listed);
    }

    pub(crate) fn inputs(&self) -> Vec<TripInput<'_>, MAX_TRIPS> {
        let mut out = Vec::new();
        for (id, _, meta) in self.storage.trip_catalog.iter() {
            if let Some(meta) = meta.as_ref() {
                let _ = out.push(TripInput { id, name: meta.name.as_str(), stage_ids: &meta.stage_ids });
            }
        }
        out
    }

    pub(crate) fn len(&self) -> usize {
        self.storage.trip_catalog.len()
    }

    pub(crate) fn total(&self) -> u16 {
        self.storage.trip_catalog.total()
    }

    pub(crate) fn observe_floor(&mut self, floor: u16) {
        self.storage.trip_catalog.observe_floor(floor);
    }

    pub(crate) fn candidate(&self) -> Option<u16> {
        self.storage.trip_catalog.candidate()
    }

    pub(crate) fn commit(&mut self, persist_floor: impl FnOnce(u16)) -> u16 {
        self.storage.trip_catalog.commit(persist_floor)
    }

    pub(crate) fn is_full(&self) -> bool {
        self.storage.trip_catalog.is_full()
    }

    pub(crate) fn contains(&self, id: u16) -> bool {
        self.storage.trip_catalog.get(id).is_some()
    }

    pub(crate) fn file(&self, id: u16) -> Option<ShortFileName> {
        self.storage.trip_catalog.get(id).map(|(_, file, _)| file.clone())
    }

    pub(crate) fn stage_ids(&self, id: u16) -> Option<Vec<u16, { obc_route::MAX_TRIP_STAGES }>> {
        self.read(id).map(|(_, meta, _)| meta.stage_ids)
    }

    pub(crate) fn read(&self, id: u16) -> Option<(u32, TripMeta, u16)> {
        let file = self.file(id)?;
        self.read_file(&file)
    }

    /// Match a fresh upload against the CRC sidecar, confirming byte length from media before a
    /// dedup hit. Missing CRCs deliberately do not match.
    pub(crate) fn find_by_content(&self, crc: u32, byte_len: u32) -> Option<u16> {
        let crcs = self.load_crcs();
        self.storage.trip_catalog.find_by_content(crc, byte_len, |id| crcs.get(id), |file, _| self.file_len(file))
    }

    /// Delete one cataloged trip and its sidecar fingerprint, shifting the three resident columns
    /// together only after the FAT delete succeeds.
    pub(crate) fn delete(&mut self, id: u16) -> bool {
        let Some(file) = self.file(id) else { return false };
        if !self.delete_file(&file) {
            return false;
        }
        self.forget_crc(id);
        if self.storage.trip_catalog.remove(id) == RemoveAction::Backfill {
            self.scan();
        }
        true
    }

    /// Promote `UPLOAD.TMP` while retaining the existing same-id replacement mechanics. This does
    /// not publish the catalog row: ObjectIdSequence persists a fresh id first, then
    /// [`record_commit`](Self::record_commit) makes the row and CRC visible together.
    pub(crate) fn promote_temp(
        &mut self,
        session: UploadSession,
        replace: Option<&ShortFileName>,
        fresh_id: u16,
    ) -> Option<(ShortFileName, Option<TripMeta>)> {
        if !self.storage.upload_take(session, UploadDestination::Trip) {
            return None;
        }
        let dir = self.storage.routes_dir?;
        let src_file = self.storage.vmgr.open_file_in_dir(dir, UPLOAD_TMP, Mode::ReadOnly).ok()?;
        let len = self.storage.vmgr.file_length(src_file).unwrap_or(0);
        let source = SdByteSource::new(&self.storage.vmgr, src_file, len);
        if TripSummary::read(&source).is_err() {
            let _ = self.storage.vmgr.close_file(src_file);
            let _ = self.storage.vmgr.delete_file_in_dir(dir, UPLOAD_TMP);
            defmt::warn!("SD: trip upload is not a valid trip object — rejected");
            return None;
        }
        // Metadata is an app-menu cache, not part of upload acceptance. Preserve the old
        // TripSummary acceptance rule and leave a readable-but-uncached row for a later rescan if
        // this optional second read encounters a transient failure.
        let meta = TripMeta::read(&source).ok();
        let final_name = match replace {
            Some(name) => {
                self.close_object_if(name);
                if let Err(error) = self.storage.vmgr.delete_file_in_dir(dir, name) {
                    defmt::warn!(
                        "SD: trip replace: cannot delete old {}: {}",
                        defmt::Debug2Format(name),
                        defmt::Debug2Format(&error)
                    );
                }
                name.clone()
            }
            None => match self.storage.fresh_object_name(dir, "TP", fresh_id, "OBT") {
                Some(name) => name,
                None => {
                    let _ = self.storage.vmgr.close_file(src_file);
                    defmt::warn!("SD: trip upload name TP{=u16}.OBT unavailable", fresh_id);
                    return None;
                }
            },
        };
        let copied = match self.storage.vmgr.open_file_in_dir(dir, &final_name, Mode::ReadWriteCreateOrTruncate) {
            Ok(dst_file) => {
                let ok = self.storage.copy_with_held_magic(src_file, dst_file, len);
                if !ok {
                    defmt::warn!("SD: trip upload copy failed — commit aborted (a replaced trip's old file is gone)");
                }
                let _ = self.storage.vmgr.close_file(dst_file);
                ok
            }
            Err(error) => {
                defmt::warn!("SD: cannot create {}: {}", defmt::Debug2Format(&final_name), defmt::Debug2Format(&error));
                false
            }
        };
        let _ = self.storage.vmgr.close_file(src_file);
        let _ = self.storage.vmgr.delete_file_in_dir(dir, UPLOAD_TMP);
        if !copied {
            return None;
        }
        defmt::info!("SD: trip committed → routes/{} ({=u32} B)", defmt::Debug2Format(&final_name), len);
        Some((final_name, meta))
    }

    /// Publish a promoted file into the canonical catalog and CRC sidecar. Returns `false` only if
    /// the expected replacement row disappeared or a fresh row cannot fit.
    pub(crate) fn record_commit(
        &mut self,
        id: u16,
        replaced: bool,
        commit: (ShortFileName, Option<TripMeta>),
        crc: u32,
    ) -> bool {
        let (file, meta) = commit;
        let changed = if replaced {
            self.storage.trip_catalog.replace(id, file, meta).is_ok()
        } else {
            self.storage.trip_catalog.insert(id, file, meta).is_ok()
        };
        if changed {
            self.set_crc(id, crc);
        }
        changed
    }

    /// After a failed destructive replacement, remove the row only when the old file is genuinely
    /// gone. A transient read failure is indistinguishable at this legacy boundary and therefore
    /// retains the pre-existing behavior.
    pub(crate) fn repair_failed_replace(&mut self, id: u16) -> bool {
        let Some(file) = self.file(id) else { return false };
        if self.read_file(&file).is_some() {
            return false;
        }
        if self.storage.trip_catalog.remove(id) == RemoveAction::Backfill {
            self.scan();
        }
        true
    }

    /// Read and emit every `tripList` row, lazily filling unknown CRCs and persisting the sidecar at
    /// most once. Route lookup stays bounded by the caller's route catalog.
    pub(crate) fn for_each_list_row(
        &mut self,
        routes: &StoredRouteCatalog,
        mut route_file: impl FnMut(u16) -> Option<ShortFileName>,
        mut emit: impl FnMut(u16, u32, u32, u32, u16, &TripMeta, u32),
    ) -> Option<u16> {
        let mut crcs = self.load_crcs();
        let mut crcs_dirty = false;
        let mut count = 0u16;
        let len = self.storage.trip_catalog.len();
        for index in 0..len {
            let (id, file) = {
                let (id, file, _) = self.storage.trip_catalog.iter().nth(index)?;
                (id, file.clone())
            };
            let (byte_len, meta, stage_count) = self.read_file(&file)?;
            let mut total_distance_m = 0u32;
            let mut total_ascent_m = 0u32;
            for stage_id in &meta.stage_ids {
                if let Some(route) = route_file(*stage_id) {
                    if let Some((_, info)) = self.storage.route_object_info(routes, &route) {
                        total_distance_m = total_distance_m.saturating_add(info.distance_m);
                        total_ascent_m = total_ascent_m.saturating_add(info.ascent_m);
                    }
                }
            }
            let stored_crc = crcs.get(id);
            let computed_crc = stored_crc.is_none().then(|| self.storage.file_crc(&file)).flatten();
            let (crc, fresh_crc) = trip_crc(stored_crc, computed_crc);
            if let Some(fresh) = fresh_crc {
                if crcs.insert(id, fresh) {
                    crcs_dirty = true;
                }
            }
            emit(id, byte_len, total_distance_m, total_ascent_m, stage_count, &meta, crc);
            count += 1;
        }
        if crcs_dirty {
            self.write_crcs(&crcs);
        }
        Some(count)
    }

    fn read_file(&self, name: &ShortFileName) -> Option<(u32, TripMeta, u16)> {
        self.storage.with_routes_object(name, |src, len| {
            let meta = TripMeta::read(src).ok()?;
            let summary = TripSummary::read(src).ok()?;
            Some((len, meta, summary.stage_count))
        })
    }

    fn file_len(&self, name: &ShortFileName) -> Option<u32> {
        self.storage.with_routes_object(name, |_, len| Some(len))
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
        if let Some(owner) = self.storage.open_object.as_ref().filter(|open| &open.name == name).map(|open| open.owner)
        {
            self.storage.close_object_owner(owner);
        }
    }

    fn load_crcs(&self) -> RouteCrcs {
        let mut map = self.storage.load_crc_sidecar(TRIP_CRCS);
        if !self.storage.trip_catalog.sideband_bound(CRC_BINDING) {
            clear_session_crcs(&mut map);
        }
        map
    }

    fn rebind_sideload_crcs(&mut self) {
        if self.storage.trip_catalog.sideband_bound(CRC_BINDING) {
            return;
        }
        let (mut map, authoritative) = self.storage.load_crc_sidecar_status(TRIP_CRCS);
        clear_session_crcs(&mut map);
        if authoritative {
            let persisted = self.storage.write_crc_sidecar(TRIP_CRCS, &map);
            self.storage.trip_catalog.record_sideband_rewrite(CRC_BINDING, persisted);
        }
    }

    fn set_crc(&mut self, id: u16, crc: u32) {
        let mut map = self.load_crcs();
        if map.insert(id, crc) {
            self.write_crcs(&map);
        }
    }

    fn forget_crc(&mut self, id: u16) {
        let mut map = self.load_crcs();
        if map.remove(id) {
            self.write_crcs(&map);
        }
    }

    fn write_crcs(&mut self, map: &RouteCrcs) {
        let persisted = self.storage.write_crc_sidecar(TRIP_CRCS, map);
        self.storage.trip_catalog.record_sideband_rewrite(CRC_BINDING, persisted);
        if !persisted {
            defmt::warn!("SD: trip-crc sidecar not persisted — a trip may serve crc 0 next list build");
        }
    }
}

const fn trip_crc(stored: Option<u32>, computed: Option<u32>) -> (u32, Option<u32>) {
    match stored {
        Some(crc) => (crc, None),
        None => match computed {
            Some(crc) => (crc, Some(crc)),
            None => (0, None),
        },
    }
}
