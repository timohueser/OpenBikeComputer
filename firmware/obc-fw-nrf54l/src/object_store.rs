//! The device object store (A6, issue #274) — the board half that turns S0's object plane into
//! SD files and RRAM settings. `obc-ble` owns the wire (descriptors, CRC, transfer sequencing);
//! [`crate::sd::Storage`] owns FatFs; this module owns the **catalog semantics** between them:
//!
//! - **Object ids** (S0 §4.1): `u16`, assigned at the mount scan and monotonically for fresh
//!   uploads, never reused within a boot. They are deliberately *session-scoped* — the store
//!   `revision` is already "monotonic per boot; not persisted" (§4.5), so the app re-reads the
//!   digest + list on every connect and never carries an id across a device reboot.
//! - **Store revision + digest** (§4.5): bumped on every commit/delete; the BLE plane notifies
//!   `storeChanged` + the digest characteristic from it.
//! - **The upload state machine**: descriptor → [`Receiver`] (+ temp-file sink) → commit; a CoC
//!   drop parks the running CRC + durable byte count in [`PendingUpload`] so a same-boot resume
//!   continues instead of restarting (S0 §4.2).
//! - **Downloads**: the `routeList` object is built into a resident buffer ([`Self::list_len`]);
//!   a route detail is served straight off the card (CRC pre-pass, then chunk reads).
//! - **Config ↔ settings** (§7.3): the Config blob reads from / writes through the persisted
//!   [`Settings`] (v3's `device_name` + `units`), so a rename survives a power cycle and feeds
//!   the advertised name.
//!
//! Everything here is synchronous SD I/O; the BLE plane borrows the store through a `RefCell`
//! **never across an `await`** (single executor — see `ble.rs`).

use embedded_sdmmc::ShortFileName;
use heapless::Vec;
use obc_app::settings::DeviceName;
use obc_app::{Settings, SettingsStore, MAX_ROUTES};
use obc_ble::{
    Crc32, ListHeader, ObjectStoreDigest, ObjectType, Receiver, RouteListEntry, StreamSender, TransferControl,
    TransferStatus,
};

use crate::sd::Storage;
use crate::settings::RramSettingsStore;

/// One catalog slot: the session object id and where its bytes live.
struct RouteSlot {
    id: u16,
    file: ShortFileName,
    byte_len: u32,
}

/// A parked mid-upload transfer (the CoC dropped): everything a same-boot resume needs. The
/// temp file holds the bytes; this holds the wire identity to match the resume descriptor
/// against and the running CRC at the durable offset.
struct PendingUpload {
    object_id: u16,
    total_len: u32,
    crc32: u32,
    /// Durable bytes in the temp — the `committed_offset` reported to the app (S0 §4.3).
    written: u32,
    /// The running CRC over `temp[..written]`, seeding [`Receiver::resumed`].
    crc: Crc32,
}

/// The routeList object buffer: header + one entry per possible catalog slot.
const LIST_BUF_LEN: usize = ListHeader::object_len(MAX_ROUTES);

pub struct ObjectStore {
    /// The mounted card, or `None` (no card): every route operation then answers `error`,
    /// while config ↔ settings (RRAM, card-independent) keeps working.
    storage: Option<Storage>,
    settings_store: RramSettingsStore,
    /// The persisted settings, loaded once at boot — the config plane's read/modify cache.
    settings: Settings,
    routes: Vec<RouteSlot, MAX_ROUTES>,
    /// The next fresh-upload object id (ids are never reused within a boot).
    next_id: u16,
    /// The store revision (S0 §4.5): monotonic per boot, bumped on every commit/delete.
    revision: u32,
    /// A parked upload awaiting a resume (S0 §4.2), if any.
    pending: Option<PendingUpload>,
    /// The built list object a download streams from (routeList today; rideList serves the
    /// empty header until A7).
    list_buf: [u8; LIST_BUF_LEN],
}

impl ObjectStore {
    /// Mount-time construction: load settings, scan `/routes` into the id table, and sweep
    /// aborted commits (files whose held-back magic never got patched — see `sd.rs`).
    pub fn new(storage: Option<Storage>, mut settings_store: RramSettingsStore) -> Self {
        let settings = settings_store.load().unwrap_or_default();
        let mut store = ObjectStore {
            storage,
            settings_store,
            settings,
            routes: Vec::new(),
            next_id: 0,
            revision: 1,
            pending: None,
            list_buf: [0; LIST_BUF_LEN],
        };
        store.rescan();
        store
    }

    /// Whether a card is mounted (the status screen's `sd` line).
    pub fn sd_ok(&self) -> bool {
        self.storage.is_some()
    }

    /// (Re)build the id table from the card. Ids continue from `next_id` — a rescan after boot
    /// (never needed today, but harmless) can't alias a deleted object's id.
    fn rescan(&mut self) {
        self.routes.clear();
        let Some(storage) = &mut self.storage else { return };
        let mut names: Vec<ShortFileName, MAX_ROUTES> = Vec::new();
        storage.for_each_route_file(|n| {
            if !names.is_full() {
                let _ = names.push(n.clone());
            }
        });
        for name in &names {
            match storage.route_object_info(name) {
                Some((byte_len, _)) => {
                    let id = self.next_id;
                    self.next_id += 1;
                    let _ = self.routes.push(RouteSlot { id, file: name.clone(), byte_len });
                }
                // Unreadable: reclaim it only if it carries the aborted-commit signature (the
                // zeroed magic) — transiently unreadable real routes must be kept.
                None => {
                    if storage.is_aborted_commit(name) {
                        defmt::info!("store: sweeping aborted commit {}", defmt::Debug2Format(name));
                        let _ = storage.delete_route_file(name);
                    }
                }
            }
        }
        defmt::info!("store: {=usize} route object(s), next id {=u16}", self.routes.len(), self.next_id);
    }

    /// The §4.5 digest (rides land at A7 — count 0 until then).
    pub fn digest(&self) -> ObjectStoreDigest {
        ObjectStoreDigest { revision: self.revision, route_count: self.routes.len() as u16, ride_count: 0 }
    }

    fn bump_revision(&mut self) -> u32 {
        self.revision = self.revision.wrapping_add(1);
        self.revision
    }

    fn slot_index(&self, id: u16) -> Option<usize> {
        self.routes.iter().position(|s| s.id == id)
    }

    /// Whether a route object with this id exists (the control plane's cheap `notFound` check).
    pub fn has_route(&self, id: u16) -> bool {
        self.slot_index(id).is_some()
    }

    // ==================== config ↔ settings (S0 §7.3) ====================

    /// The current settings (the config read + the advertised-name source).
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Apply a validated Config write: persist name + units through the RRAM store. The name is
    /// stored verbatim; an empty name clears back to factory (S0 §2 — the factory `OBC-XXXX`
    /// returns to the advertisement).
    pub fn apply_config(&mut self, name: &str, units: u8) {
        self.settings.device_name = DeviceName::from_str_lossy(name);
        self.settings.units = if units == 1 { obc_app::Units::Imperial } else { obc_app::Units::Metric };
        self.settings_store.save(&self.settings);
    }

    // ==================== delete (S0 §4.4 cmd 1) ====================

    /// Delete a stored route by object id. `true` = deleted (revision bumped).
    pub fn delete_route(&mut self, id: u16) -> bool {
        let Some(idx) = self.slot_index(id) else { return false };
        let Some(storage) = &mut self.storage else { return false };
        if !storage.delete_route_file(&self.routes[idx].file) {
            return false;
        }
        self.routes.remove(idx);
        self.bump_revision();
        true
    }

    // ==================== upload (S0 §4.2 op 1) ====================

    /// Open an upload from its descriptor: fresh (`offset == 0`, temp truncated) or a resume
    /// (`offset > 0`, matched against the parked [`PendingUpload`] and the temp's durable
    /// length). Returns the [`Receiver`] to drive, or the typed status to answer immediately.
    pub fn upload_open(&mut self, desc: &TransferControl) -> Result<Receiver, TransferStatus> {
        // A named id must exist (0xFFFF = fresh); check before touching the temp.
        if desc.object_id != TransferControl::NEW_OBJECT_ID && self.slot_index(desc.object_id).is_none() {
            return Err(TransferStatus::NotFound);
        }
        let Some(storage) = &mut self.storage else { return Err(TransferStatus::Error) };
        let rx = if desc.offset == 0 {
            let rx = Receiver::new(desc).map_err(|_| TransferStatus::Error)?;
            if !storage.upload_begin() {
                return Err(TransferStatus::Error);
            }
            rx
        } else {
            // Resume: the descriptor must name the parked transfer (same id/len/CRC) and its
            // offset must equal the durable byte count — anything else is answered `error` and
            // the app restarts fresh (S0 §4.2: the CRC covers the whole object either way).
            let Some(p) = &self.pending else { return Err(TransferStatus::Error) };
            if (desc.object_id, desc.total_len, desc.crc32) != (p.object_id, p.total_len, p.crc32)
                || desc.offset != p.written
            {
                return Err(TransferStatus::Error);
            }
            let rx = Receiver::resumed(desc, p.crc).map_err(|_| TransferStatus::Error)?;
            if !storage.upload_resume(desc.offset) {
                return Err(TransferStatus::Error);
            }
            rx
        };
        // Park state from byte 0: a disconnect can cancel the data-plane future at any await,
        // so the resume anchor is kept current on every append rather than written at the drop.
        self.pending = Some(PendingUpload {
            object_id: rx.object_id(),
            total_len: rx.total_len(),
            crc32: rx.expected_crc(),
            written: rx.committed_offset(),
            crc: rx.crc(),
        });
        Ok(rx)
    }

    /// Sink one CoC chunk: append to the temp and advance the parked resume anchor to the
    /// receiver's state. False = storage failure (the caller aborts with `error`).
    pub fn upload_append(&mut self, bytes: &[u8], rx: &Receiver) -> bool {
        let ok = self.storage.as_mut().is_some_and(|s| s.upload_append(bytes));
        if ok {
            if let Some(p) = &mut self.pending {
                p.written = rx.committed_offset();
                p.crc = rx.crc();
            }
        }
        ok
    }

    /// The CoC dropped mid-upload: flush + close the temp (the parked anchor is already
    /// current) and hand back the durable offset to report
    /// (`transferResult(aborted, committed_offset)`, S0 §4.2/§4.3).
    pub fn upload_park(&mut self) -> u32 {
        if let Some(storage) = &mut self.storage {
            storage.upload_close(); // flushes — the temp now durably holds the parked count
        }
        self.pending.as_ref().map_or(0, |p| p.written)
    }

    /// The whole link dropped: release any open storage handles (a cancelled data-plane future
    /// can't). A parked upload stays parked — the app can reconnect and resume it.
    pub fn link_reset(&mut self) {
        if let Some(storage) = &mut self.storage {
            storage.upload_close();
            storage.close_object();
        }
    }

    /// Abort (op 3 or a failed append): discard the temp and any parked state.
    pub fn upload_discard(&mut self) {
        self.pending = None;
        if let Some(storage) = &mut self.storage {
            storage.upload_abort();
        }
    }

    /// All bytes arrived: verify + commit. On a CRC match the temp is promoted (fresh id
    /// assigned / replaced file swapped), the revision bumps, and the result carries the
    /// assigned id (S0 §4.3); on a mismatch nothing is committed and the temp is dropped.
    /// Returns `(object_id, status)` for the `transferResult`.
    pub fn upload_finish(&mut self, rx: &Receiver) -> (u16, TransferStatus) {
        self.pending = None;
        let outcome = match rx.outcome() {
            Some(o) => o,
            None => return (rx.object_id(), TransferStatus::Error), // caller bug: not complete
        };
        if outcome.status != TransferStatus::Committed {
            self.upload_discard();
            return (rx.object_id(), outcome.status);
        }
        let fresh = rx.object_id() == TransferControl::NEW_OBJECT_ID;
        if fresh && self.routes.is_full() {
            // Storage-full, typed (S0 §4.1 duplicate/storage policy): the catalog can't index
            // another object, so reject before touching the card's name slots.
            self.upload_discard();
            return (rx.object_id(), TransferStatus::Error);
        }
        let replace_idx = if fresh { None } else { self.slot_index(rx.object_id()) };
        let Some(storage) = &mut self.storage else { return (rx.object_id(), TransferStatus::Error) };
        let replace_file = replace_idx.map(|i| self.routes[i].file.clone());
        match storage.upload_commit(replace_file.as_ref()) {
            Some((file, byte_len, _info)) => {
                let id = match replace_idx {
                    Some(i) => {
                        self.routes[i].byte_len = byte_len;
                        self.routes[i].file = file;
                        self.routes[i].id
                    }
                    None => {
                        let id = self.next_id;
                        self.next_id += 1;
                        let _ = self.routes.push(RouteSlot { id, file, byte_len });
                        id
                    }
                };
                self.bump_revision();
                (id, TransferStatus::Committed)
            }
            None => {
                // Validation/copy failed. If this was a replace, the old file may already be
                // gone (deleted after validation, before the copy landed) — re-check it and
                // drop its slot if so, so the catalog matches the card.
                if let Some(i) = replace_idx {
                    let gone =
                        self.storage.as_ref().is_none_or(|s| s.route_object_info(&self.routes[i].file).is_none());
                    if gone {
                        self.routes.remove(i);
                        self.bump_revision();
                    }
                }
                (rx.object_id(), TransferStatus::Error)
            }
        }
    }

    // ==================== downloads (S0 §4.2 op 2) ====================

    /// Open a download: build the list object / open the stored route (with its CRC pre-pass —
    /// the whole-object CRC the announce carries, S0 §4.2). Returns the sender to drive plus
    /// which source [`Self::download_read`] serves from.
    pub fn download_open(&mut self, desc: &TransferControl) -> Result<(StreamSender, DownloadSource), TransferStatus> {
        match desc.ty {
            ObjectType::RouteList | ObjectType::RideList => {
                let len = self.build_list(desc.ty);
                let crc = Crc32::checksum(&self.list_buf[..len]);
                let tx = StreamSender::new(desc, len as u32, crc).map_err(|_| TransferStatus::Error)?;
                Ok((tx, DownloadSource::List))
            }
            ObjectType::Route => {
                let Some(idx) = self.slot_index(desc.object_id) else {
                    return Err(TransferStatus::NotFound);
                };
                let file = self.routes[idx].file.clone();
                let Some(storage) = &mut self.storage else { return Err(TransferStatus::Error) };
                let Some(len) = storage.open_object(&file) else {
                    return Err(TransferStatus::Error);
                };
                let Some(crc) = object_crc(storage, len) else {
                    storage.close_object();
                    return Err(TransferStatus::Error);
                };
                let tx = StreamSender::new(desc, len, crc).map_err(|_| TransferStatus::Error)?;
                Ok((tx, DownloadSource::Object))
            }
            _ => Err(TransferStatus::NotFound),
        }
    }

    /// Read the chunk at `offset` into `buf` from the opened download source. False = read
    /// failure (the caller answers `error`).
    pub fn download_read(&mut self, source: DownloadSource, offset: u32, buf: &mut [u8]) -> bool {
        match source {
            DownloadSource::List => {
                let (start, end) = (offset as usize, offset as usize + buf.len());
                if end > self.list_buf.len() {
                    return false;
                }
                buf.copy_from_slice(&self.list_buf[start..end]);
                true
            }
            DownloadSource::Object => self
                .storage
                .as_ref()
                .and_then(|s| s.object_source())
                .is_some_and(|src| obc_route::ByteSource::read_at(&src, offset, buf).is_ok()),
        }
    }

    /// Close the download's storage handle (done, dropped, or superseded).
    pub fn download_close(&mut self) {
        if let Some(storage) = &mut self.storage {
            storage.close_object();
        }
    }

    /// Build the list object for `ty` into [`Self::list_buf`], returning its byte length.
    /// `routeList` entries come from each stored file's header (one read per route — a full
    /// catalog is ~64 header reads, tens of ms, done once per download); `rideList` is the
    /// empty header until A7 stores rides.
    fn build_list(&mut self, ty: ObjectType) -> usize {
        let mut count: u16 = 0;
        let mut off = ListHeader::ENCODED_LEN;
        if ty == ObjectType::RouteList {
            if let Some(storage) = &self.storage {
                for slot in &self.routes {
                    let Some((byte_len, info)) = storage.route_object_info(&slot.file) else {
                        continue; // transiently unreadable — serve the rest
                    };
                    let entry = RouteListEntry {
                        object_id: slot.id,
                        byte_len,
                        distance_m: info.distance_m,
                        ascent_m: info.ascent_m,
                        point_count: info.point_count,
                        waypoint_count: info.waypoint_count,
                        name: info.name.as_bytes(),
                    };
                    self.list_buf[off..off + obc_ble::LIST_ENTRY_LEN].copy_from_slice(&entry.encode());
                    off += obc_ble::LIST_ENTRY_LEN;
                    count += 1;
                }
            }
        }
        self.list_buf[..ListHeader::ENCODED_LEN].copy_from_slice(&ListHeader { count }.encode());
        off
    }
}

/// Which source an open download streams from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DownloadSource {
    /// The built list object in [`ObjectStore::list_buf`].
    List,
    /// The open route file on the card.
    Object,
}

/// The whole-object CRC pre-pass over the open detail-download file: one sequential read of
/// `len` bytes in card-block-sized chunks. Synchronous (the caller yields between GATT events,
/// not mid-CRC) — ~0.5 s/MB at the 8 MHz bus, and a route object is typically well under one.
fn object_crc(storage: &Storage, len: u32) -> Option<u32> {
    let src = storage.object_source()?;
    let mut crc = Crc32::new();
    let mut buf = [0u8; 512];
    let mut off = 0u32;
    while off < len {
        let n = ((len - off) as usize).min(buf.len());
        obc_route::ByteSource::read_at(&src, off, &mut buf[..n]).ok()?;
        crc.update(&buf[..n]);
        off += n as u32;
    }
    Some(crc.finalize())
}
