//! Borrowed route repository over [`Storage`].
//!
//! One catalog supplies the on-device route projection, retained geometry lookup, and companion
//! object plane. Media I/O stays board-owned; aligned rows, full count, and fresh-id sequencing are
//! owned by `obc-storage`'s host-tested [`Catalog`].

use super::*;
use obc_storage::{Catalog, RemoveAction, ScanAction, ScanRead};

const CRC_BINDING: u8 = 0;
const RETENTION_BINDING: u8 = 1;

const _: () = assert!(
    core::mem::size_of::<Catalog<u32, MAX_ROUTES, SIDELOAD_ID_BASE>>()
        == core::mem::size_of::<Vec<u16, MAX_ROUTES>>()
            + core::mem::size_of::<Vec<ShortFileName, MAX_ROUTES>>()
            + core::mem::size_of::<Vec<u32, MAX_ROUTES>>()
            + core::mem::size_of::<Vec<ShortFileName, MAX_ROUTES>>()
            + core::mem::size_of::<u32>()
);

/// One scoped route repository. Its borrow ends before the shared-store lock crosses an `await`.
pub(crate) struct Routes<'a> {
    storage: &'a mut Storage,
    catalog: &'a mut StoredRouteCatalog,
}

/// Result of finishing a validated local-navigation stage.
pub(crate) enum NavCommit {
    /// The new final route is valid and has been published under this id.
    Published(u16),
    /// Planning or promotion failed. `revision` is true only after the final name was touched and
    /// the canonical catalog was rescanned to publish the resulting media truth.
    Failed { revision: bool },
}

enum StagePromote {
    Committed(u32),
    Refused,
    Failed,
}

impl<'a> Routes<'a> {
    pub(super) fn new(storage: &'a mut Storage, catalog: &'a mut StoredRouteCatalog) -> Self {
        Self { storage, catalog }
    }

    pub(crate) fn state(&self) -> (usize, u16, Option<u16>) {
        (self.catalog.len(), self.catalog.total(), self.catalog.candidate())
    }

    pub(crate) fn admission(&self, id: u16) -> (bool, bool, Option<ShortFileName>, Option<u16>) {
        let file = self.catalog.get(id).map(|(_, file, _)| file.clone());
        (file.is_some(), self.catalog.is_full(), file, self.catalog.candidate())
    }
    pub(crate) fn observe_floor(&mut self, floor: u16) {
        self.catalog.observe_floor(floor);
    }

    /// Rebuild the canonical catalog in FAT directory order. Only a zero-marker interrupted commit
    /// is swept; a generic read failure remains on media for a later scan.
    pub(crate) fn scan(&mut self) {
        self.rebind_sideload_sidecars();
        let open_geometry = self
            .storage
            .open_route
            .and_then(|(id, file, len)| self.catalog.get(id).map(|(_, name, _)| (id, name.clone(), file, len)));
        self.catalog.clear();
        let Some(dir) = self.storage.routes_dir else { return };
        let mut names: Vec<ShortFileName, MAX_ROUTES> = Vec::new();
        let mut over_cap = 0u16;
        self.storage.iter_dir_lfn(dir, |entry, long| {
            if is_route_entry(entry, long) && names.push(entry.name.clone()).is_err() {
                over_cap = over_cap.saturating_add(1);
            }
        });
        if over_cap > 0 {
            defmt::warn!("SD: more than {=usize} route files — {=u16} not listed", MAX_ROUTES, over_cap);
        }

        for name in names {
            let parsed = route_name::uploaded_id(name.base_name(), name.extension());
            let Some((id, uploaded)) = self.catalog.id_for_scan(parsed, &name) else {
                defmt::warn!("SD: route {} has no object id — not listed", defmt::Debug2Format(&name));
                continue;
            };
            let read = match self.read_info_with_geometry(&name, open_geometry.as_ref()) {
                Some((byte_len, _)) => ScanRead::Valid(byte_len),
                None if self.is_zero_marker_with_geometry(&name, open_geometry.as_ref()) => ScanRead::ZeroMarker,
                None => ScanRead::Unreadable,
            };
            match self.catalog.observe_scan(id, uploaded, name.clone(), read) {
                ScanAction::Cataloged => {}
                ScanAction::Sweep => {
                    defmt::info!("store: sweeping aborted route commit {}", defmt::Debug2Format(&name));
                    let _ = self.delete_file(&name);
                }
                ScanAction::KeepUnlisted => {
                    defmt::warn!("SD: route {} unreadable — kept for a later scan", defmt::Debug2Format(&name));
                }
                ScanAction::Full => unreachable!("directory names are capped before catalog insertion"),
            }
        }
        self.catalog.finish_scan(over_cap);

        // A scan can reorder the catalog. Keep a retained geometry handle paired with its filename,
        // or close it if the file is no longer a canonical row.
        if let Some((id, name, file, len)) = open_geometry {
            self.storage.open_route =
                self.catalog.get(id).filter(|(_, candidate, _)| *candidate == &name).map(|_| (id, file, len));
            if self.storage.open_route.is_none() {
                let _ = self.storage.vmgr.close_file(file);
            }
        }
        defmt::info!(
            "store: {=usize} route object(s), next id {=u16}",
            self.catalog.len(),
            self.catalog.candidate().unwrap_or(SIDELOAD_ID_BASE)
        );
    }

    /// Project readable canonical rows into aligned on-device menu columns without rescanning.
    /// A transiently unreadable existing row reuses its previous summary by durable id, so an
    /// unrelated local commit cannot unload UI/trip state. A new unreadable row has no safe prior
    /// value and remains omitted until a later projection. Retention always comes from the freshly
    /// loaded sidecar, including for a fallback summary.
    pub(crate) fn snapshot_into(
        &self,
        previous_summaries: &[RouteSummary],
        previous_ids: &[u16],
        summaries: &mut Vec<RouteSummary, MAX_ROUTES>,
        ids: &mut Vec<u16, MAX_ROUTES>,
        metas: &mut Vec<RouteRetentionMeta, MAX_ROUTES>,
    ) {
        summaries.clear();
        ids.clear();
        metas.clear();
        let retention = self.load_retention();
        for (id, name, _) in self.catalog.iter() {
            let summary = self.with_file(name, |source, _| RouteSummary::read(source).ok()).or_else(|| {
                let i = previous_ids.iter().position(|candidate| *candidate == id)?;
                previous_summaries.get(i).cloned()
            });
            if let Some(summary) = summary {
                let _ = summaries.push(summary);
                let _ = ids.push(id);
                let _ = metas.push(retention.get(id));
            }
        }
    }

    /// Reconcile retained geometry to a durable route id. The filename is resolved while this view
    /// holds both Storage and catalog, so projection gaps and reorder cannot retarget the handle.
    pub(crate) fn reconcile(&mut self, want: Option<u16>) {
        if self.storage.open_route.map(|(id, _, _)| id) == want {
            return;
        }
        self.storage.close_route();
        let Some(id) = want else { return };
        let Some((_, name, _)) = self.catalog.get(id) else { return };
        let Some(dir) = self.storage.routes_dir else { return };
        if let Ok(file) = self.storage.vmgr.open_file_in_dir(dir, name, Mode::ReadOnly) {
            let len = self.storage.vmgr.file_length(file).unwrap_or(0);
            self.storage.open_route = Some((id, file, len));
        }
    }

    /// Match a fresh upload against a known CRC and the canonical byte length.
    pub(crate) fn find_by_content(&self, crc: u32, byte_len: u32) -> Option<u16> {
        let crcs = self.load_crcs();
        self.catalog.find_by_content(crc, byte_len, |id| crcs.get(id), |_, stored_len| Some(*stored_len))
    }

    /// Delete a cataloged route and both sidecar rows. An over-cap catalog is rescanned before the
    /// caller publishes its single revision edge so a hidden file is admitted coherently.
    pub(crate) fn delete(&mut self, id: u16) -> bool {
        let Some((_, file, _)) = self.catalog.get(id) else { return false };
        let file = file.clone();
        let active = self.storage.open_route.and_then(|(active_id, _, _)| (active_id == id).then_some(active_id));
        self.close_geometry_if(&file);
        if !self.delete_file(&file) {
            if let Some(active_id) = active {
                self.reconcile(Some(active_id));
            }
            return false;
        }
        self.forget_crc(id);
        self.forget_retention(id);
        if self.catalog.remove(id) == RemoveAction::Backfill {
            self.scan();
        }
        true
    }

    /// Set one known route's retention level, preserving `last_used`.
    pub(crate) fn set_retention(&mut self, id: u16, retention: Retention) -> Option<Result<bool, SidecarWriteError>> {
        self.catalog.get(id)?;
        let mut store = self.load_retention();
        let meta = RouteRetentionMeta { retention, last_used_utc: store.get(id).last_used_utc };
        if !store.set(id, meta) {
            return Some(Ok(false));
        }
        Some(self.storage.write_route_retention(&store).map(|()| {
            self.catalog.record_sideband_rewrite(RETENTION_BINDING, true);
            true
        }))
    }

    pub(crate) fn stamp_last_used(&mut self, id: u16, utc: u32) {
        if self.catalog.get(id).is_none() {
            return;
        }
        let mut store = self.load_retention();
        if store.stamp_last_used(id, utc) && self.storage.write_route_retention(&store).is_ok() {
            self.catalog.record_sideband_rewrite(RETENTION_BINDING, true);
        }
    }

    /// Promote the validated upload temp using the existing copy/held-magic replacement behavior.
    pub(crate) fn promote_temp(
        &mut self,
        replace: Option<&ShortFileName>,
        fresh_id: u16,
    ) -> Option<(ShortFileName, u32)> {
        self.storage.upload_close();
        let dir = self.storage.routes_dir?;
        let len = self.validate_stage(UPLOAD_TMP)?;
        let final_name = match replace {
            Some(name) => name.clone(),
            None => self.storage.fresh_upload_name(dir, fresh_id)?,
        };
        if !matches!(
            self.promote_stage(UPLOAD_TMP, &final_name, len, replace.is_some(), false, false),
            StagePromote::Committed(_)
        ) {
            return None;
        }
        defmt::info!("SD: route committed → routes/{} ({=u32} B)", defmt::Debug2Format(&final_name), len);
        Some((final_name, len))
    }

    /// Publish a promoted file and its CRC. Fresh commits advance/persist the candidate exactly once;
    /// replacements retain their id and ordering slot.
    pub(crate) fn record_commit(
        &mut self,
        id: Option<u16>,
        file: ShortFileName,
        byte_len: u32,
        crc: u32,
        persist_floor: impl FnOnce(u16),
    ) -> Option<u16> {
        let id = match id {
            Some(id) => {
                self.catalog.replace(id, file, byte_len).ok()?;
                id
            }
            None => self.catalog.insert_committed(file, byte_len, persist_floor).ok()?,
        };
        self.set_crc(id, crc);
        Some(id)
    }

    /// Remove a row after the legacy destructive replacement window only when its file no longer
    /// reads, preserving the pre-repository failure direction.
    pub(crate) fn repair_failed_replace(&mut self, id: u16) -> bool {
        let Some((_, file, _)) = self.catalog.get(id) else { return false };
        let file = file.clone();
        if self.read_info(&file).is_some() {
            return false;
        }
        if self.catalog.remove(id) == RemoveAction::Backfill {
            self.scan();
        }
        true
    }

    /// Read and emit every companion `routeList` row, lazily filling unknown CRCs once.
    pub(crate) fn for_each_list_row(
        &mut self,
        mut emit: impl FnMut(u16, u32, &RouteObjectInfo, u32, RouteRetentionMeta),
    ) -> Option<u16> {
        let mut crcs = self.load_crcs();
        let retention = self.load_retention();
        let mut crcs_dirty = false;
        let len = self.catalog.len();
        for i in 0..len {
            let (id, file, _) = self.catalog.iter().nth(i)?;
            let file = file.clone();
            let (byte_len, info) = self.read_info(&file)?;
            let crc = match crcs.get(id) {
                Some(crc) => crc,
                None => match self.storage.file_crc(&file) {
                    Some(crc) => {
                        if crcs.insert(id, crc) {
                            crcs_dirty = true;
                        }
                        crc
                    }
                    None => 0,
                },
            };
            emit(id, byte_len, &info, crc, retention.get(id));
        }
        if crcs_dirty && self.storage.write_route_crcs(&crcs) {
            self.catalog.record_sideband_rewrite(CRC_BINDING, true);
        }
        Some(len as u16)
    }

    pub(crate) fn elevation_sparkline(&self, id: u16) -> Option<[u8; obc_route::SPARKLINE_BUCKETS]> {
        let file = self.catalog.get(id)?.1.clone();
        self.with_file(&file, |source, _| obc_route::elevation_sparkline(source))
    }

    /// Start a local navigation write in an invisible stage. The committed `_NAV.OBR`, its catalog
    /// row, active geometry, and retained detail/download handle remain untouched while the planner
    /// is running. Opening with truncate also reclaims an orphan left by an interrupted search.
    pub(crate) fn nav_begin(&mut self) -> Option<RawFile> {
        let dir = self.storage.routes_dir_or_create()?;
        self.storage.vmgr.open_file_in_dir(dir, NAV_TMP, Mode::ReadWriteCreateOrTruncate).ok()
    }

    /// Abandon a local-navigation stage without changing the previously committed route or any
    /// retained final-file handle.
    pub(crate) fn nav_abort(&mut self, file: RawFile) {
        let _ = self.storage.vmgr.flush_file(file);
        let _ = self.storage.vmgr.close_file(file);
        if let Some(dir) = self.storage.routes_dir {
            let _ = self.storage.vmgr.delete_file_in_dir(dir, NAV_TMP);
        }
    }

    /// Validate and promote `NAV.TMP` to `_NAV.OBR` with the existing held-magic copy. No final-file
    /// handle or catalog row is touched until the stage validates and admission is known to fit.
    /// A destructive copy failure performs the exceptional full rescan so the published catalog and
    /// revision describe what is actually readable; the ordinary success path never revalidates an
    /// unrelated route.
    pub(crate) fn nav_commit(&mut self, file: RawFile) -> NavCommit {
        let flushed = self.storage.vmgr.flush_file(file).is_ok();
        let _ = self.storage.vmgr.close_file(file);
        let Some(dir) = self.storage.routes_dir else { return NavCommit::Failed { revision: false } };
        if !flushed {
            let _ = self.storage.vmgr.delete_file_in_dir(dir, NAV_TMP);
            return NavCommit::Failed { revision: false };
        }
        let Some(len) = self.validate_stage(NAV_TMP) else { return NavCommit::Failed { revision: false } };
        let Ok(nav) = ShortFileName::create_from_str(NAV_ROUTE_FILE) else {
            return NavCommit::Failed { revision: false };
        };
        let already_counted = self.nav_was_counted(&nav);
        let parsed = route_name::uploaded_id(nav.base_name(), nav.extension());
        let Some((id, uploaded)) = self.catalog.id_for_scan(parsed, &nav) else {
            let _ = self.storage.vmgr.delete_file_in_dir(dir, NAV_TMP);
            return NavCommit::Failed { revision: false };
        };
        if self.catalog.get(id).is_none() && self.catalog.is_full() {
            let _ = self.storage.vmgr.delete_file_in_dir(dir, NAV_TMP);
            defmt::warn!("SD: local route catalog full — old _NAV kept");
            return NavCommit::Failed { revision: false };
        }
        let final_exists = match self.storage.vmgr.find_directory_entry(dir, &nav) {
            Ok(_) => true,
            Err(embedded_sdmmc::Error::NotFound) => false,
            Err(error) => {
                let _ = self.storage.vmgr.delete_file_in_dir(dir, NAV_TMP);
                defmt::warn!("SD: cannot inspect old _NAV: {}", defmt::Debug2Format(&error));
                return NavCommit::Failed { revision: false };
            }
        };
        match self.promote_stage(NAV_TMP, &nav, len, final_exists, true, true) {
            StagePromote::Committed(byte_len) => {
                if self.read_info(&nav).is_some()
                    && self.catalog.adopt_visible(id, uploaded, nav, byte_len, already_counted).is_ok()
                {
                    self.forget_crc(id);
                    return NavCommit::Published(id);
                }
            }
            StagePromote::Refused => return NavCommit::Failed { revision: false },
            StagePromote::Failed => {}
        }
        self.forget_crc(id);
        self.scan();
        defmt::warn!("SD: local route promotion failed — canonical catalog reconciled");
        NavCommit::Failed { revision: true }
    }

    fn validate_stage(&mut self, stage: &str) -> Option<u32> {
        let dir = self.storage.routes_dir?;
        let file = self.storage.vmgr.open_file_in_dir(dir, stage, Mode::ReadOnly).ok()?;
        let len = self.storage.vmgr.file_length(file).unwrap_or(0);
        let valid = RouteObjectInfo::read(&SdByteSource::new(&self.storage.vmgr, file, len)).is_ok();
        let _ = self.storage.vmgr.close_file(file);
        if !valid {
            let _ = self.storage.vmgr.delete_file_in_dir(dir, stage);
            defmt::warn!("SD: staged route is not a valid OBCR — rejected");
        }
        valid.then_some(len)
    }

    fn promote_stage(
        &mut self,
        stage: &str,
        final_name: &ShortFileName,
        len: u32,
        replace: bool,
        close_retained: bool,
        delete_must_succeed: bool,
    ) -> StagePromote {
        let Some(dir) = self.storage.routes_dir else { return StagePromote::Refused };
        let Ok(src) = self.storage.vmgr.open_file_in_dir(dir, stage, Mode::ReadOnly) else {
            return StagePromote::Refused;
        };
        let mut restore_active = None;
        if replace {
            let active = self.storage.open_route.and_then(|(id, _, _)| {
                self.catalog.get(id).is_some_and(|(_, name, _)| name == final_name).then_some(id)
            });
            let retained =
                close_retained && matches!(&self.storage.open_object, Some((name, ..)) if name == final_name);
            self.close_geometry_if(final_name);
            if retained {
                self.storage.close_object();
            }
            if let Err(error) = self.storage.vmgr.delete_file_in_dir(dir, final_name) {
                defmt::warn!("SD: route replace delete failed: {}", defmt::Debug2Format(&error));
                if delete_must_succeed {
                    if let Some(id) = active {
                        self.reconcile(Some(id));
                    }
                    if retained {
                        let _ = self.storage.open_object(final_name);
                    }
                    let _ = self.storage.vmgr.close_file(src);
                    let _ = self.storage.vmgr.delete_file_in_dir(dir, stage);
                    return StagePromote::Refused;
                }
                restore_active = active;
            }
        }
        let result = match self.storage.vmgr.open_file_in_dir(dir, final_name, Mode::ReadWriteCreateOrTruncate) {
            Ok(dst) => {
                let copied = self.storage.copy_with_held_magic(src, dst, len);
                let _ = self.storage.vmgr.close_file(dst);
                if copied {
                    StagePromote::Committed(len)
                } else {
                    StagePromote::Failed
                }
            }
            Err(_) if replace => StagePromote::Failed,
            Err(_) => StagePromote::Refused,
        };
        let _ = self.storage.vmgr.close_file(src);
        let _ = self.storage.vmgr.delete_file_in_dir(dir, stage);
        if !matches!(result, StagePromote::Committed(_)) {
            if let Some(id) = restore_active {
                self.reconcile(Some(id));
            }
        }
        result
    }

    /// Whether the old `_NAV.OBR` is already represented in `total`: either it has a resident valid
    /// row, or its raw filename lies beyond the first `MAX_ROUTES` admitted directory names. An
    /// unreadable old file among the admitted names is deliberately not counted.
    fn nav_was_counted(&self, nav: &ShortFileName) -> bool {
        let Some(dir) = self.storage.routes_dir else { return self.local_file_was_counted(nav, None) };
        let mut ordinal = 0usize;
        let mut nav_ordinal = None;
        self.storage.iter_dir_lfn(dir, |entry, long| {
            if is_route_entry(entry, long) {
                if entry.name == *nav {
                    nav_ordinal = Some(ordinal);
                }
                ordinal += 1;
            }
        });
        self.local_file_was_counted(nav, nav_ordinal)
    }

    fn local_file_was_counted(&self, file: &ShortFileName, raw_ordinal: Option<usize>) -> bool {
        self.catalog.iter().any(|(_, candidate, _)| candidate == file)
            || raw_ordinal.is_some_and(|ordinal| ordinal >= MAX_ROUTES)
    }

    fn read_info(&self, name: &ShortFileName) -> Option<(u32, RouteObjectInfo)> {
        self.with_file(name, |source, len| Some((len, RouteObjectInfo::read(source).ok()?)))
    }

    fn read_info_with_geometry(
        &self,
        name: &ShortFileName,
        geometry: Option<&(u16, ShortFileName, RawFile, u32)>,
    ) -> Option<(u32, RouteObjectInfo)> {
        if let Some((_, open_name, file, len)) = geometry {
            if name == open_name {
                let source = SdByteSource::new(&self.storage.vmgr, *file, *len);
                return Some((*len, RouteObjectInfo::read(&source).ok()?));
            }
        }
        self.read_info(name)
    }

    fn is_zero_marker_with_geometry(
        &self,
        name: &ShortFileName,
        geometry: Option<&(u16, ShortFileName, RawFile, u32)>,
    ) -> bool {
        let read = |source: &dyn ByteSource| {
            let mut magic = [0u8; 4];
            ByteSource::read_at(source, 0, &mut magic).ok()?;
            Some(magic == [0; 4])
        };
        if let Some((_, open_name, file, len)) = geometry {
            if name == open_name {
                return read(&SdByteSource::new(&self.storage.vmgr, *file, *len)).unwrap_or(false);
            }
        }
        self.with_file(name, |source, _| read(source)).unwrap_or(false)
    }

    fn with_file<T>(&self, name: &ShortFileName, read: impl FnOnce(&dyn ByteSource, u32) -> Option<T>) -> Option<T> {
        if let Some((id, file, len)) = self.storage.open_route {
            if self.catalog.get(id).is_some_and(|(_, open_name, _)| open_name == name) {
                return read(&SdByteSource::new(&self.storage.vmgr, file, len), len);
            }
        }
        self.storage.with_routes_object(name, |source, len| read(source, len))
    }

    fn delete_file(&mut self, name: &ShortFileName) -> bool {
        let Some(dir) = self.storage.routes_dir else { return false };
        match self.storage.vmgr.delete_file_in_dir(dir, name) {
            Ok(()) => true,
            Err(error) => {
                defmt::warn!(
                    "SD: delete {} failed: {} — file kept, catalog unchanged",
                    defmt::Debug2Format(name),
                    defmt::Debug2Format(&error)
                );
                false
            }
        }
    }

    fn close_geometry_if(&mut self, name: &ShortFileName) {
        if let Some((id, file, _)) = self.storage.open_route {
            if self.catalog.get(id).is_some_and(|(_, open_name, _)| open_name == name) {
                let _ = self.storage.vmgr.close_file(file);
                self.storage.open_route = None;
            }
        }
    }

    fn rebind_sideload_sidecars(&mut self) {
        if !self.catalog.sideband_bound(CRC_BINDING) {
            let (mut crcs, authoritative) = self.storage.load_crc_sidecar_status(ROUTE_CRCS);
            clear_session_crcs(&mut crcs);
            if authoritative {
                let persisted = self.storage.write_route_crcs(&crcs);
                self.catalog.record_sideband_rewrite(CRC_BINDING, persisted);
            }
        }
        if !self.catalog.sideband_bound(RETENTION_BINDING) {
            let (mut retention, authoritative) = self.storage.load_route_retention_status();
            retention.clear_ids_from(SIDELOAD_ID_BASE);
            if authoritative {
                let persisted = self.storage.write_route_retention(&retention).is_ok();
                self.catalog.record_sideband_rewrite(RETENTION_BINDING, persisted);
            }
        }
    }

    fn load_crcs(&self) -> RouteCrcs {
        let mut crcs = self.storage.load_route_crcs();
        if !self.catalog.sideband_bound(CRC_BINDING) {
            clear_session_crcs(&mut crcs);
        }
        crcs
    }

    fn set_crc(&mut self, id: u16, crc: u32) {
        let mut crcs = self.load_crcs();
        if crcs.insert(id, crc) && self.storage.write_route_crcs(&crcs) {
            self.catalog.record_sideband_rewrite(CRC_BINDING, true);
        }
    }

    fn forget_crc(&mut self, id: u16) {
        let mut crcs = self.load_crcs();
        if crcs.remove(id) && self.storage.write_route_crcs(&crcs) {
            self.catalog.record_sideband_rewrite(CRC_BINDING, true);
        }
    }

    fn load_retention(&self) -> RouteRetentionStore {
        let mut retention = self.storage.load_route_retention();
        if !self.catalog.sideband_bound(RETENTION_BINDING) {
            retention.clear_ids_from(SIDELOAD_ID_BASE);
        }
        retention
    }

    fn forget_retention(&mut self, id: u16) {
        let mut retention = self.load_retention();
        if retention.set(id, RouteRetentionMeta::default()) && self.storage.write_route_retention(&retention).is_ok() {
            self.catalog.record_sideband_rewrite(RETENTION_BINDING, true);
        }
    }
}
