//! microSD storage for the nRF54L15 board: map / routes / track log over FatFs.
//!
//! This owns the concrete SPI bus → [`SdCard`] → [`VolumeManager`] stack and reconciles the FAT
//! filesystem to the shared app's *intent*, exactly as the simulator's `RouteStore`/`TrackStore`
//! reconcile a folder of files on the host. The reusable, board-agnostic adapters it hands the
//! format code live in [`obc_platform::sd`] ([`SdByteSource`]/[`SdByteSink`]/[`SdTrackSink`]);
//! everything here is nRF-specific (a dedicated SPIM + a GPIO chip-select).
//!
//! The `Storage` impl and every adapter below are generic over the concrete [`SdCard`] **bus type**
//! (they speak `embedded_sdmmc`'s `BlockDevice` / `TimeSource`). So routes and the chosen map both
//! **stream** from the card and the ride is logged to a temp `.obct` converted to the durable ride
//! object on Finish.
//!
//! ## Card layout (FAT16/FAT32)
//!   `/<name>.obcm`   — the map tile (first one found in the root is loaded)
//!   `/routes/*.obcr` — the route catalog the Route menu lists (side-loaded, long filenames)
//!   `/routes/RT{id}.OBR` — BLE-uploaded routes (the durable object id lives in the name);
//!                      the in-flight upload lives here as `UPLOAD.TMP` until commit
//!   `/tracks/`       — saved rides (created if absent); the in-progress log lives here as
//!                      `TRACK.OBT` and is deleted once converted. Each Finish writes **one**
//!                      artifact: the BLE ride object `RD{id}.ORD` (the durable ride object id
//!                      lives in the name, mirroring `RT{id}.OBR`). The device writes no GPX —
//!                      the phone owns human-format export after sync.
//!
//! ## SPI wiring (nRF54L15-DK, **SERIAL22 / SPIM22** — its own bus, separate from the display)
//!   SCK P1_11 · MISO P1_07 · MOSI P1_06 · CS **P1_12** (software, held low) · GND · 3V3.
//! The card is initialised at [`SD_INIT_HZ`] (≤400 kHz, SD spec) then the bus is re-clocked to
//! [`SD_FAST_HZ`] for bulk transfer — see [`init`]. embassy-nrf's `Spim` exposes no internal MISO
//! pull-up (its `Config` has no `miso_pull`), so the card's DO line must be pulled high externally
//! — most microSD breakouts include this; if not, add a 10 kΩ from MISO (P1_07) to 3V3. (DO
//! floating low during init reads `0x00`, which looks like a hung card.)

// The route-selection + ride-save half of this module (`reconcile_route`/`reconcile_track`,
// `track_sink`, the ride-object namer, `TRACK_TMP`) is the SD `Storage`'s full API; let the write
// path sit unused rather than carve up a module that ports as one piece.
#![allow(dead_code)]

use embassy_embedded_hal::SetConfig;
use embassy_nrf::gpio::Output;
use embassy_nrf::spim::{Config as SpiConfig, Frequency, Spim};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::{
    LfnBuffer, Mode, RawDirectory, RawFile, SdCard, ShortFileName, TimeSource, Timestamp, VolumeIdx, VolumeManager,
};
use heapless::{String, Vec};
use obc_app::{
    decode_synced_rides, encode_synced_rides, SyncedRides, MAX_RIDES, MAX_ROUTES, SYNCED_RIDES_MAX_LEN, UI_RIDES_CAP,
};
use obc_platform::{SdByteSink, SdByteSource, SdTrackSink};
use obc_route::{track_to_ride, RideInfo, RideStats, RouteIndex, RouteObjectInfo, RouteSummary, NAME_CAP};

/// SD clock during the init handshake — the spec caps it at 400 kHz. embassy-nrf's discrete
/// [`Frequency`] ladder has no 400 kHz step, so [`Frequency::K250`] is the fastest in-spec choice
/// (250 kHz). The caller configures the bus at this speed *before* [`init`], which re-clocks to
/// [`SD_FAST_HZ`] once the card is up — `pub` so that single source of the init speed is named.
pub const SD_INIT_HZ: Frequency = Frequency::K250;

/// SD clock for bulk transfer once the card is initialised. [`Frequency::M8`] (8 MHz) is the
/// fastest SERIAL22 reaches on the PERI-domain P1 header (its 16 MHz base ÷2) and well within the
/// 25 MHz default-speed limit — conservative for breadboard jumpers.
const SD_FAST_HZ: Frequency = Frequency::M8;

/// The in-progress ride log on the card — a header-less array of fixed track records (8.3
/// name). Truncated-and-reused per ride, converted to the `RD{id}.ORD` ride object, then
/// deleted on Finish.
const TRACK_TMP: &str = "TRACK.OBT";

/// The synced-ride sidecar in `/tracks` (epic #447 P7 / #454): the set of ride object ids the phone
/// has downloaded at least once, so the Rides screen can render an *unsynced* ride's delete footer
/// with the warning cue. In `/tracks` (not RRAM) so it survives a reflash and travels with the card
/// alongside the rides it flags. Rewritten on a download completion; parsed leniently (a torn/missing
/// file = "nothing synced"). Codec + torn-line semantics live in `obc-app::settings` (host-tested).
const SYNCED_SET: &str = "SYNCED.SET";

/// The in-flight BLE route upload, inside `/routes`. Its extension never matches the catalog scan,
/// so a partial upload — a drop, a power cut — is invisible until [`Storage::upload_commit`]
/// promotes it. Truncated-and-reused per upload.
const UPLOAD_TMP: &str = "UPLOAD.TMP";

/// First id of the reserved **session-scoped** band handed to side-loaded `.obcr` files (their
/// names carry no durable id — see [`Storage::sideload_id`]). Uploaded ids grow monotonically from
/// 0 and reject at this floor — 65,024 lifetime uploads before a card must be cleared, i.e. never.
pub(crate) const SIDELOAD_ID_BASE: u16 = 0xFF00;

/// The concrete SD stack for this board: embassy-nrf's blocking `Spim` wrapped as the `SpiDevice`
/// the card driver wants, an [`SdCard`], and a 6-file/4-dir [`VolumeManager`]. The chip-select is
/// a no-op [`NoCs`] — the *real* CS (P1_12) is held low for the whole session (see [`NoCs`]/[`init`]).
///
/// **Why 6 open files** (the default 4 loses mid-ride uploads): riding with tracking holds three
/// handles for the whole session — the map stream, the active route's geometry, and the ORD track
/// log. A BLE route upload adds its temp (4), and `upload_commit`'s copy-promote (embedded-sdmmc
/// can't rename, see the note above [`Storage::upload_commit`]) holds the reopened temp **and**
/// the final `.OBR` at once — a 5-handle peak, which the 4-slot default answered with a failed
/// commit exactly and only mid-ride. 6 = that peak + one slot of headroom; each slot is a few
/// dozen bytes of `FileInfo`, so the RAM cost is noise.
type SdSpi = Spim<'static>;
type SdDev = ExclusiveDevice<SdSpi, NoCs, Delay>;
type Sd = SdCard<SdDev, Delay>;
/// The open-handle budget (see the 6-file note above) — one set of consts so the manager and the
/// `obc-platform` wrapper aliases below can never drift apart.
const SD_MAX_DIRS: usize = 4;
const SD_MAX_FILES: usize = 6;
const SD_MAX_VOLUMES: usize = 1;
type Vmgr = VolumeManager<Sd, NullTime, SD_MAX_DIRS, SD_MAX_FILES, SD_MAX_VOLUMES>;
/// [`SdByteSource`] over this board's manager (the wrappers are generic over the handle budget).
type Source<'a> = SdByteSource<'a, Sd, NullTime, SD_MAX_DIRS, SD_MAX_FILES, SD_MAX_VOLUMES>;
/// [`SdTrackSink`] over this board's manager.
type TrackSinkT<'a> = SdTrackSink<'a, Sd, NullTime, SD_MAX_DIRS, SD_MAX_FILES, SD_MAX_VOLUMES>;

/// FAT timestamps need a clock; the device has none yet (see [`obc_route::TrackPoint::t_ms`]),
/// so every file gets the epoch. Real dates wait on a clock source.
/// `pub(crate)` only because it surfaces in the adapter return types the loop names.
pub(crate) struct NullTime;
impl TimeSource for NullTime {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp { year_since_1970: 0, zero_indexed_month: 0, zero_indexed_day: 0, hours: 0, minutes: 0, seconds: 0 }
    }
}

/// The mounted card: the volume manager plus the open root/`routes`/`tracks` directory handles
/// and the per-frame reconcile state (which route's geometry is open, which session's log).
pub struct Storage {
    vmgr: Vmgr,
    root: RawDirectory,
    /// `/routes`, or `None` if the card has no such folder (catalog is then empty).
    routes_dir: Option<RawDirectory>,
    /// `/tracks` (created on mount if absent), or `None` if it couldn't be opened/created
    /// (rides then can't be saved, but the rest still works).
    tracks_dir: Option<RawDirectory>,
    /// 8.3 filename of each catalog entry, parallel to the app's route order — so a selected
    /// route index reopens the right `.obcr` (the menu shows the *internal* route name, not
    /// the filename, so 8.3 truncation is invisible).
    route_files: Vec<ShortFileName, MAX_ROUTES>,
    /// Each catalog entry's **object id**, parallel to [`route_files`](Storage::route_files) —
    /// the identity [`App::set_routes_with_ids`](obc_app::App::set_routes_with_ids) remaps held
    /// indices by across live rescans (#450). Uploaded routes carry it in the filename
    /// (`RT{id}.OBR`); side-loaded `.obcr` files get a session id from [`sideload_id`](Storage::sideload_id).
    route_ids: Vec<u16, MAX_ROUTES>,
    /// 8.3 filename of each *ride* catalog entry, parallel to the ride order
    /// [`scan_rides`](Storage::scan_rides) last returned — so a ride's durable object id resolves back
    /// to the `RD{id}.ORD` file for a hold-to-delete (`delete_ride_by_id`, map-only build).
    ride_files: Vec<ShortFileName, UI_RIDES_CAP>,
    /// Each ride catalog entry's **durable object id**, parallel to [`ride_files`](Storage::ride_files)
    /// — filename-encoded (`RD{id}.ORD`), the identity the app's ride-menu remap and the phone's
    /// synced/tombstone sets key on.
    ride_ids: Vec<u16, UI_RIDES_CAP>,
    /// The session's side-load id registry: filename → assigned [`SIDELOAD_ID_BASE`]-band id.
    /// **Append-only** (a delete leaves a tombstone), so a name keeps one id for the whole session
    /// no matter how often — or in which order — the ride loop and the BLE `ObjectStore` rescan;
    /// without this, a delete would shift later side-load ids between the two scans' tables and
    /// the identity remap would unload the wrong route. Session-scoped by design: the app never
    /// persists these ids, and side-loaded files only change while the card is out of the device.
    sideload_ids: Vec<(ShortFileName, u16), MAX_ROUTES>,
    /// Next unassigned side-load id, `u32` so the exhausted case is "past `u16::MAX`", not a
    /// saturating collapse onto 0xFFFF (an aliased id would remap/serve the wrong file).
    next_sideload: u32,
    /// The active route's open geometry file: `(catalog index, handle, length)`. Reopened only
    /// when the selected route changes.
    open_route: Option<(usize, RawFile, u32)>,
    /// The map `.obcm`, opened once at startup and held open for the whole session: `(handle,
    /// length)`. The map streams through this (issue #37) instead of being read resident into
    /// RAM — `map_source` hands out a fresh [`SdByteSource`] over it each redraw.
    open_map: Option<(RawFile, u32)>,
    /// The open ride log for the current tracking session.
    open_track: Option<OpenTrack>,
    /// A finished ride whose log → ride-object conversion hasn't run yet. Finish only closes the
    /// log and stashes this; the ride loop runs [`run_pending_save`](Storage::run_pending_save)
    /// once the confirm animation has left the glass, so the save's blocking SD stretch never
    /// freezes the hold bulge (the "finishing a ride is laggy" bug).
    pending_save: Option<PendingSave>,
    /// The BLE object plane's open route/ride file (a detail download in flight): `(filename,
    /// handle, length)`. A separate slot from `open_route` so a download can't disturb an active
    /// ride's geometry. The name is kept so the catalog scan can recognise (and read through)
    /// this handle instead of a second open — embedded-sdmmc refuses every second open of an
    /// open file (`FileAlreadyOpen`, even ReadOnly), which would silently drop the route from
    /// the catalog (issue #480).
    open_object: Option<(ShortFileName, RawFile, u32)>,
    /// The in-flight BLE upload's open [`UPLOAD_TMP`] handle.
    open_upload: Option<RawFile>,
    /// The real chip-select (P1_12), held LOW for the whole session so the card stays selected.
    /// embedded-sdmmc drives a no-op [`NoCs`] instead; toggling a real CS breaks CMD0 on embassy.
    /// Kept here only to keep the pin driven low — never touched after [`init`].
    _cs: Output<'static>,
}

/// One open `.obct` ride log: the session it belongs to, its file handle, and the save name
/// (the route name, frozen at begin, so a later "swap route" can't rename a finished file).
struct OpenTrack {
    session: u32,
    file: RawFile,
    name: String<NAME_CAP>,
}

/// A Finish waiting for its deferred log → `RD{id}.ORD` conversion: the save name and the ride
/// totals snapshotted on the Finish frame. The log itself is already flushed + closed on the card
/// as [`TRACK_TMP`], so a power cut before the conversion loses nothing a crash mid-ride wouldn't.
struct PendingSave {
    name: String<NAME_CAP>,
    stats: RideStats,
}

/// Bring up the SD card: wrap the SPI bus + CS as a `SpiDevice`, initialise the card at the
/// bus's current (slow) clock, re-clock to [`SD_FAST_HZ`], then mount the FAT volume. Returns
/// `None` on any failure (no card, not FAT, unreadable) so the caller degrades gracefully — never
/// panicking (acceptance criterion). `spi` must already be configured at [`SD_INIT_HZ`].
pub fn init(mut spi: SdSpi, mut cs: Output<'static>) -> Option<Storage> {
    // ≥74 wake-up clocks with CS high (SD spec), then hold CS LOW for the whole session.
    // `ExclusiveDevice` drives a no-op [`NoCs`], so the real CS never toggles high between a command
    // and its reply — which embassy's SPI can't survive (the card drops the bus and CMD0's `0x01` is
    // lost; toggling = CardNotFound, held low = mounts).
    cs.set_high();
    let _ = spi.blocking_write(&[0xFFu8; 10]);
    cs.set_low();
    let dev = ExclusiveDevice::new(spi, NoCs, Delay).ok()?;
    let card = SdCard::new(dev, Delay);
    // `num_bytes` forces the SPI init sequence (must be ≤400 kHz, the bus's current setting).
    match card.num_bytes() {
        Ok(bytes) => defmt::info!("SD: card initialised, {=u64} MB", bytes >> 20),
        Err(e) => {
            defmt::warn!("SD: no card / init failed: {}", defmt::Debug2Format(&e));
            return None;
        }
    }
    // Card is up — re-clock the bus for bulk reads/writes (init speed would crawl the 1.4 MB map).
    // embassy-nrf's `Spim` has no inherent setter, so the bump goes through the `SetConfig` seam;
    // a full default config + the fast frequency, with `orc = 0xFF` so any over-read clocks the SD
    // idle byte on MOSI (the card expects 0xFF during read padding).
    card.spi(|dev| {
        let mut fast = SpiConfig::default();
        fast.frequency = SD_FAST_HZ;
        fast.orc = 0xFF;
        let _ = dev.bus_mut().set_config(&fast);
    });
    Storage::mount(card, cs)
}

/// A no-op chip-select for [`ExclusiveDevice`]. embedded-sdmmc issues each byte as its own
/// `SpiDevice` op, so a real CS would toggle high between a command and its reply — which
/// embassy's SPI doesn't survive (the card drops the bus in the gap; CMD0's `0x01` is lost).
/// Holding the *real* CS low for the whole session and feeding `ExclusiveDevice` this no-op
/// keeps the card selected across commands (the validated held-low workaround).
/// `pub(crate)` only because it surfaces in the adapter return types (like [`NullTime`]).
pub(crate) struct NoCs;
impl embedded_hal::digital::ErrorType for NoCs {
    type Error = core::convert::Infallible;
}
impl embedded_hal::digital::OutputPin for NoCs {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn set_high(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl Storage {
    /// Iterate `dir`'s entries with their long filenames, running `f` per entry. Wraps
    /// `iterate_dir_lfn`'s [`LfnBuffer`] scratch setup (a 256-byte buffer is ample for an 8.3 dir),
    /// so the `.obcm`/`.obcr` scans below don't each repeat it. The iteration error is ignored —
    /// a partial scan still yields what it read, the same as the bare call did.
    fn iter_dir_lfn(&self, dir: RawDirectory, mut f: impl FnMut(&embedded_sdmmc::DirEntry, Option<&str>)) {
        let mut lfn_storage = [0u8; 256];
        let mut lfn = LfnBuffer::new(&mut lfn_storage);
        let _ = self.vmgr.iterate_dir_lfn(dir, &mut lfn, |e, long| f(e, long));
    }

    /// Mount the first FAT volume and open the root / `routes` / `tracks` directories.
    fn mount(card: Sd, cs: Output<'static>) -> Option<Storage> {
        // `new()` is pinned to the 4,4,1 defaults — the custom budget goes through `new_with_limits`
        // (5000 = the id offset `new()` itself uses).
        let vmgr: Vmgr = VolumeManager::new_with_limits(card, NullTime, 5000);
        let volume = match vmgr.open_raw_volume(VolumeIdx(0)) {
            Ok(v) => v,
            Err(e) => {
                defmt::warn!("SD: no FAT volume: {}", defmt::Debug2Format(&e));
                return None;
            }
        };
        let root = vmgr.open_root_dir(volume).ok()?;
        let routes_dir = vmgr.open_dir(root, "routes").ok();
        // `/tracks` must exist to save rides — create it if the card doesn't have one yet.
        let tracks_dir = match vmgr.open_dir(root, "tracks") {
            Ok(d) => Some(d),
            Err(_) => {
                let _ = vmgr.make_dir_in_dir(root, "tracks");
                vmgr.open_dir(root, "tracks").ok()
            }
        };
        defmt::info!("SD: mounted; /routes {=bool}, /tracks {=bool}", routes_dir.is_some(), tracks_dir.is_some());
        Some(Storage {
            vmgr,
            root,
            routes_dir,
            tracks_dir,
            route_files: Vec::new(),
            route_ids: Vec::new(),
            ride_files: Vec::new(),
            ride_ids: Vec::new(),
            sideload_ids: Vec::new(),
            next_sideload: SIDELOAD_ID_BASE as u32,
            open_route: None,
            open_map: None,
            open_track: None,
            pending_save: None,
            open_object: None,
            open_upload: None,
            _cs: cs,
        })
    }

    /// Scan `/routes` for the catalog (side-loaded `.obcr` + uploaded `.OBR`), read each header
    /// into a [`RouteSummary`], and return the catalog for
    /// [`App::set_routes_with_ids`](obc_app::App::set_routes_with_ids). Also records, parallel to
    /// the catalog, the 8.3 filenames (so a later selection reopens the right file) and each
    /// entry's object id ([`route_ids`](Storage::route_ids)) — recovered from an upload's
    /// `RT{id}.OBR` name, or a session-stable [`sideload_id`](Storage::sideload_id) for `.obcr`
    /// files. Filenames are collected first, then opened — opening a file inside the iteration
    /// callback would re-enter the volume manager's lock.
    ///
    /// Called at boot **and** on every store-changed edge (#450) — the live rescan is this same
    /// scan re-run; identity across the re-runs is exactly what the id column carries.
    ///
    /// Matching is on the **long** name: the 8.3 short name truncates `.obcr`/`.obcm` to the
    /// 3-char `OBC`, so the short extension can't tell routes from maps. The long name also lets
    /// us skip macOS `._*`/`.DS_Store` clutter (any dot-prefixed name).
    pub fn scan_routes(&mut self) -> Vec<RouteSummary, MAX_ROUTES> {
        let mut catalog: Vec<RouteSummary, MAX_ROUTES> = Vec::new();
        // A file this `Storage` already holds open can't be opened a second time — embedded-sdmmc
        // 0.9 answers `FileAlreadyOpen` for *any* re-open, ReadOnly included — so a scan that meets
        // one must read through the existing handle or the route silently drops out of the catalog
        // and the identity remap unloads it from the menu (the #480 vanishing-routes bug). The ride
        // loop closes the geometry handle before its rescan; this is the backstop for that, and the
        // fix for a scan racing an in-flight detail download (`open_object`). The geometry's name is
        // resolved against the *outgoing* table, before the clear below.
        let open_geometry =
            self.open_route.and_then(|(i, f, len)| self.route_files.get(i).map(|n| (n.clone(), f, len)));
        self.route_files.clear();
        self.route_ids.clear();
        let Some(dir) = self.routes_dir else { return catalog };

        let mut names: Vec<ShortFileName, MAX_ROUTES> = Vec::new();
        let mut overflow = false;
        self.iter_dir_lfn(dir, |e, long| {
            if is_route_entry(e, long) && names.push(e.name.clone()).is_err() {
                overflow = true;
            }
        });
        if overflow {
            defmt::warn!("SD: scan: more than {=usize} route files — the excess is not listed", MAX_ROUTES);
        }

        for n in &names {
            // Id first: a route without an id can't be listed (the remap and the BLE catalog both
            // key on it) — only the exhausted side-load band hits this, warned in `sideload_id`.
            let Some(id) = uploaded_route_id(n).or_else(|| self.sideload_id(n)) else {
                defmt::warn!("SD: scan: {} has no object id — not listed", defmt::Debug2Format(n));
                continue;
            };
            // Open the file — or serve it through a handle this `Storage` already holds (above).
            let (file, len, borrowed) = match self.vmgr.open_file_in_dir(dir, n, Mode::ReadOnly) {
                Ok(f) => (f, self.vmgr.file_length(f).unwrap_or(0), false),
                Err(e) => match (&open_geometry, &self.open_object) {
                    (Some((gn, gf, glen)), _) if gn == n => (*gf, *glen, true),
                    (_, Some((on, of, olen))) if on == n => (*of, *olen, true),
                    _ => {
                        defmt::warn!(
                            "SD: scan: cannot open {}: {} — route not listed until the next rescan",
                            defmt::Debug2Format(n),
                            defmt::Debug2Format(&e)
                        );
                        continue;
                    }
                },
            };
            let src = SdByteSource::new(&self.vmgr, file, len);
            match RouteSummary::read(&src) {
                Ok(sum) => {
                    if catalog.push(sum).is_ok() {
                        let _ = self.route_files.push(n.clone());
                        let _ = self.route_ids.push(id);
                    }
                }
                Err(_) => defmt::warn!("SD: scan: {} unreadable — not listed", defmt::Debug2Format(n)),
            }
            if !borrowed {
                let _ = self.vmgr.close_file(file);
            }
        }
        // The open geometry's catalog *index* may have moved with the rebuilt tables — re-point it
        // so `reconcile_route`/`route_source` keep serving the right file (or release the handle if
        // the file left the catalog altogether).
        if let Some((gn, gf, glen)) = open_geometry {
            self.open_route = self.route_files.iter().position(|n| *n == gn).map(|i| (i, gf, glen));
            if self.open_route.is_none() {
                let _ = self.vmgr.close_file(gf);
            }
        }
        defmt::info!("SD: {=usize} route(s) in /routes", catalog.len());
        catalog
    }

    /// Each catalog entry's object id, parallel to the catalog [`scan_routes`](Storage::scan_routes)
    /// last returned — the second argument to
    /// [`App::set_routes_with_ids`](obc_app::App::set_routes_with_ids).
    pub fn route_ids(&self) -> &[u16] {
        &self.route_ids
    }

    /// Scan `/tracks` for stored ride objects into the app's Rides menu (epic #447 P7 / #454):
    /// [`RideSummary`](obc_app::RideSummary) per `RD{id}.ORD` — the **newest [`UI_RIDES_CAP`]**
    /// (by `start_time`), newest first — each stamped with its `synced` flag from the
    /// [`SYNCED_SET`] sidecar (read once here, not per file). Fills the parallel
    /// [`ride_files`](Storage::ride_files)/[`ride_ids`](Storage::ride_ids) tables so a
    /// hold-to-delete can resolve a durable id back to its file.
    ///
    /// **Stack discipline** (this fn hard-faulted the 256 KB part at boot, twice, before it
    /// respected the budget): the first cut stacked an ~8 KB aligned sort temp on the ~6 KB
    /// 128-cap catalog (>16 KB one frame); the second kept a 128-cap catalog whose *resident*
    /// twin in `App`+`Storage` ate the deep-render path's last ~1.6 KB of margin — statics and
    /// stack are zero-sum on this part. Now the catalog is [`UI_RIDES_CAP`]-capped (~1.4 KB) and
    /// ordering is a bounded **top-K insertion** as summaries are read — no sort temp at all.
    pub fn scan_rides(&mut self) -> Vec<obc_app::RideSummary, UI_RIDES_CAP> {
        let mut catalog: Vec<obc_app::RideSummary, UI_RIDES_CAP> = Vec::new();
        self.ride_files.clear();
        self.ride_ids.clear();
        let synced = self.load_synced_set();
        let Some(dir) = self.tracks_dir else { return catalog };

        // Collect (id, name) for every RD{id}.ORD; an in-flight download's open handle is read
        // through rather than re-opened (embedded-sdmmc refuses a second open, #480).
        let mut entries: Vec<(u16, ShortFileName), MAX_RIDES> = Vec::new();
        let mut overflow = false;
        self.iter_dir_lfn(dir, |e, _| {
            if let Some(id) = stored_ride_id(&e.name) {
                if entries.push((id, e.name.clone())).is_err() {
                    overflow = true;
                }
            }
        });
        if overflow {
            defmt::warn!("SD: scan: more than {=usize} ride files — the excess is not listed", MAX_RIDES);
        }

        // Read each header and keep the newest UI_RIDES_CAP via bounded insertion: find the
        // summary's slot by descending start_time; a full catalog drops the oldest (or skips the
        // candidate when it is the oldest). The three parallel tables move together on every
        // insert/evict, staying aligned.
        for (id, n) in &entries {
            let (file, len, borrowed) = match self.vmgr.open_file_in_dir(dir, n, Mode::ReadOnly) {
                Ok(f) => (f, self.vmgr.file_length(f).unwrap_or(0), false),
                Err(_) => match &self.open_object {
                    Some((on, of, olen)) if on == n => (*of, *olen, true),
                    _ => {
                        defmt::warn!("SD: scan: cannot open ride {} — not listed", defmt::Debug2Format(n));
                        continue;
                    }
                },
            };
            match RideInfo::read(&SdByteSource::new(&self.vmgr, file, len)) {
                Ok(info) => {
                    let sum = obc_app::RideSummary::from_info(&info, synced.contains(*id));
                    let pos = catalog.iter().position(|c| sum.start_time > c.start_time).unwrap_or(catalog.len());
                    // A full catalog evicts its oldest for a newer candidate; a candidate older
                    // than everything listed is simply not one of the newest UI_RIDES_CAP.
                    if catalog.is_full() && pos < catalog.len() {
                        let _ = catalog.pop();
                        let _ = self.ride_files.pop();
                        let _ = self.ride_ids.pop();
                    }
                    if pos <= catalog.len() && !catalog.is_full() {
                        let _ = catalog.insert(pos, sum);
                        let _ = self.ride_files.insert(pos, n.clone());
                        let _ = self.ride_ids.insert(pos, *id);
                    }
                }
                Err(_) => defmt::warn!("SD: scan: ride {} unreadable — not listed", defmt::Debug2Format(n)),
            }
            if !borrowed {
                let _ = self.vmgr.close_file(file);
            }
        }

        if entries.len() > catalog.len() {
            defmt::info!("SD: rides menu lists the newest {=usize} of {=usize} stored", catalog.len(), entries.len());
        }
        defmt::info!("SD: {=usize} ride(s) in /tracks", catalog.len());
        catalog
    }

    /// Each ride catalog entry's durable object id, parallel to the catalog
    /// [`scan_rides`](Storage::scan_rides) last returned — the second argument to
    /// [`App::set_rides`](obc_app::App::set_rides).
    pub fn ride_ids(&self) -> &[u16] {
        &self.ride_ids
    }

    /// Read the synced-ride sidecar (`/tracks/SYNCED.SET`) into a [`SyncedRides`] set. A missing,
    /// torn, or malformed sidecar decodes to the **empty** set ("nothing synced") — never a panic
    /// (the codec + torn-line semantics are host-tested in `obc-app::settings`). One file read.
    pub fn load_synced_set(&self) -> SyncedRides {
        let Some(dir) = self.tracks_dir else { return SyncedRides::new() };
        let Ok(file) = self.vmgr.open_file_in_dir(dir, SYNCED_SET, Mode::ReadOnly) else {
            return SyncedRides::new(); // absent = nothing synced
        };
        let mut buf = [0u8; SYNCED_RIDES_MAX_LEN];
        let n = self.vmgr.read(file, &mut buf).unwrap_or(0);
        let _ = self.vmgr.close_file(file);
        decode_synced_rides(&buf[..n])
    }

    /// Record ride `id` as synced and persist the sidecar, but only when it's a **new** entry (a
    /// re-download of an already-flagged ride rewrites nothing). Returns `true` if the sidecar
    /// changed. Called at a ride download's completion (epic #447 P7). Read-modify-write within the
    /// call — the handle is opened, written truncating, and closed here, so it never counts against
    /// the open-file budget across an `await`.
    pub fn mark_ride_synced(&mut self, id: u16) -> bool {
        let mut set = self.load_synced_set();
        if !set.insert(id) {
            return false; // already synced (or the set is full) — no rewrite
        }
        self.write_synced_set(&set);
        true
    }

    /// Retire ride `id`'s synced flag from the sidecar (a deleted ride — ids never reuse, so this is
    /// belt-and-braces tidiness). Rewrites the sidecar only when the flag was present. The `ble`
    /// build's `ObjectStore::delete_ride` calls this (the map-only [`delete_ride_by_id`] inlines it).
    pub fn forget_ride_synced(&mut self, id: u16) {
        let mut set = self.load_synced_set();
        if set.remove(id) {
            self.write_synced_set(&set);
        }
    }

    /// Overwrite the synced-ride sidecar (truncating). A write failure is warned, not fatal — the
    /// worst case is a ride reads as unsynced next boot (the safe default), never a crash.
    fn write_synced_set(&mut self, set: &SyncedRides) {
        let Some(dir) = self.tracks_dir else { return };
        let mut buf = [0u8; SYNCED_RIDES_MAX_LEN];
        let n = encode_synced_rides(set, &mut buf);
        match self.vmgr.open_file_in_dir(dir, SYNCED_SET, Mode::ReadWriteCreateOrTruncate) {
            Ok(file) => {
                if self.vmgr.write(file, &buf[..n]).is_err() {
                    defmt::warn!("SD: synced-set write failed — a ride may read unsynced next boot");
                }
                let _ = self.vmgr.flush_file(file);
                let _ = self.vmgr.close_file(file);
            }
            Err(e) => defmt::warn!("SD: cannot open synced-set: {}", defmt::Debug2Format(&e)),
        }
    }

    /// Delete the stored ride with durable object id `id` (the map-only build's hold-to-delete, epic
    /// #447 P7 / #454): resolve the id → `RD{id}.ORD` file via the scan-parallel
    /// [`ride_ids`](Storage::ride_ids)/[`ride_files`](Storage::ride_files) tables, close it if this
    /// `Storage` holds it open, delete it, and retire its flag from the synced sidecar. `true` =
    /// deleted. The `ble` build routes deletes through `ObjectStore` instead (see `object_store.rs`).
    pub fn delete_ride_by_id(&mut self, id: u16) -> bool {
        let Some(pos) = self.ride_ids.iter().position(|&x| x == id) else { return false };
        let name = self.ride_files[pos].clone();
        // An open detail-download handle on this ride must be closed before the delete — embedded-sdmmc
        // refuses to delete an open file (#485).
        if matches!(&self.open_object, Some((on, ..)) if *on == name) {
            self.close_object();
        }
        if !self.delete_ride_file(&name) {
            return false;
        }
        self.forget_ride_synced(id); // tidy the sidecar (ids never reuse, so belt-and-braces)
        true
    }

    /// The **session-scoped** id for a side-loaded route file: the one already registered for this
    /// name, or the next from the [`SIDELOAD_ID_BASE`] band. The registry is append-only for the
    /// session (see the field doc), so every scan — the ride loop's and the BLE `ObjectStore`'s,
    /// in any order, across deletes — hands the same name the same id. `None` when the band or the
    /// registry is exhausted (the route is then not listed, rather than aliased onto a wrong id).
    pub(crate) fn sideload_id(&mut self, name: &ShortFileName) -> Option<u16> {
        if let Some((_, id)) = self.sideload_ids.iter().find(|(n, _)| n == name) {
            return Some(*id);
        }
        if self.next_sideload > u16::MAX as u32 {
            defmt::warn!("SD: side-load id band exhausted — a route is not listed");
            return None;
        }
        let id = self.next_sideload as u16;
        if self.sideload_ids.push((name.clone(), id)).is_err() {
            defmt::warn!("SD: side-load id registry full — a route is not listed");
            return None;
        }
        self.next_sideload += 1;
        Some(id)
    }

    /// Open the first `*.obcm` in the card root and hold it open for the session, so the map can
    /// **stream** from it (issue #37) rather than be read resident into RAM. Returns the file
    /// length on success, or `None` if there's no map file / it can't be opened. Call once at
    /// startup; [`map_source`](Self::map_source) then hands out a reader over the open handle.
    pub fn open_map(&mut self) -> Option<u32> {
        if let Some((_, len)) = self.open_map {
            return Some(len);
        }
        let mut found: Option<ShortFileName> = None;
        self.iter_dir_lfn(self.root, |e, long| {
            if found.is_none() && !e.attributes.is_directory() && long_has_ext(long, b".obcm") {
                found = Some(e.name.clone());
            }
        });
        let name = found?;
        let file = self.vmgr.open_file_in_dir(self.root, &name, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0);
        if len == 0 {
            let _ = self.vmgr.close_file(file);
            return None;
        }
        self.open_map = Some((file, len));
        Some(len)
    }

    /// A [`ByteSource`](obc_route::ByteSource) over the open map file, for reading the header
    /// ([`obc_reader::read_header`]) or building a per-frame [`Reader`](obc_reader::Reader). `None` if
    /// no map was opened ([`open_map`](Self::open_map) returned `None`). Cheap — the source just wraps
    /// the already-open handle, so it's rebuilt every redraw, keeping no borrow across the `&mut self`
    /// route/track operations.
    pub fn map_source(&self) -> Option<Source<'_>> {
        self.open_map.map(|(f, len)| SdByteSource::new(&self.vmgr, f, len))
    }

    /// Make the open route geometry match the app's selected route (a catalog index), reopening
    /// the `.obcr` only when the selection changes — cheap to call every frame, like the sim's
    /// `RouteStore::sync_active`.
    pub fn reconcile_route(&mut self, want: Option<usize>) {
        if self.open_route.map(|(i, _, _)| i) == want {
            return;
        }
        if let Some((_, f, _)) = self.open_route.take() {
            let _ = self.vmgr.close_file(f);
        }
        if let (Some(i), Some(dir)) = (want, self.routes_dir) {
            if let Some(n) = self.route_files.get(i) {
                if let Ok(file) = self.vmgr.open_file_in_dir(dir, n, Mode::ReadOnly) {
                    let len = self.vmgr.file_length(file).unwrap_or(0);
                    self.open_route = Some((i, file, len));
                }
            }
        }
    }

    /// A [`ByteSource`](obc_route::ByteSource) over the active route's open file, for opening a
    /// [`RouteReader`](obc_route::RouteReader) to stream geometry from. `None` when no route is
    /// loaded.
    pub fn route_source(&self) -> Option<Source<'_>> {
        self.open_route.map(|(_, f, len)| SdByteSource::new(&self.vmgr, f, len))
    }

    /// Parse the active route's [`RouteIndex`] — the header plus the **full chunk-meta walk**, the one
    /// up-front per-route cost. The render loop builds this once when the active route changes and
    /// reuses it across frames: a redraw then streams only the visible geometry chunks, instead of
    /// re-walking the whole index off the card at panel rate. `None` when no route is open or the index
    /// read fails (a flaky link) — the loop retries the build on a later redraw, so a transient glitch
    /// doesn't hide the route.
    pub fn build_route_index(&self) -> Option<RouteIndex> {
        let src = self.route_source()?;
        RouteIndex::read(&src).ok()
    }

    /// Reconcile the open ride log to the app's tracking intent — call once per frame *before*
    /// ticking, mirroring the sim's `TrackStore::reconcile`. Drains the one-shot disposition
    /// first (finalising / abandoning the current log), then opens a fresh log when the session
    /// id changes. `name` is the active route's name (the save filename); `stats` is the app's
    /// ride totals + wall-clock anchor, read the same frame — the ride object's header. `marks`
    /// is the RRAM id high-water store (#450), threaded through for the ride-id allocation a
    /// back-to-back begin's early flush may need.
    pub fn reconcile_track(
        &mut self,
        action: Option<obc_app::TrackAction>,
        session: Option<u32>,
        name: &str,
        stats: Option<&RideStats>,
        marks: &mut crate::settings::RramSettingsStore,
    ) {
        use obc_app::TrackAction;
        match action {
            Some(TrackAction::Save) => self.finalize_track(stats),
            Some(TrackAction::Discard) => self.abandon_track(),
            None => {}
        }
        match session {
            Some(id) if self.open_track.as_ref().map(|o| o.session) != Some(id) => self.begin_track(id, name, marks),
            None => self.abandon_track(), // no session → ensure nothing is left open
            _ => {}                       // same session → keep appending
        }
    }

    /// The [`TrackSink`](obc_app::TrackSink) for the open log, or `None` when not recording.
    pub fn track_sink(&self) -> Option<TrackSinkT<'_>> {
        self.open_track.as_ref().map(|o| SdTrackSink::new(&self.vmgr, o.file))
    }

    /// Open (truncating) a fresh `TRACK.OBT` for session `id`, to be saved as `name`.
    fn begin_track(&mut self, id: u32, name: &str, marks: &mut crate::settings::RramSettingsStore) {
        // A still-deferred previous Finish must convert **before** the truncate below destroys its
        // log — the rare back-to-back case (Finish, then a new ride within the same quiet moment)
        // pays the blocking save up front rather than losing the ride.
        self.run_pending_save(marks);
        self.abandon_track(); // close any previous handle first
        let Some(dir) = self.tracks_dir else { return };
        match self.vmgr.open_file_in_dir(dir, TRACK_TMP, Mode::ReadWriteCreateOrTruncate) {
            Ok(file) => {
                let mut nm = String::new();
                let _ = nm.push_str(name);
                self.open_track = Some(OpenTrack { session: id, file, name: nm });
                defmt::info!("SD: recording ride → {=str}", name);
            }
            Err(e) => defmt::warn!("SD: cannot open ride log: {}", defmt::Debug2Format(&e)),
        }
    }

    /// Finish the ride log: flush + close the temp and **stash** the log → ride-object conversion
    /// as a [`PendingSave`] — deliberately doing no bulk SD work here. The Finish gesture lands
    /// mid-confirm-animation, and the conversion is the longest blocking SD stretch the ride loop
    /// has; the loop runs [`run_pending_save`](Self::run_pending_save) once the glass is quiet.
    /// With no ride totals to head the object (`stats == None` — can't happen on a real Finish)
    /// the temp is kept unconverted rather than writing a headerless object.
    fn finalize_track(&mut self, stats: Option<&RideStats>) {
        let Some(ot) = self.open_track.take() else { return };
        let _ = self.vmgr.flush_file(ot.file);
        let _ = self.vmgr.close_file(ot.file);
        let Some(stats) = stats else {
            defmt::warn!("SD: finish without ride stats — kept TRACK.OBT unconverted");
            return;
        };
        self.pending_save = Some(PendingSave { name: ot.name, stats: *stats });
    }

    /// Whether a finished ride still awaits its deferred conversion — the ride loop keeps its
    /// short wake cadence while this is true, so the save actually runs.
    pub fn has_pending_save(&self) -> bool {
        self.pending_save.is_some()
    }

    /// Run a deferred Finish, if any: one streaming [`track_to_ride`] pass over the closed
    /// `TRACK.OBT` into a confirmed-free `RD{id}.ORD`, deleting the temp **only once the ride is
    /// safely in the object**. Any path that can't guarantee a clean save — no confirmed-free
    /// name, the object won't open, or the conversion errors — keeps `TRACK.OBT` so the ride
    /// isn't lost to a transient SD glitch (a card-pull can still recover it; a fresh ride
    /// truncates it, as before).
    ///
    /// `track_to_ride` holds the version byte back and patches it in as its final write, so a
    /// power cut mid-save leaves a file every reader rejects (and whose id-in-name still reserves
    /// the id — a later ride can't alias it for the app's synced-set).
    ///
    /// This is the ride path's one long blocking SD stretch, which is exactly why it is deferred:
    /// the ride loop calls it only when the hold bulge is quiet, and [`begin_track`](Self::begin_track)
    /// flushes it early if a new ride would otherwise truncate the unconverted temp.
    ///
    /// `marks` is the RRAM id high-water store (#450): the ride id is allocated as
    /// `max(scan_max + 1, stored floor)` and the floor is bumped past it **before** the object is
    /// written — so once device-side ride deletion exists, a deleted id can never be re-issued to
    /// a later ride and alias it in the phone's synced/tombstone sets. A blank/torn floor line
    /// decodes to "no floor" and this degrades to exactly the old scan-max + 1.
    pub fn run_pending_save(&mut self, marks: &mut crate::settings::RramSettingsStore) {
        let Some(ps) = self.pending_save.take() else { return };
        let Some(dir) = self.tracks_dir else { return };
        let Ok(src_file) = self.vmgr.open_file_in_dir(dir, TRACK_TMP, Mode::ReadOnly) else {
            defmt::warn!("SD: pending save: cannot reopen TRACK.OBT — kept for a card-pull");
            return;
        };
        let len = self.vmgr.file_length(src_file).unwrap_or(0);

        let mut m = marks.load_id_marks().unwrap_or_default();
        let id = m.alloc_ride(self.next_ride_id(dir));
        marks.save_id_marks(&m); // one 16-byte RRAM line per ride finish — the durable never-reuse floor
        let saved = match self.fresh_object_name(dir, "RD", id, "ORD") {
            Some(file_name) => match self.vmgr.open_file_in_dir(dir, &file_name, Mode::ReadWriteCreateOrTruncate) {
                Ok(dst_file) => {
                    let source = SdByteSource::new(&self.vmgr, src_file, len);
                    let mut sink = SdByteSink::new(&self.vmgr, dst_file);
                    let ok = match track_to_ride(&source, &ps.name, &ps.stats, &mut sink) {
                        Ok(()) => {
                            defmt::info!("SD: saved ride → tracks/RD{=u16}.ORD", id);
                            true
                        }
                        Err(e) => {
                            defmt::warn!("SD: ride-object write failed: {} — kept TRACK.OBT", defmt::Debug2Format(&e));
                            false
                        }
                    };
                    let _ = self.vmgr.flush_file(dst_file);
                    let _ = self.vmgr.close_file(dst_file);
                    ok
                }
                Err(e) => {
                    defmt::warn!("SD: cannot open ride object: {} — kept TRACK.OBT", defmt::Debug2Format(&e));
                    false
                }
            },
            None => {
                defmt::warn!("SD: ride-object name RD{=u16}.ORD unavailable — kept TRACK.OBT (no overwrite)", id);
                false
            }
        };
        let _ = self.vmgr.close_file(src_file);
        // Drop the temp only after the ride is confirmed written; otherwise keep it.
        if saved {
            let _ = self.vmgr.delete_file_in_dir(dir, TRACK_TMP);
        }
    }

    /// One past the highest ride object id stored in `/tracks` (0 on a virgin card) — the **scan
    /// half** of the ride-id allocation. On its own it would resurrect a deleted id; the caller
    /// (`run_pending_save`) maxes it against the persisted RRAM high-water floor (#450), which is
    /// what keeps ids unique across deletes and reboots.
    fn next_ride_id(&self, dir: RawDirectory) -> u16 {
        let mut next = 0u16;
        self.iter_dir_lfn(dir, |e, _| {
            if let Some(id) = stored_ride_id(&e.name) {
                next = next.max(id.saturating_add(1));
            }
        });
        next
    }

    /// Drop the open log without saving (Discard, or a no-session reconcile), deleting the temp.
    fn abandon_track(&mut self) {
        if let Some(ot) = self.open_track.take() {
            let _ = self.vmgr.close_file(ot.file);
            if let Some(dir) = self.tracks_dir {
                let _ = self.vmgr.delete_file_in_dir(dir, TRACK_TMP);
            }
        }
    }

    /// Whether `name` is **confirmed absent** in `dir` — i.e. safe to create without overwriting.
    /// Only `embedded_sdmmc::Error::NotFound` counts as free — the *only* answer that proves a
    /// name is unused. A present entry, or **any other error** (a transient `DeviceError` on the
    /// flaky breadboard link — the same condition the render loop guards a route read against),
    /// is treated as "taken", so a glitch can never green-light reusing a name and truncate an
    /// existing object.
    fn name_is_free(&self, dir: RawDirectory, name: &str) -> bool {
        matches!(self.vmgr.find_directory_entry(dir, name), Err(embedded_sdmmc::Error::NotFound))
    }
}

// ==================== The BLE route-object plane ====================
//
// The storage half of the route object: upload (stream → temp → validated promote), detail
// download (an open handle + `ByteSource`), delete, and the per-file facts the `routeList`
// entries serve. The BLE control plane serialises everything (one transfer at a time), so these
// never contend with each other; on the `ble` build there is no ride loop, so they never contend
// with the map plane either.
//
// **Atomicity without `rename`** — embedded-sdmmc 0.9 cannot rename, so the same guarantee is got
// another way: the upload streams into [`UPLOAD_TMP`] (an extension the catalog scan never
// matches), and `upload_commit` copies it to its final `.OBR` name **with the 4-byte `OBCR` magic
// held back as zeros**, patching the magic in as the last write. A power cut at any point leaves
// either the invisible temp or a zero-magic final file — [`is_route_entry`] may list the latter,
// but every header read rejects it (`BadMagic`), so it can never reach a catalog;
// [`Storage::is_aborted_commit`] identifies exactly that signature so the boot sweep can reclaim
// the name.
impl Storage {
    /// `/routes`, created on demand — a virgin card must accept its first upload.
    fn routes_dir_or_create(&mut self) -> Option<RawDirectory> {
        if self.routes_dir.is_none() {
            let _ = self.vmgr.make_dir_in_dir(self.root, "routes");
            self.routes_dir = self.vmgr.open_dir(self.root, "routes").ok();
        }
        self.routes_dir
    }

    /// Visit every catalog file in `/routes` (side-loaded `.obcr` + uploaded `.OBR`).
    pub fn for_each_route_file(&self, mut f: impl FnMut(&ShortFileName)) {
        let Some(dir) = self.routes_dir else { return };
        self.iter_dir_lfn(dir, |e, long| {
            if is_route_entry(e, long) {
                f(&e.name);
            }
        });
    }

    /// A stored route's byte length + the wire facts its `routeList` entry serves. One header (+ v2
    /// extension) read; `None` when the file doesn't parse as OBCR.
    ///
    /// The actively-open geometry is read **through its existing handle**: embedded-sdmmc refuses
    /// a second open (`FileAlreadyOpen`), which otherwise failed every mid-ride `routeList` build
    /// (the whole list errors on one unreadable slot, by design) and made `upload_finish`'s
    /// liveness re-check misread the open — very much present — file as gone (issue #480).
    pub fn route_object_info(&self, name: &ShortFileName) -> Option<(u32, RouteObjectInfo)> {
        let dir = self.routes_dir?;
        if let Some((i, f, len)) = self.open_route {
            if self.route_files.get(i) == Some(name) {
                let info = RouteObjectInfo::read(&SdByteSource::new(&self.vmgr, f, len)).ok()?;
                return Some((len, info));
            }
        }
        let file = self.vmgr.open_file_in_dir(dir, name, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0);
        let info = RouteObjectInfo::read(&SdByteSource::new(&self.vmgr, file, len)).ok();
        let _ = self.vmgr.close_file(file);
        Some((len, info?))
    }

    /// Whether a catalog file is an **aborted commit** — the held-back magic still zeroed
    /// because the commit's final patch never ran. Only that exact signature is sweepable; a
    /// merely unreadable file (a transient bus glitch) must be kept.
    pub fn is_aborted_commit(&self, name: &ShortFileName) -> bool {
        let Some(dir) = self.routes_dir else { return false };
        let Ok(file) = self.vmgr.open_file_in_dir(dir, name, Mode::ReadOnly) else {
            return false;
        };
        let mut magic = [0u8; 4];
        let zeroed = matches!(self.vmgr.read(file, &mut magic), Ok(4)) && magic == [0u8; 4];
        let _ = self.vmgr.close_file(file);
        zeroed
    }

    /// Release the active route's open geometry handle **if** it is `name`. embedded-sdmmc 0.9
    /// refuses to delete — or truncate-open, or re-open — a file with a live handle
    /// (`FileAlreadyOpen`), so every path about to delete or replace a route file must call this
    /// first: an idle-previewed route keeps its geometry open, and a phone-side delete/replace of
    /// the navigated route arrives with it open too. Dropping the handle here is always safe —
    /// the store-changed edge forces the ride loop's reconcile to re-derive and reopen
    /// (`prev_active = None`).
    fn close_route_if_open(&mut self, name: &ShortFileName) {
        if let Some((i, f, _)) = self.open_route {
            if self.route_files.get(i) == Some(name) {
                let _ = self.vmgr.close_file(f);
                self.open_route = None;
            }
        }
    }

    /// Delete a stored route file (the `deleteObject` command / a replace-upload's swap / the
    /// on-device hold-to-delete). Closes our own open geometry handle on it first — an open file
    /// can't be deleted (see [`close_route_if_open`](Self::close_route_if_open)).
    pub fn delete_route_file(&mut self, name: &ShortFileName) -> bool {
        let Some(dir) = self.routes_dir else { return false };
        self.close_route_if_open(name);
        match self.vmgr.delete_file_in_dir(dir, name) {
            Ok(()) => true,
            Err(e) => {
                defmt::warn!(
                    "SD: delete {} failed: {} — file kept, catalog unchanged",
                    defmt::Debug2Format(name),
                    defmt::Debug2Format(&e)
                );
                false
            }
        }
    }

    /// Delete a stored route by its **object id** — the map-only (non-`ble`) build's on-device
    /// hold-to-delete path (epic #447, P6). Resolves the id to its 8.3 filename through the
    /// scan-parallel [`route_ids`](Storage::route_ids)/[`route_files`](Storage::route_files) tables,
    /// then deletes the file. `true` = deleted; the caller re-scans the catalog. (The `ble` build
    /// routes deletes through the shared `ObjectStore` instead, keeping the wire revision coherent.)
    pub fn delete_route_by_id(&mut self, id: u16) -> bool {
        let Some(pos) = self.route_ids.iter().position(|&x| x == id) else { return false };
        let name = self.route_files[pos].clone();
        self.delete_route_file(&name)
    }

    /// Open (truncating) the upload temp for a fresh transfer, dropping any stale handle.
    pub fn upload_begin(&mut self) -> bool {
        self.upload_close();
        let Some(dir) = self.routes_dir_or_create() else { return false };
        match self.vmgr.open_file_in_dir(dir, UPLOAD_TMP, Mode::ReadWriteCreateOrTruncate) {
            Ok(file) => {
                self.open_upload = Some(file);
                true
            }
            Err(e) => {
                defmt::warn!("SD: cannot open upload temp: {}", defmt::Debug2Format(&e));
                false
            }
        }
    }

    /// Append CoC payload bytes to the open temp.
    pub fn upload_append(&mut self, bytes: &[u8]) -> bool {
        let Some(file) = self.open_upload else { return false };
        self.vmgr.write(file, bytes).is_ok()
    }

    /// Flush + close the temp handle, keeping the bytes on the card — the step [`upload_commit`]
    /// runs before it re-opens the temp to validate + promote it.
    pub fn upload_close(&mut self) {
        if let Some(file) = self.open_upload.take() {
            let _ = self.vmgr.flush_file(file);
            let _ = self.vmgr.close_file(file);
        }
    }

    /// Abort: close and delete the partial.
    pub fn upload_abort(&mut self) {
        if let Some(file) = self.open_upload.take() {
            let _ = self.vmgr.close_file(file);
        }
        if let Some(dir) = self.routes_dir {
            let _ = self.vmgr.delete_file_in_dir(dir, UPLOAD_TMP);
        }
    }

    /// Promote the CRC-verified temp into the catalog (see the section doc for the power-cut
    /// story). `replace` is the file the upload's object id already owns (deleted only *after* the
    /// temp validated — a failed CRC/validation never touches the old copy); `None` names the file
    /// `RT{fresh_id}.OBR`, so the object id is durable in the filename and a rescan after a reboot
    /// recovers it. Returns the final name + byte length + wire facts, or `None` with the temp deleted
    /// (invalid payload) or kept (transient copy failure).
    pub fn upload_commit(
        &mut self,
        replace: Option<&ShortFileName>,
        fresh_id: u16,
    ) -> Option<(ShortFileName, u32, RouteObjectInfo)> {
        self.upload_close();
        let dir = self.routes_dir?;

        // Validate: the temp must parse as OBCR (magic/version/header) — the transfer CRC only
        // proved the bytes match what the app sent, not that they are a route.
        let src_file = self.vmgr.open_file_in_dir(dir, UPLOAD_TMP, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(src_file).unwrap_or(0);
        let info = RouteObjectInfo::read(&SdByteSource::new(&self.vmgr, src_file, len)).ok();
        let Some(info) = info else {
            let _ = self.vmgr.close_file(src_file);
            let _ = self.vmgr.delete_file_in_dir(dir, UPLOAD_TMP);
            defmt::warn!("SD: upload is not a valid OBCR — rejected");
            return None;
        };

        // The final name: a replace reuses (and now frees) its object's file — the id stays in
        // the name; fresh encodes its assigned id (confirmed absent, the `name_is_free`
        // discipline: a bus glitch can never green-light overwriting a stored route).
        let final_name = match replace {
            Some(name) => {
                // A replace of the actively-navigated (or idle-previewed) route arrives with its
                // geometry handle open — release it first, or both this delete *and* the
                // truncate-open below are refused (`FileAlreadyOpen`) and the whole commit fails
                // with the old file intact but the upload lost (issue #480).
                self.close_route_if_open(name);
                if let Err(e) = self.vmgr.delete_file_in_dir(dir, name) {
                    defmt::warn!(
                        "SD: replace: cannot delete old {}: {}",
                        defmt::Debug2Format(name),
                        defmt::Debug2Format(&e)
                    );
                }
                name.clone()
            }
            None => match self.fresh_upload_name(dir, fresh_id) {
                Some(name) => name,
                None => {
                    let _ = self.vmgr.close_file(src_file);
                    defmt::warn!("SD: upload name RT{=u16}.OBR unavailable", fresh_id);
                    return None;
                }
            },
        };

        // Copy temp → final, magic held back; patch it in as the commit point.
        let copied = match self.vmgr.open_file_in_dir(dir, &final_name, Mode::ReadWriteCreateOrTruncate) {
            Ok(dst_file) => {
                let ok = self.copy_with_held_magic(src_file, dst_file, len);
                if !ok {
                    // On a replace the old file is already deleted — this is the destructive
                    // window (temp dropped below, old bytes gone): must be loud, never silent.
                    defmt::warn!("SD: upload copy failed — commit aborted (a replaced route's old file is gone)");
                }
                let _ = self.vmgr.close_file(dst_file);
                ok
            }
            Err(e) => {
                defmt::warn!("SD: cannot create {}: {}", defmt::Debug2Format(&final_name), defmt::Debug2Format(&e));
                false
            }
        };
        let _ = self.vmgr.close_file(src_file);
        if !copied {
            // The final file (if any) still has a zero magic — invisible to catalogs, reclaimed
            // by the boot sweep. Drop the temp too: a retry is a whole fresh upload.
            let _ = self.vmgr.delete_file_in_dir(dir, UPLOAD_TMP);
            return None;
        }
        let _ = self.vmgr.delete_file_in_dir(dir, UPLOAD_TMP);
        defmt::info!("SD: route committed → routes/{} ({=u32} B)", defmt::Debug2Format(&final_name), len);
        Some((final_name, len, info))
    }

    /// The commit's copy: `src[0..len]` → `dst` with bytes 0..4 (the OBCR magic) written as
    /// zeros, then — after the body is flushed — patched to the real magic and flushed again.
    /// True only when every step landed.
    fn copy_with_held_magic(&self, src: RawFile, dst: RawFile, len: u32) -> bool {
        if self.vmgr.file_seek_from_start(src, 0).is_err() {
            return false;
        }
        let mut magic = [0u8; 4];
        let mut buf = [0u8; 512];
        let mut off: u32 = 0;
        while off < len {
            let want = ((len - off) as usize).min(buf.len());
            let n = match self.vmgr.read(src, &mut buf[..want]) {
                Ok(n) if n > 0 => n,
                _ => return false,
            };
            if off == 0 {
                // A validated OBCR header is ≥ 112 B, so the magic sits inside the first read.
                magic.copy_from_slice(&buf[..4]);
                buf[..4].fill(0);
            }
            if self.vmgr.write(dst, &buf[..n]).is_err() {
                return false;
            }
            off += n as u32;
        }
        // Body durable (still invisible), then the one-write commit point.
        self.vmgr.flush_file(dst).is_ok()
            && self.vmgr.file_seek_from_start(dst, 0).is_ok()
            && self.vmgr.write(dst, &magic).is_ok()
            && self.vmgr.flush_file(dst).is_ok()
    }

    /// The confirmed-free `RT{id}.OBR` name for a fresh upload (`RT0`–`RT65535` all fit 8.3).
    /// Ids are assigned monotonically and never reused within a boot, and the rescan resumes
    /// past the highest stored one, so the name is expected absent — `None` (a foreign file
    /// squatting on it, or an unproven check) fails the commit rather than risk an overwrite.
    fn fresh_upload_name(&self, dir: RawDirectory, id: u16) -> Option<ShortFileName> {
        self.fresh_object_name(dir, "RT", id, "OBR")
    }

    /// A confirmed-free `{prefix}{id}.{ext}` 8.3 name for a durable-id object file — the shared
    /// discipline behind `RT{id}.OBR` uploads and `RD{id}.ORD` ride objects: only a proven-absent
    /// name (see [`name_is_free`](Self::name_is_free)) is handed back, so a squatting foreign
    /// file or an unproven check fails the save rather than risk an overwrite.
    fn fresh_object_name(&self, dir: RawDirectory, prefix: &str, id: u16, ext: &str) -> Option<ShortFileName> {
        let mut s: String<12> = String::new();
        let _ = core::fmt::write(&mut s, format_args!("{prefix}{id}.{ext}"));
        if self.name_is_free(dir, s.as_str()) {
            return ShortFileName::create_from_str(s.as_str()).ok();
        }
        None
    }

    /// Visit every stored ride object in `/tracks` (the `RD{id}.ORD` files) with its filename-encoded
    /// durable id. An in-progress `TRACK.OBT` (or any foreign file) never matches.
    pub fn for_each_ride_file(&self, mut f: impl FnMut(u16, &ShortFileName)) {
        let Some(dir) = self.tracks_dir else { return };
        self.iter_dir_lfn(dir, |e, _| {
            if let Some(id) = stored_ride_id(&e.name) {
                f(id, &e.name);
            }
        });
    }

    /// Whether a stored ride file is an **interrupted save** — the held-back version byte still
    /// zeroed because [`track_to_ride`]'s final patch never ran. Only that exact signature is
    /// sweepable (the ride-scan's analogue of [`is_aborted_commit`](Self::is_aborted_commit));
    /// a merely unreadable file must be kept.
    pub fn is_aborted_ride_object(&self, name: &ShortFileName) -> bool {
        let Some(dir) = self.tracks_dir else { return false };
        let Ok(file) = self.vmgr.open_file_in_dir(dir, name, Mode::ReadOnly) else {
            return false;
        };
        let mut version = [0xFFu8; 1];
        let zeroed = matches!(self.vmgr.read(file, &mut version), Ok(1)) && version[0] == 0;
        let _ = self.vmgr.close_file(file);
        zeroed
    }

    /// Delete a stored ride object file (the boot sweep of interrupted saves).
    pub fn delete_ride_file(&mut self, name: &ShortFileName) -> bool {
        let Some(dir) = self.tracks_dir else { return false };
        self.vmgr.delete_file_in_dir(dir, name).is_ok()
    }

    /// A stored ride object's byte length + the header facts its `rideList` entry serves. One header
    /// read; `None` when the file doesn't validate as a ride object v1 (incl. an interrupted save's
    /// held-back version byte — see [`track_to_ride`]).
    pub fn ride_object_info(&self, name: &ShortFileName) -> Option<(u32, RideInfo)> {
        let dir = self.tracks_dir?;
        let file = self.vmgr.open_file_in_dir(dir, name, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0);
        let info = RideInfo::read(&SdByteSource::new(&self.vmgr, file, len)).ok();
        let _ = self.vmgr.close_file(file);
        Some((len, info?))
    }

    /// Open a stored route for a detail download (the OBCR bytes verbatim), returning its byte
    /// length. Held in a slot separate from the ride's `open_route`.
    pub fn open_object(&mut self, name: &ShortFileName) -> Option<u32> {
        self.open_object_in(self.routes_dir, name)
    }

    /// Open a stored ride object for a download (the stored bytes *are* the wire object) — the
    /// `/tracks` twin of [`open_object`](Self::open_object), sharing the same handle slot (one
    /// transfer at a time).
    pub fn open_ride_object(&mut self, name: &ShortFileName) -> Option<u32> {
        self.open_object_in(self.tracks_dir, name)
    }

    fn open_object_in(&mut self, dir: Option<RawDirectory>, name: &ShortFileName) -> Option<u32> {
        self.close_object();
        let file = self.vmgr.open_file_in_dir(dir?, name, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0);
        self.open_object = Some((name.clone(), file, len));
        Some(len)
    }

    /// A [`ByteSource`](obc_route::ByteSource) over the open object — the CRC pre-pass and the
    /// chunked sends both read through it.
    pub fn object_source(&self) -> Option<Source<'_>> {
        self.open_object.as_ref().map(|(_, f, len)| SdByteSource::new(&self.vmgr, *f, *len))
    }

    /// Close the detail-download handle (transfer done, aborted, or superseded).
    pub fn close_object(&mut self) {
        if let Some((_, file, _)) = self.open_object.take() {
            let _ = self.vmgr.close_file(file);
        }
    }
}

/// Whether a `/routes` directory entry belongs to the route catalog: a side-loaded `.obcr`
/// (long-filename match) **or** a BLE-uploaded `*.OBR`. Uploads get plain 8.3
/// names because embedded-sdmmc creates short names only — the 4-char `.obcr` extension needs
/// an LFN it can't write — so the catalog accepts the dedicated 3-char twin. Dot-prefixed
/// clutter is excluded on both arms (an AppleDouble `._x.OBR` also fails the header read at
/// scan, but why open it at all).
fn is_route_entry(e: &embedded_sdmmc::DirEntry, long: Option<&str>) -> bool {
    if e.attributes.is_directory() {
        return false;
    }
    long_has_ext(long, b".obcr") || (e.name.extension() == b"OBR" && !long.is_some_and(|n| n.starts_with('.')))
}

/// The **durable object id** encoded in a BLE-uploaded route's filename — `RT{id}.OBR` → `id` (ids
/// are stable for the life of the stored object, across reboots — the phone persists the id an
/// upload commits under). `None` for anything else (side-loaded `.obcr` files carry no id and get a
/// session-scoped one from the reserved band — see `object_store`).
pub fn uploaded_route_id(name: &ShortFileName) -> Option<u16> {
    id_in_name(name, b"RT", b"OBR")
}

/// The **durable ride object id** in a stored ride's filename — `RD{id}.ORD` → `id`. The same
/// durability contract as the routes': the app's synced-set and tombstones key on these ids across
/// device reboots.
pub fn stored_ride_id(name: &ShortFileName) -> Option<u16> {
    id_in_name(name, b"RD", b"ORD")
}

/// Parse `{prefix}{decimal u16}.{ext}` from an 8.3 name; `None` for anything else.
fn id_in_name(name: &ShortFileName, prefix: &[u8], ext: &[u8]) -> Option<u16> {
    if name.extension() != ext {
        return None;
    }
    let digits = name.base_name().strip_prefix(prefix)?;
    if digits.is_empty() {
        return None;
    }
    let mut id: u32 = 0;
    for &b in digits {
        if !b.is_ascii_digit() {
            return None;
        }
        id = id * 10 + (b - b'0') as u32;
        if id > u16::MAX as u32 {
            return None;
        }
    }
    Some(id as u16)
}

/// Whether a directory entry's **long** name ends with `ext` (e.g. `b".obcr"`, case-
/// insensitive) and isn't a dot-prefixed file (macOS `._*` AppleDouble / `.DS_Store`). The long
/// name is required because the 8.3 short name can't represent a 4-char extension — both
/// `.obcr` and `.obcm` truncate to `OBC`. A `None` long name (a plain 8.3 file) never matches,
/// which is fine: every `.obcr`/`.obcm` forces a long-filename entry.
fn long_has_ext(long: Option<&str>, ext: &[u8]) -> bool {
    let Some(name) = long else { return false };
    let b = name.as_bytes();
    !b.starts_with(b".") && b.len() >= ext.len() && b[b.len() - ext.len()..].eq_ignore_ascii_case(ext)
}
