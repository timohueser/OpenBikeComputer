//! The device object store (A6, issue #274) — the board half that turns S0's object plane into
//! SD files and RRAM settings. `obc-ble` owns the wire (descriptors, CRC, transfer sequencing);
//! [`crate::sd::Storage`] owns FatFs; this module owns the **catalog semantics** between them:
//!
//! - **Object ids** (S0 §4.1): `u16`, **durable for uploaded objects** — the id is encoded in
//!   the SD filename (`RT{id}.OBR`, see `sd.rs`), recovered at the mount scan, and fresh ids
//!   continue monotonically past the highest stored one. Durability matters because the phone
//!   persists the id an upload commits under (`PlannedRouteRecord.deviceObjectID`) and uses it
//!   to badge-reconcile and replace-in-place across device reboots. Side-loaded `.obcr` files
//!   carry no id in their name and get a *session-scoped* one from the reserved
//!   [`SIDELOAD_ID_BASE`] band — the app never persists those (they never come out of an
//!   upload result).
//! - **Store revision + digest** (§4.5): bumped on every commit/delete; the BLE plane notifies
//!   `storeChanged` + the digest characteristic from it.
//! - **The upload state machine**: descriptor → [`Receiver`] (+ temp-file sink) → commit. Uploads
//!   are not resumable (S0 §1 principle 4): an interrupted upload (a drop or an `op=3` abort) is
//!   discarded and the app re-sends the object from the start.
//! - **Downloads**: the `routeList` / `rideList` objects are built into a resident buffer;
//!   a route or ride detail is served straight off the card (CRC pre-pass, then chunk reads —
//!   a stored `RD{id}.ORD` *is* the §7.2 wire object, so a ride download is verbatim, A7).
//! - **Rides are read-only here** (A7): recorded by the map build's ride loop, scanned once at
//!   boot, never mutated over the link — the device retains them until a future device-side
//!   delete, and the app hides synced rides locally instead of deleting them.
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
    Crc32, ListHeader, ObjectStoreDigest, ObjectType, Receiver, RideListEntry, RouteListEntry, StreamSender,
    TransferControl, TransferStatus,
};

use crate::sd::Storage;
use crate::settings::RramSettingsStore;

/// One catalog slot: the object id and where its bytes live (routes and rides alike).
struct ObjectSlot {
    id: u16,
    file: ShortFileName,
    byte_len: u32,
}

/// Ride catalog capacity (A7). Rides accumulate — the device keeps every tracked ride until a
/// (future) manual delete — so this is roomier than [`MAX_ROUTES`]; past it the newest rides
/// stop being listed (warned at scan) until the card is tidied.
pub const MAX_RIDES: usize = 128;

/// The list-object buffer: header + one entry per slot of the **larger** catalog (both lists
/// stream from the same scratch — one transfer at a time, S0 §4.1).
const LIST_BUF_LEN: usize =
    ListHeader::object_len(if MAX_RIDES > MAX_ROUTES { MAX_RIDES } else { MAX_ROUTES });

/// First id of the reserved **session-scoped** band handed to side-loaded `.obcr` files at the
/// mount scan (their names carry no durable id). Uploaded ids grow monotonically from 0 and
/// reject at this floor — 65,024 lifetime uploads before a card must be cleared, i.e. never.
const SIDELOAD_ID_BASE: u16 = 0xFF00;

pub struct ObjectStore {
    /// The mounted card, or `None` (no card): every route operation then answers `error`,
    /// while config ↔ settings (RRAM, card-independent) keeps working.
    storage: Option<Storage>,
    settings_store: RramSettingsStore,
    /// The persisted settings, loaded once at boot — the config plane's read/modify cache.
    settings: Settings,
    routes: Vec<ObjectSlot, MAX_ROUTES>,
    /// The stored rides (A7), scanned once at boot: the `ble` build has no ride loop, so the
    /// catalog can't change while it runs (rides are recorded by the map build, then served
    /// here after a reflash — same card).
    rides: Vec<ObjectSlot, MAX_RIDES>,
    /// The next fresh-upload object id (ids are never reused within a boot).
    next_id: u16,
    /// The store revision (S0 §4.5): monotonic per boot, bumped on every commit/delete.
    revision: u32,
    /// The built list / diagnostics object a download streams from.
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
            rides: Vec::new(),
            next_id: 0,
            revision: 1,
            list_buf: [0; LIST_BUF_LEN],
        };
        store.rescan();
        store.rescan_rides();
        store
    }

    /// Whether a card is mounted (the status screen's `sd` line).
    pub fn sd_ok(&self) -> bool {
        self.storage.is_some()
    }

    /// (Re)build the id table from the card. Uploaded files carry their **durable id in the
    /// filename** (`RT{id}.OBR`); side-loaded `.obcr` files get session ids from the
    /// [`SIDELOAD_ID_BASE`] band. `next_id` resumes past the highest stored upload id, so a
    /// fresh upload can't alias a stored object across reboots.
    fn rescan(&mut self) {
        self.routes.clear();
        let Some(storage) = &mut self.storage else { return };
        let mut names: Vec<ShortFileName, MAX_ROUTES> = Vec::new();
        storage.for_each_route_file(|n| {
            if !names.is_full() {
                let _ = names.push(n.clone());
            }
        });
        let mut next_sideload = SIDELOAD_ID_BASE;
        for name in &names {
            match storage.route_object_info(name) {
                Some((byte_len, _)) => {
                    let id = match crate::sd::uploaded_route_id(name) {
                        Some(id) => {
                            self.next_id = self.next_id.max(id.saturating_add(1));
                            id
                        }
                        None => {
                            let id = next_sideload;
                            next_sideload = next_sideload.saturating_add(1);
                            id
                        }
                    };
                    let _ = self.routes.push(ObjectSlot { id, file: name.clone(), byte_len });
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

    /// Scan `/tracks` for stored ride objects (`RD{id}.ORD`, A7) — the id is durable in the
    /// filename, like the routes'. An interrupted save (the held-back version byte, exactly
    /// that signature) is swept; a merely unreadable file is kept off the catalog but never
    /// deleted. Ordered as the directory lists them; the app sorts by `start_time`.
    fn rescan_rides(&mut self) {
        self.rides.clear();
        let Some(storage) = &mut self.storage else { return };
        let mut entries: Vec<(u16, ShortFileName), MAX_RIDES> = Vec::new();
        let mut overflow = false;
        storage.for_each_ride_file(|id, n| {
            if entries.push((id, n.clone())).is_err() {
                overflow = true;
            }
        });
        if overflow {
            defmt::warn!("store: more than {=usize} ride objects — the excess is not listed", MAX_RIDES);
        }
        for (id, name) in &entries {
            match storage.ride_object_info(name) {
                Some((byte_len, _)) => {
                    let _ = self.rides.push(ObjectSlot { id: *id, file: name.clone(), byte_len });
                }
                None => {
                    if storage.is_aborted_ride_object(name) {
                        defmt::info!("store: sweeping interrupted ride save {}", defmt::Debug2Format(name));
                        let _ = storage.delete_ride_file(name);
                    }
                }
            }
        }
        defmt::info!("store: {=usize} ride object(s)", self.rides.len());
    }

    /// The §4.5 digest.
    pub fn digest(&self) -> ObjectStoreDigest {
        ObjectStoreDigest {
            revision: self.revision,
            route_count: self.routes.len() as u16,
            ride_count: self.rides.len() as u16,
        }
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

    /// Whether a ride object with this id exists (the download-request `notFound` check, A7).
    pub fn has_ride(&self, id: u16) -> bool {
        self.rides.iter().any(|s| s.id == id)
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

    /// Open a fresh upload from its descriptor (uploads restart, not resume — S0 §1 principle 4):
    /// truncate the temp and return the [`Receiver`] to drive, or the typed status to answer
    /// immediately. A non-zero offset is rejected (`Receiver::new`) — the app always sends 0.
    pub fn upload_open(&mut self, desc: &TransferControl) -> Result<Receiver, TransferStatus> {
        // A named id must exist (0xFFFF = fresh); check before touching the temp.
        if desc.object_id != TransferControl::NEW_OBJECT_ID && self.slot_index(desc.object_id).is_none() {
            return Err(TransferStatus::NotFound);
        }
        let rx = Receiver::new(desc).map_err(|_| TransferStatus::Error)?;
        let Some(storage) = &mut self.storage else { return Err(TransferStatus::Error) };
        if !storage.upload_begin() {
            return Err(TransferStatus::Error);
        }
        Ok(rx)
    }

    /// Sink one CoC chunk: append to the temp. False = storage failure (the caller aborts).
    pub fn upload_append(&mut self, bytes: &[u8]) -> bool {
        self.storage.as_mut().is_some_and(|s| s.upload_append(bytes))
    }

    /// The whole link dropped, or the CoC dropped mid-upload, or the app aborted (op 3): discard
    /// the partial upload and release any open storage handles a cancelled future couldn't.
    /// Uploads don't resume, so nothing is kept — the app re-sends from the start.
    pub fn link_reset(&mut self) {
        self.upload_discard();
        if let Some(storage) = &mut self.storage {
            storage.close_object();
        }
    }

    /// Abort/interrupt: discard the in-flight temp (S0 §4.2 — "drains and discards").
    pub fn upload_discard(&mut self) {
        if let Some(storage) = &mut self.storage {
            storage.upload_abort();
        }
    }

    /// All bytes arrived: verify + commit. On a CRC match the temp is promoted (fresh id
    /// assigned / replaced file swapped), the revision bumps, and the result carries the
    /// assigned id (S0 §4.3); on a mismatch nothing is committed and the temp is dropped.
    /// Returns `(object_id, status)` for the `transferResult`.
    pub fn upload_finish(&mut self, rx: &Receiver) -> (u16, TransferStatus) {
        let outcome = match rx.outcome() {
            Some(o) => o,
            None => return (rx.object_id(), TransferStatus::Error), // caller bug: not complete
        };
        if outcome.status != TransferStatus::Committed {
            self.upload_discard();
            return (rx.object_id(), outcome.status);
        }
        let fresh = rx.object_id() == TransferControl::NEW_OBJECT_ID;
        if fresh && (self.routes.is_full() || self.next_id >= SIDELOAD_ID_BASE) {
            // Storage-full, typed (S0 §4.1 duplicate/storage policy): the catalog can't index
            // another object (or the durable-id space is exhausted — practically unreachable),
            // so reject before touching the card's name slots.
            self.upload_discard();
            return (rx.object_id(), TransferStatus::Error);
        }
        let replace_idx = if fresh { None } else { self.slot_index(rx.object_id()) };
        let Some(storage) = &mut self.storage else { return (rx.object_id(), TransferStatus::Error) };
        let replace_file = replace_idx.map(|i| self.routes[i].file.clone());
        match storage.upload_commit(replace_file.as_ref(), self.next_id) {
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
                        let _ = self.routes.push(ObjectSlot { id, file, byte_len });
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
                // No card ≠ no objects: an empty *success* here would let one flaky mount
                // masquerade as "the device holds nothing" — the app takes a committed list
                // as authoritative and durably clears its on-device links off it. Answer the
                // typed error instead; the app keeps its links and retries later.
                if self.storage.is_none() {
                    return Err(TransferStatus::Error);
                }
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
                self.open_object_download(desc, &file, false)
            }
            // A ride download (A7) is the same verbatim stream — the stored `RD{id}.ORD` *is*
            // the S0 §7.2 wire object — just out of `/tracks`.
            ObjectType::Ride => {
                let Some(slot) = self.rides.iter().find(|s| s.id == desc.object_id) else {
                    return Err(TransferStatus::NotFound);
                };
                let file = slot.file.clone();
                self.open_object_download(desc, &file, true)
            }
            _ => Err(TransferStatus::NotFound),
        }
    }

    /// Open a stored object file for a verbatim download: the handle, the CRC pre-pass (the
    /// whole-object CRC the announce carries, S0 §4.2), the [`StreamSender`].
    fn open_object_download(
        &mut self,
        desc: &TransferControl,
        file: &ShortFileName,
        ride: bool,
    ) -> Result<(StreamSender, DownloadSource), TransferStatus> {
        let Some(storage) = &mut self.storage else { return Err(TransferStatus::Error) };
        let opened = if ride { storage.open_ride_object(file) } else { storage.open_object(file) };
        let Some(len) = opened else {
            return Err(TransferStatus::Error);
        };
        let Some(crc) = object_crc(storage, len) else {
            storage.close_object();
            return Err(TransferStatus::Error);
        };
        let tx = StreamSender::new(desc, len, crc).map_err(|_| TransferStatus::Error)?;
        Ok((tx, DownloadSource::Object))
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
    /// Entries come from each stored file's header (one read per object — a full catalog is
    /// ~a hundred header reads, tens of ms, done once per download).
    fn build_list(&mut self, ty: ObjectType) -> usize {
        let mut count: u16 = 0;
        let mut off = ListHeader::ENCODED_LEN;
        if let Some(storage) = &self.storage {
            match ty {
                ObjectType::RouteList => {
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
                ObjectType::RideList => {
                    for slot in &self.rides {
                        let Some((byte_len, info)) = storage.ride_object_info(&slot.file) else {
                            continue; // transiently unreadable — serve the rest
                        };
                        let entry = RideListEntry {
                            object_id: slot.id,
                            byte_len,
                            start_time: info.start_time,
                            distance_m: info.distance_m,
                            moving_time_s: info.moving_time_s,
                            avg_speed_cms: info.avg_speed_cms,
                            climb_m: info.climb_m,
                            name: info.name.as_bytes(),
                        };
                        self.list_buf[off..off + obc_ble::LIST_ENTRY_LEN].copy_from_slice(&entry.encode());
                        off += obc_ble::LIST_ENTRY_LEN;
                        count += 1;
                    }
                }
                _ => {}
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
