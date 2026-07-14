//! microSD storage for the nRF54L15 board: map / routes / track log over FatFs.
//!
//! This owns the concrete SPI bus → [`SdCard`] → [`VolumeManager`] stack and reconciles the FAT
//! filesystem to the shared app's *intent*, exactly as the simulator's `RouteStore`/`TrackStore`
//! reconcile a folder of files on the host. The reusable, board-agnostic adapters it hands the
//! format code live in [`obc_storage::sd`] ([`SdByteSource`]/[`SdByteSink`]/[`SdTrackSink`]);
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
    decode_route_crcs, decode_store_epoch, decode_synced_rides, encode_route_crcs, encode_store_epoch,
    encode_synced_rides, RouteCrcs, SyncedRides, TripInput, MAX_RIDES, MAX_ROUTES, MAX_TRIPS, ROUTE_CRCS_MAX_LEN,
    STORE_EPOCH_LEN, SYNCED_RIDES_MAX_LEN, UI_RIDES_CAP,
};
use obc_dfu::armer::{ExtentsError, ScanError, StageIo};
use obc_route::{
    ride_elevation_profile, ride_preview_polyline, track_to_ride, ByteSource, Profile, RideInfo, RideStats, RouteIndex,
    RouteObjectInfo, RouteSummary, TripMeta, TripSummary, NAME_CAP,
};
use obc_storage::fat_extents::{BuildError, ExtentSource, ExtentTable, SharedBlockDevice};
use obc_storage::{SdByteSink, SdByteSource, SdTrackSink};

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

/// The route-CRC sidecar in `/routes` (epic #632 item 6, V2): route object id → whole-object CRC-32
/// of the stored OBCR bytes, so the `routeList` entry can carry a content fingerprint the app verifies
/// linked-id badges against and adopts identical copies by. In `/routes` (not RRAM) so it survives a
/// reflash and travels with the card/routes; a BLE upload writes its entry at commit, a side-loaded /
/// pre-v2 route fills lazily at first list build. A missing/torn file = the empty map (every route
/// serves `0 = unknown`). Codec + torn-line semantics live in `obc-app::settings` (host-tested).
const ROUTE_CRCS: &str = "ROUTES.CRC";

/// The trip-CRC sidecar in `/routes` (epic #526 TR4, #653) — the trip twin of [`ROUTE_CRCS`]: trip
/// object id → whole-object CRC-32 of the stored `TP{id}.OBT` bytes, so a `tripList` entry carries a
/// content fingerprint the app verifies (a stage reorder changes neither `byte_len` nor `name`, only
/// the CRC). Same file-resident, survives-a-reflash, torn-line-is-empty-map contract as the route
/// sidecar; a BLE upload writes its entry at commit, a side-loaded trip fills lazily at first list
/// build. Reuses the [`RouteCrcs`] codec (a `u16 → u32` map — the trip-id namespace is disjoint from
/// the route-id one only *logically*; the sidecar is a separate file, so no key can collide).
const TRIP_CRCS: &str = "TRIPS.CRC";

/// The store-epoch nonce file in the **card root** (protocol v2 #632 item 5; card-resident #776):
/// the `u32` id-era name the phone reads over the pre-pairing `protocolVersion` read. Kept in the
/// card **root** (not `/routes` or `/tracks`) because the epoch names the *whole* store, not the
/// routes or rides specifically — so the SD card is the sole home of the id-era name: a card swap
/// transplants the store's identity (swap back restores the old era, a card written by a *different*
/// device presents *its* epoch — its own scope, closing the foreign-card hole the retired RRAM line
/// left open). Minted/rewritten only at boot by the mint pass; a missing/torn file reads as "no
/// epoch" → the mint rule draws a fresh one. Codec + torn-line semantics live in `obc-app::settings`
/// (host-tested), the direct analogue of the `SYNCED.SET` / `ROUTES.CRC` sidecars.
const EPOCH_FILE: &str = "EPOCH.OBE";

/// The in-flight BLE route upload, inside `/routes`. Its extension never matches the catalog scan,
/// so a partial upload — a drop, a power cut — is invisible until [`Storage::upload_commit`]
/// promotes it. Truncated-and-reused per upload.
const UPLOAD_TMP: &str = "UPLOAD.TMP";

/// The staged firmware update in the **card root** (epic #615, locked: 8.3-safe, no LFN — the
/// same file contract the future LM20 USB-MSC epic exposes). Sideloaded by the user (or, S6, the
/// phone); the armer only ever reads it.
const UPDATE_BIN: &str = "UPDATE.BIN";

/// The armer's snapshot of the **running** image (epic #615 S4, #619), in the card root next to
/// [`UPDATE_BIN`]: a full OBCU container (64-byte header + raw image read straight out of RRAM),
/// truncated-and-reused per arm like `TRACK.OBT`. The bootloader flashes it back if a trial boot
/// goes unconfirmed.
const ROLLBACK_BIN: &str = "ROLLBACK.BIN";

/// The reserved **computed-route** file (epic #116, R4): the on-device router's OBCR output,
/// overwritten on every plan. The 8.3 face of the spec'd `/routes/_nav.obcr` — embedded-sdmmc
/// creates short names only, and the 4-char `.obcr` extension needs an LFN it can't write, so the
/// device uses the `.OBR` twin the catalog scan already lists. No `RT` prefix ⇒ no durable
/// upload id; the scan hands it a session-scoped side-load id, exactly like a side-loaded `.obcr`.
const NAV_ROUTE_FILE: &str = "_NAV.OBR";

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
/// What the manager actually owns: the card **by shared reference** ([`SharedBlockDevice`]), so
/// the raw `&'static Sd` twin stays available for the map's extent-mapped direct block reads
/// (#500) — `VolumeManager::device()` can't hand it back out (its 0.9 signature can only return
/// the `TimeSource` type), so the share happens here, one level up. The card itself lives in
/// [`SD_CARD`].
type SdShared = SharedBlockDevice<'static, Sd>;
/// The open-handle budget (see the 6-file note above) — one set of consts so the manager and the
/// `obc-platform` wrapper aliases below can never drift apart.
const SD_MAX_DIRS: usize = 4;
const SD_MAX_FILES: usize = 6;
const SD_MAX_VOLUMES: usize = 1;
type Vmgr = VolumeManager<SdShared, NullTime, SD_MAX_DIRS, SD_MAX_FILES, SD_MAX_VOLUMES>;
/// [`SdByteSource`] over this board's manager (the wrappers are generic over the handle budget).
type Source<'a> = SdByteSource<'a, SdShared, NullTime, SD_MAX_DIRS, SD_MAX_FILES, SD_MAX_VOLUMES>;
/// [`SdByteSink`] over this board's manager — the router's OBCR emit writes through it.
type Sink<'a> = SdByteSink<'a, SdShared, NullTime, SD_MAX_DIRS, SD_MAX_FILES, SD_MAX_VOLUMES>;
/// [`SdTrackSink`] over this board's manager.
type TrackSinkT<'a> = SdTrackSink<'a, SdShared, NullTime, SD_MAX_DIRS, SD_MAX_FILES, SD_MAX_VOLUMES>;

/// The concrete [`SdCard`]'s home — a `.bss` slot written once by [`init`] (the warm-reset-safe
/// `init_static` pattern, see `main.rs`), so both the [`VolumeManager`] (via [`SdShared`]) and the
/// extent read path can borrow it for `'static`.
static mut SD_CARD: core::mem::MaybeUninit<Sd> = core::mem::MaybeUninit::uninit();

/// The map's resolved [`ExtentTable`]'s home (#500) — its own `.bss` slot rather than a field
/// *inside* [`Storage`], because `Storage` transits `main`'s async frame **by value** on its way
/// into the shared store, and an async frame allocates every local at entry (#270): carrying the
/// ~2 KB table inside `Storage` measurably cost the main-task future ~4 KB (two resident copies)
/// and the ride stack region shrank by the same RAM. `Storage` holds `Option<&'static _>`.
static mut MAP_EXTENTS: core::mem::MaybeUninit<ExtentTable> = core::mem::MaybeUninit::uninit();

/// What [`Storage::map_source`] hands out: extent-mapped direct block reads when the map's chain
/// resolved at open (#500), the manager's seek+read path otherwise. One enum rather than a trait
/// object so the render/nav paths stay monomorphic (no vtable on the per-chunk hot path).
pub enum MapSource<'a> {
    /// Direct block reads through the resolved [`ExtentTable`] — zero FAT traffic per read.
    Extent(ExtentSource<'a, Sd>),
    /// The plain seek path — correct on any card, O(offset) on backward seeks.
    Seek(Source<'a>),
}

impl ByteSource for MapSource<'_> {
    // `inline(never)`: reached from the deepest render/nav frames — keep the dispatch (and both
    // arms' machinery) out of those frames' locals, whatever the inliner decides later; a call
    // per multi-ms SD read is free. See the matching note on `ExtentSource::read_at`.
    #[inline(never)]
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), obc_route::Error> {
        match self {
            MapSource::Extent(s) => s.read_at(offset, buf),
            MapSource::Seek(s) => s.read_at(offset, buf),
        }
    }

    fn len(&self) -> u32 {
        match self {
            MapSource::Extent(s) => s.len(),
            MapSource::Seek(s) => s.len(),
        }
    }
}

/// FAT timestamps need a clock; the device has none yet (see [`obc_ports::TrackPoint::t_ms`]),
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
    /// The raw card the manager's [`SdShared`] borrows — the extent path's direct read handle.
    card: &'static Sd,
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
    /// Each scanned trip's **durable object id**, parallel to [`trip_metas`](Storage::trip_metas) and
    /// [`trip_files`](Storage::trip_files) — recovered from an uploaded `TP{id}.OBT` name, or a
    /// session-scoped side-load id for an id-less `.obt` file (epic #526 TR4). The app's trip folders
    /// (TR3) remap by this id across rescans.
    trip_ids: Vec<u16, MAX_TRIPS>,
    /// The 8.3 filename of each scanned trip, parallel to [`trip_ids`](Storage::trip_ids) — so a
    /// trip's durable id resolves back to its `TP{id}.OBT` file for a hold-to-delete cascade on the
    /// map-only build (`delete_trip_cascade_by_id`), the trip twin of
    /// [`ride_files`](Storage::ride_files).
    trip_files: Vec<ShortFileName, MAX_TRIPS>,
    /// The scanned trips' decoded metadata (name + stage route ids in ride order), fed to
    /// [`App::set_trips`](obc_app::App::set_trips) as [`TripInput`]s (borrowing these) so the app
    /// resolves each stage id against the live route catalog. Held resident (like the route/ride
    /// filename tables) so the `TripInput`s can borrow stable storage across the `set_trips` call.
    trip_metas: Vec<TripMeta, MAX_TRIPS>,
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
    /// RAM — `map_source` hands out a fresh source over it each redraw.
    open_map: Option<(RawFile, u32)>,
    /// The open map's FAT chain resolved to extent runs (#500): when present, `map_source` serves
    /// direct block reads (zero per-read FAT traffic) instead of the manager's O(offset) seek.
    /// `None` = build refused (fragmented past the cap / odd geometry) or failed verification —
    /// the seek path still works, just slowly, and open_map logged why. A reference into the
    /// [`MAP_EXTENTS`] `.bss` slot — see its doc for why the table must not live in here by value.
    map_extents: Option<&'static ExtentTable>,
    /// The open ride log for the current tracking session.
    open_track: Option<OpenTrack>,
    /// A finished ride whose log → ride-object conversion hasn't run yet. Finish only closes the
    /// log and stashes this; the ride loop runs [`run_pending_save`](Storage::run_pending_save)
    /// once the confirm animation has left the glass, so the save's blocking SD stretch never
    /// freezes the hold bulge (the "finishing a ride is laggy" bug).
    pending_save: Option<PendingSave>,
    /// A ride object landed on the card this pass ([`run_pending_save`](Storage::run_pending_save)
    /// committed an `RD{id}.ORD`) and the store edge hasn't been raised yet. Set by **every** save
    /// path (the quiet-glass deferred run *and* the back-to-back flush inside
    /// [`begin_track`](Storage::begin_track)/[`reconcile_track`](Storage::reconcile_track)), drained
    /// once per ride-loop pass via [`take_ride_saved`](Storage::take_ride_saved) — which is what
    /// makes a freshly-finished ride reach the Rides menu (and, on `ble`, the phone's catalog)
    /// without a reboot.
    ride_saved: bool,
    /// The BLE object plane's open route/ride file (a detail download in flight): `(filename,
    /// handle, length)`. A separate slot from `open_route` so a download can't disturb an active
    /// ride's geometry. The name is kept so the catalog scan can recognise (and read through)
    /// this handle instead of a second open — embedded-sdmmc refuses every second open of an
    /// open file (`FileAlreadyOpen`, even ReadOnly), which would silently drop the route from
    /// the catalog (issue #480).
    open_object: Option<(ShortFileName, RawFile, u32)>,
    /// The in-flight BLE upload's open [`UPLOAD_TMP`] handle.
    open_upload: Option<RawFile>,
    /// The loaded map's display name — its filename stem, captured in [`open_map`](Storage::open_map)
    /// (T8 item 6). Empty until a map opens; the System settings screen renders it (`grimsel · v10`)
    /// via [`App::set_map_info`](obc_app::App::set_map_info).
    map_name: String<24>,
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
    // Into its `.bss` slot before anything else: the manager and the extent read path both want
    // `'static` borrows of the one card.
    // SAFETY: sole writer of SD_CARD; `init` runs once per boot on the one thread-mode executor,
    // and a warm-reset re-run overwrites in place (no `Drop`), the `init_static` contract.
    let card: &'static Sd = unsafe { crate::init_static(core::ptr::addr_of_mut!(SD_CARD), SdCard::new(dev, Delay)) };
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
    fn mount(card: &'static Sd, cs: Output<'static>) -> Option<Storage> {
        // `new()` is pinned to the 4,4,1 defaults — the custom budget goes through `new_with_limits`
        // (5000 = the id offset `new()` itself uses).
        let vmgr: Vmgr = VolumeManager::new_with_limits(SharedBlockDevice(card), NullTime, 5000);
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
            card,
            root,
            routes_dir,
            tracks_dir,
            route_files: Vec::new(),
            route_ids: Vec::new(),
            ride_files: Vec::new(),
            ride_ids: Vec::new(),
            trip_ids: Vec::new(),
            trip_files: Vec::new(),
            trip_metas: Vec::new(),
            sideload_ids: Vec::new(),
            next_sideload: SIDELOAD_ID_BASE as u32,
            open_route: None,
            open_map: None,
            map_extents: None,
            open_track: None,
            pending_save: None,
            ride_saved: false,
            open_object: None,
            open_upload: None,
            map_name: String::new(),
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

    /// Scan `/routes` for the **trip catalog** (epic #526 TR4, #653): the uploaded `TP{id}.OBT` files
    /// and any side-loaded `.obt` files, decoding each into its [`TripMeta`] (name + stage route ids)
    /// held resident in [`trip_metas`](Storage::trip_metas), parallel to its durable/​session id in
    /// [`trip_ids`](Storage::trip_ids). Feed the app via [`trip_inputs`](Storage::trip_inputs) →
    /// [`App::set_trips`](obc_app::App::set_trips), which resolves each stage id against the route
    /// catalog. Called at boot and on every store-changed edge, next to [`scan_routes`](Storage::scan_routes).
    ///
    /// Trips live flat in `/routes` beside the routes (no subdirectories, spec §7.7). A trip whose
    /// stored `stage_count` exceeds the resident cap ([`obc_route::MAX_TRIP_STAGES`]) reads its first
    /// stages windowed (`TripMeta.truncated`) rather than overflowing. More than [`MAX_TRIPS`] trips →
    /// the first `MAX_TRIPS` are listed and the excess warned, mirroring the route-scan overflow.
    ///
    /// #480 open-handle lesson: a trip file this `Storage` already holds open (a detail download's
    /// `open_object`) is read **through** that handle — a second `open_file_in_dir` would answer
    /// `FileAlreadyOpen` and silently drop the trip from the catalog.
    pub fn scan_trips(&mut self) {
        self.trip_ids.clear();
        self.trip_files.clear();
        self.trip_metas.clear();
        let Some(dir) = self.routes_dir else { return };

        let mut names: Vec<ShortFileName, MAX_TRIPS> = Vec::new();
        let mut overflow = false;
        self.iter_dir_lfn(dir, |e, long| {
            if is_trip_entry(e, long) && names.push(e.name.clone()).is_err() {
                overflow = true;
            }
        });
        if overflow {
            defmt::warn!("SD: scan: more than {=usize} trip files — the excess is not listed", MAX_TRIPS);
        }

        for n in &names {
            // A trip without a resolvable id can't be listed (the app's folder remap keys on it) —
            // only the exhausted side-load band hits this, warned in `sideload_id`.
            let Some(id) = uploaded_trip_id(n).or_else(|| self.sideload_id(n)) else {
                defmt::warn!("SD: scan: trip {} has no object id — not listed", defmt::Debug2Format(n));
                continue;
            };
            // Open the file — or serve it through the download handle this `Storage` already holds.
            let (file, len, borrowed) = match self.vmgr.open_file_in_dir(dir, n, Mode::ReadOnly) {
                Ok(f) => (f, self.vmgr.file_length(f).unwrap_or(0), false),
                Err(e) => match &self.open_object {
                    Some((on, of, olen)) if on == n => (*of, *olen, true),
                    _ => {
                        defmt::warn!(
                            "SD: scan: cannot open trip {}: {} — not listed until the next rescan",
                            defmt::Debug2Format(n),
                            defmt::Debug2Format(&e)
                        );
                        continue;
                    }
                },
            };
            let meta = TripMeta::read(&SdByteSource::new(&self.vmgr, file, len));
            if !borrowed {
                let _ = self.vmgr.close_file(file);
            }
            match meta {
                Ok(m) => {
                    if self.trip_metas.push(m).is_ok() {
                        let _ = self.trip_ids.push(id);
                        let _ = self.trip_files.push(n.clone());
                    }
                }
                Err(_) => defmt::warn!("SD: scan: trip {} unreadable — not listed", defmt::Debug2Format(n)),
            }
        }
        defmt::info!("SD: {=usize} trip(s) in /routes", self.trip_metas.len());
    }

    /// The scanned trips as [`TripInput`]s for [`App::set_trips`](obc_app::App::set_trips) — each
    /// borrows its resident [`TripMeta`] (name + stage ids), so the returned vec borrows `self` and
    /// must outlive the `set_trips` call. Run [`scan_trips`](Storage::scan_trips) first.
    pub fn trip_inputs(&self) -> Vec<TripInput<'_>, MAX_TRIPS> {
        let mut out = Vec::new();
        for (id, meta) in self.trip_ids.iter().zip(self.trip_metas.iter()) {
            let _ = out.push(TripInput { id: *id, name: meta.name.as_str(), stage_ids: &meta.stage_ids });
        }
        out
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
        self.mark_rides_synced(core::iter::once(id)) > 0
    }

    /// Record a batch of ride ids as synced in **one** sidecar read-modify-write (the `ackRides`
    /// command can carry dozens of ids — a per-id rewrite would be that many file round-trips).
    /// Returns how many ids were **newly** flagged; `0` = the sidecar was not rewritten. Ids
    /// already flagged (or dropped by a full set) count as nothing-new.
    pub fn mark_rides_synced(&mut self, ids: impl Iterator<Item = u16>) -> usize {
        let mut set = self.load_synced_set();
        let added = ids.filter(|&id| set.insert(id)).count();
        if added > 0 {
            self.write_synced_set(&set);
        }
        added
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

    /// Read the route-CRC sidecar (`/routes/ROUTES.CRC`) into a [`RouteCrcs`] map (epic #632 item 6).
    /// A missing, torn, or malformed sidecar decodes to the **empty** map (every route serves
    /// `0 = unknown`) — never a panic (the codec + torn-line semantics are host-tested in
    /// `obc-app::settings`). One file read.
    pub fn load_route_crcs(&self) -> RouteCrcs {
        let Some(dir) = self.routes_dir else { return RouteCrcs::new() };
        let Ok(file) = self.vmgr.open_file_in_dir(dir, ROUTE_CRCS, Mode::ReadOnly) else {
            return RouteCrcs::new(); // absent = no CRC known
        };
        let mut buf = [0u8; ROUTE_CRCS_MAX_LEN];
        let n = self.vmgr.read(file, &mut buf).unwrap_or(0);
        let _ = self.vmgr.close_file(file);
        decode_route_crcs(&buf[..n])
    }

    /// Upsert route `id`'s whole-object CRC into the sidecar, persisting only when it actually
    /// changed (a re-upload with the same content rewrites nothing). Called at an upload commit —
    /// the CRC is already verified there. Read-modify-write within the call (open, write truncating,
    /// close), so it never counts against the open-file budget across an `await`.
    pub fn set_route_crc(&mut self, id: u16, crc: u32) {
        let mut map = self.load_route_crcs();
        if map.insert(id, crc) {
            self.write_route_crcs(&map);
        }
    }

    /// Retire route `id`'s CRC entry from the sidecar (a deleted route — ids never reuse, so this is
    /// belt-and-braces tidiness). Rewrites only when the entry was present.
    pub fn forget_route_crc(&mut self, id: u16) {
        let mut map = self.load_route_crcs();
        if map.remove(id) {
            self.write_route_crcs(&map);
        }
    }

    /// Overwrite the route-CRC sidecar (truncating). A write failure is warned, not fatal — the
    /// worst case is a route serves `0 = unknown` and re-fills lazily next list build, never a crash.
    pub fn write_route_crcs(&mut self, map: &RouteCrcs) {
        let Some(dir) = self.routes_dir else { return };
        let mut buf = [0u8; ROUTE_CRCS_MAX_LEN];
        let n = encode_route_crcs(map, &mut buf);
        match self.vmgr.open_file_in_dir(dir, ROUTE_CRCS, Mode::ReadWriteCreateOrTruncate) {
            Ok(file) => {
                if self.vmgr.write(file, &buf[..n]).is_err() {
                    defmt::warn!("SD: route-crc sidecar write failed — a route may serve crc 0 next list build");
                }
                let _ = self.vmgr.flush_file(file);
                let _ = self.vmgr.close_file(file);
            }
            Err(e) => defmt::warn!("SD: cannot open route-crc sidecar: {}", defmt::Debug2Format(&e)),
        }
    }

    /// Read the card-resident store-epoch nonce (`/EPOCH.OBE`, protocol v2 #632 item 5 / #776), or
    /// `None` when the file is **absent** (a fresh/foreign-formatted card) or torn/foreign — "no
    /// epoch", which the boot mint rule ([`obc_app::settings::store_epoch_mint`]) treats as clause 1
    /// (draw a fresh nonce). Never panics on malformed input (the codec is host-tested). One file
    /// read; the card **root** is always open on a mounted card.
    pub fn load_card_epoch(&self) -> Option<u32> {
        let Ok(file) = self.vmgr.open_file_in_dir(self.root, EPOCH_FILE, Mode::ReadOnly) else {
            return None; // absent = no epoch (the mint pass draws a fresh one)
        };
        let mut buf = [0u8; STORE_EPOCH_LEN];
        let n = self.vmgr.read(file, &mut buf).unwrap_or(0);
        let _ = self.vmgr.close_file(file);
        decode_store_epoch(&buf[..n])
    }

    /// Overwrite the store-epoch file (truncating) with `epoch`. Called once, from the boot mint
    /// pass, when the mint rule fires — so the write rate is negligible. Returns `true` only when
    /// **every** step — open, write, flush, close — succeeded: a discarded flush/close error is a
    /// torn persist, and the mint pass gates the id-marks write and the served epoch on this result
    /// (unlike the other sidecar writers, whose failure direction is safe by design, a swallowed
    /// epoch-write failure would let a clause-2 mint go permanently undetected: old valid epoch on
    /// card + freshly-written valid floor = steady state next boot — the exact aliasing the epoch
    /// exists to catch). Whole persist within the call (open, write truncating, flush, close), so it
    /// never counts against the open-file budget across an `await`.
    #[must_use]
    pub fn save_card_epoch(&mut self, epoch: u32) -> bool {
        let bytes = encode_store_epoch(epoch);
        let file = match self.vmgr.open_file_in_dir(self.root, EPOCH_FILE, Mode::ReadWriteCreateOrTruncate) {
            Ok(file) => file,
            Err(e) => {
                defmt::warn!("SD: cannot open store-epoch file: {}", defmt::Debug2Format(&e));
                return false;
            }
        };
        let wrote = self.vmgr.write(file, &bytes).is_ok();
        let flushed = self.vmgr.flush_file(file).is_ok();
        let closed = self.vmgr.close_file(file).is_ok();
        let ok = wrote && flushed && closed;
        if !ok {
            // The consequence log lives at the mint site (which knows whether this was a clause-2
            // mint); here just name which step tore.
            defmt::warn!(
                "SD: store-epoch persist failed (write {=bool} flush {=bool} close {=bool})",
                wrote,
                flushed,
                closed
            );
        }
        ok
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

    /// Build the stored ride `id`'s recorded-track elevation [`Profile`] — the Ride detail's band
    /// fill (epic #678 T2 / #680), answering
    /// [`App::take_ride_track_request`](obc_app::App::take_ride_track_request). Resolves the id
    /// through the scan-parallel [`ride_ids`](Storage::ride_ids)/[`ride_files`](Storage::ride_files)
    /// tables and streams the `RD{id}.ORD` once through the shared `ride_elevation_profile`
    /// (~448 B per SD read, no whole-track buffer — the ~36 KB stack budget's discipline; the
    /// returned `Profile` is the nrf-mem ~3 KB build). An in-flight BLE download's open handle is
    /// read through rather than re-opened (embedded-sdmmc refuses a second open, #480), exactly as
    /// [`scan_rides`](Storage::scan_rides) does. `None` = unknown id / unopenable / torn file —
    /// the caller parks the failure so the read isn't ground against every pass.
    pub fn ride_profile_by_id(&mut self, id: u16) -> Option<Profile> {
        let pos = self.ride_ids.iter().position(|&x| x == id)?;
        let name = self.ride_files[pos].clone();
        let dir = self.tracks_dir?;
        let (file, len, borrowed) = match self.vmgr.open_file_in_dir(dir, &name, Mode::ReadOnly) {
            Ok(f) => (f, self.vmgr.file_length(f).unwrap_or(0), false),
            Err(_) => match &self.open_object {
                Some((on, of, olen)) if *on == name => (*of, *olen, true),
                _ => {
                    defmt::warn!("SD: ride profile: cannot open {} — band stays empty", defmt::Debug2Format(&name));
                    return None;
                }
            },
        };
        let profile = ride_elevation_profile(&SdByteSource::new(&self.vmgr, file, len)).ok();
        if !borrowed {
            let _ = self.vmgr.close_file(file);
        }
        profile
    }

    /// Build the stored ride `id`'s decimated recorded-track shape polyline (#678 rework 3) —
    /// the preview half of the Ride detail's track-request answer, `ride_profile_by_id`'s twin:
    /// the same id resolution, the same open-or-borrow handle discipline (#480), one forward
    /// streaming pass through the shared `ride_preview_polyline` (~448 B blocks, no whole-track
    /// buffer, no backward seeks — the #502 FAT lesson). Empty = unknown id / unopenable / torn
    /// file — the detail's track page just leaves its slot blank.
    pub fn ride_preview_by_id(&mut self, id: u16) -> heapless::Vec<(i32, i32), { obc_app::NAV_PREVIEW_MAX }> {
        let Some(pos) = self.ride_ids.iter().position(|&x| x == id) else { return heapless::Vec::new() };
        let name = self.ride_files[pos].clone();
        let Some(dir) = self.tracks_dir else { return heapless::Vec::new() };
        let (file, len, borrowed) = match self.vmgr.open_file_in_dir(dir, &name, Mode::ReadOnly) {
            Ok(f) => (f, self.vmgr.file_length(f).unwrap_or(0), false),
            Err(_) => match &self.open_object {
                Some((on, of, olen)) if *on == name => (*of, *olen, true),
                _ => {
                    defmt::warn!(
                        "SD: ride preview: cannot open {} — track page stays empty",
                        defmt::Debug2Format(&name)
                    );
                    return heapless::Vec::new();
                }
            },
        };
        let pts = ride_preview_polyline(&SdByteSource::new(&self.vmgr, file, len)).unwrap_or_default();
        if !borrowed {
            let _ = self.vmgr.close_file(file);
        }
        pts
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
        // The name to open, plus the directory-entry location the extent build reads the first
        // cluster from (public `DirEntry` facts, captured in the same scan). The long name's stem is
        // captured too for the System screen's `Map` row (T8 item 6).
        let mut found: Option<(ShortFileName, embedded_sdmmc::BlockIdx, u32)> = None;
        let mut long_stem: String<24> = String::new();
        self.iter_dir_lfn(self.root, |e, long| {
            if found.is_none() && !e.attributes.is_directory() && long_has_ext(long, b".obcm") {
                found = Some((e.name.clone(), e.entry_block, e.entry_offset));
                if let Some(long) = long {
                    long_stem.clear();
                    // The stem before the `.obcm` extension (case-insensitive), truncated to the cap.
                    let stem = long.rsplit_once('.').map(|(s, _)| s).unwrap_or(long);
                    for ch in stem.chars() {
                        if long_stem.push(ch).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let (name, entry_block, entry_offset) = found?;
        // Fall back to the 8.3 short name's base (trailing padding trimmed) if there was no LFN.
        if long_stem.is_empty() {
            for &b in name.base_name().iter().take_while(|&&b| b != b' ') {
                if long_stem.push(b as char).is_err() {
                    break;
                }
            }
        }
        self.map_name = long_stem;
        let file = self.vmgr.open_file_in_dir(self.root, &name, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0);
        if len == 0 {
            let _ = self.vmgr.close_file(file);
            return None;
        }
        self.open_map = Some((file, len));
        self.build_map_extents(entry_block, entry_offset, file, len);
        Some(len)
    }

    /// The loaded map's display name (T8 item 6) — its filename stem, or `""` before a map opens.
    pub fn map_name(&self) -> &str {
        self.map_name.as_str()
    }

    /// Free space on the SD card in bytes (T8 item 6) — a bounded **FAT free-cluster** read: the
    /// FAT32 FSInfo sector's cached free-cluster count × cluster size. Three single-block CMD17s (the
    /// MBR partition entry, the volume BPB, then FSInfo) — never a full FAT walk, so it's cheap enough
    /// to run on the System screen's on-entry request. Returns `None` unless the card is MBR + FAT32
    /// with a valid FSInfo free count (the screen then keeps `--`).
    pub fn card_free_bytes(&self) -> Option<u64> {
        use embedded_sdmmc::{Block, BlockDevice, BlockIdx};
        let read = |lba: u32, blk: &mut Block| self.card.read(core::slice::from_mut(blk), BlockIdx(lba)).ok();
        let rd_u16 = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let rd_u32 = |b: &[u8], o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);

        let mut blk = Block::new();
        // Sector 0: an MBR (partition 0 start LBA at +8 of the 446-offset entry) or, on a
        // "superfloppy" (a BPB directly at LBA 0), the boot sector itself — detected by the FAT jump
        // + "FAT32" type string, in which case the partition starts at LBA 0.
        read(0, &mut blk)?;
        let superfloppy = (blk.contents[0] == 0xEB || blk.contents[0] == 0xE9) && &blk.contents[82..87] == b"FAT32";
        let part_lba = if superfloppy { 0 } else { rd_u32(&blk.contents, 446 + 8) };

        // The volume BPB.
        read(part_lba, &mut blk)?;
        let bytes_per_sec = rd_u16(&blk.contents, 11) as u64;
        let sec_per_clus = blk.contents[13] as u64;
        let fsinfo_sec = rd_u16(&blk.contents, 48) as u32;
        if bytes_per_sec == 0 || sec_per_clus == 0 || fsinfo_sec == 0 {
            return None;
        }

        // The FSInfo sector — the FAT's cached free-cluster count (lead sig 0x41615252 @0, struct sig
        // 0x61417272 @484). `0xFFFFFFFF` = unknown / uncomputed.
        read(part_lba + fsinfo_sec, &mut blk)?;
        if rd_u32(&blk.contents, 0) != 0x4161_5252 || rd_u32(&blk.contents, 484) != 0x6141_7272 {
            return None;
        }
        let free_clusters = rd_u32(&blk.contents, 488);
        if free_clusters == 0xFFFF_FFFF {
            return None;
        }
        Some(free_clusters as u64 * sec_per_clus * bytes_per_sec)
    }

    /// Resolve the just-opened map's FAT chain into [`map_extents`](Storage::map_extents) — the
    /// one-time walk that makes every later map read a direct block read (#500). Any refusal
    /// (fragmented past the cap, unexpected geometry, failed verification) leaves `None`: the
    /// manager's seek path still serves the map, just at the old speed, and the log says why.
    /// The logged extent count is also #500's fragmentation measurement — 1 = contiguous.
    fn build_map_extents(&mut self, entry_block: embedded_sdmmc::BlockIdx, entry_offset: u32, file: RawFile, len: u32) {
        self.map_extents = None;
        match ExtentTable::build(self.card, entry_block, entry_offset, len) {
            Ok(table) => {
                // Into the `.bss` slot before it can be captured anywhere by value (see
                // `MAP_EXTENTS`). SAFETY: sole writer, same once-per-boot discipline as `SD_CARD`
                // (a re-open overwrites in place; no `Drop`), the `init_static` contract.
                let table: &'static ExtentTable =
                    unsafe { crate::init_static(core::ptr::addr_of_mut!(MAP_EXTENTS), table) };
                if self.verify_extents(table, file, len) {
                    defmt::info!(
                        "SD: map is {=usize} extent(s) over {=u32} bytes — direct block reads on",
                        table.extent_count(),
                        len
                    );
                    self.map_extents = Some(table);
                } else {
                    defmt::warn!("SD: map extent table failed verification — keeping the FAT-seek read path");
                }
            }
            Err(e) => defmt::warn!(
                "SD: map extent table unavailable ({}) — keeping the FAT-seek read path",
                defmt::Debug2Format(&e)
            ),
        }
    }

    /// Cross-check a fresh extent table against the manager's own read of the same file: a small
    /// window at the head and at the tail through **both** paths must agree byte-for-byte. Cheap
    /// (one-time, four short reads), and it turns any geometry slip into a loud fallback instead
    /// of wrong map bytes.
    fn verify_extents(&self, table: &ExtentTable, file: RawFile, len: u32) -> bool {
        let slow = Source::new(&self.vmgr, file, len);
        let fast = ExtentSource::new(self.card, table);
        let mut a = [0u8; 64];
        let mut b = [0u8; 64];
        for off in [0, len.saturating_sub(a.len() as u32)] {
            let n = a.len().min(len as usize);
            if slow.read_at(off, &mut a[..n]).is_err() || fast.read_at(off, &mut b[..n]).is_err() || a[..n] != b[..n] {
                return false;
            }
        }
        true
    }

    /// A [`ByteSource`](obc_route::ByteSource) over the open map file, for reading the header
    /// ([`obc_reader::read_header`]) or building a per-frame [`Reader`](obc_reader::Reader). `None` if
    /// no map was opened ([`open_map`](Self::open_map) returned `None`). Cheap — the source just wraps
    /// the already-open handle, so it's rebuilt every redraw, keeping no borrow across the `&mut self`
    /// route/track operations. Extent-mapped direct block reads when the table built (#500), the
    /// manager's seek path otherwise.
    pub fn map_source(&self) -> Option<MapSource<'_>> {
        let (f, len) = self.open_map?;
        Some(match self.map_extents {
            Some(table) => MapSource::Extent(ExtentSource::new(self.card, table)),
            None => MapSource::Seek(SdByteSource::new(&self.vmgr, f, len)),
        })
    }

    /// Whether the open map reads through the **slow FAT-seek path** — [`build_map_extents`] refused
    /// the extent table (fragmented past the cap or failed verification), so every backward seek is
    /// O(offset) again (#500/#504). `false` with no map open and for a contiguous map (direct block
    /// reads on). Surfaced on glass as a dismissable "map reads are slow — re-copy the card" warning:
    /// it needs ~3× the reference card's fragmentation to trip, but when it does the rider gets an
    /// actionable one-liner instead of a device that just went sluggish.
    pub fn map_degraded(&self) -> bool {
        self.open_map.is_some() && self.map_extents.is_none()
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
    /// re-walking the whole index off the card at panel rate. Returns whether `idx` now holds a valid
    /// index; `false` (slot cleared) when no route is open or the read fails (a flaky link) — the loop
    /// retries the build on a later redraw, so a transient glitch doesn't hide the route.
    ///
    /// **In place** into the caller's resident slot (`RouteIndex::read_into`), never by value: the
    /// ~6.7 KB index returned through `Option<RouteIndex>` rode the stack at the ride pass's deepest
    /// point, and the post-upload rescan's rebuild overflowed the 44 KB main stack the moment `.bss`
    /// crept 216 B (STKOF HardFault inside `RouteIndex::read`, 2026-07-12).
    pub fn build_route_index_into(&self, idx: &mut RouteIndex) -> bool {
        let Some(src) = self.route_source() else { return false };
        idx.read_into(&src).is_ok()
    }

    /// The mini elevation sparkline for the route with object id `id` (#682): open its `.obcr` on a
    /// scoped handle, stream it once through [`obc_route::elevation_sparkline`], and close it — the
    /// board side of the route-upload seam, built at commit time so the idle "ROUTE RECEIVED" card
    /// can draw the band. `None` when the id is unknown, the file won't open (e.g. it's held by the
    /// active geometry — that route replaces navigation and never draws the band anyway), or the
    /// route carries no elevation. Cheap and one-shot: called once per upload, off the render path.
    pub fn route_elevation_sparkline(&mut self, id: u16) -> Option<[u8; obc_route::SPARKLINE_BUCKETS]> {
        let dir = self.routes_dir?;
        let pos = self.route_ids.iter().position(|&x| x == id)?;
        let name = self.route_files.get(pos)?.clone();
        let file = self.vmgr.open_file_in_dir(dir, name, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0);
        let spark = obc_route::elevation_sparkline(&SdByteSource::new(&self.vmgr, file, len));
        let _ = self.vmgr.close_file(file);
        spark
    }

    /// Open (truncating) the reserved computed-route file `/routes/_NAV.OBR` for the router's
    /// OBCR emit (epic #116, R4). Releases every handle this `Storage` may hold **on that file**
    /// first — the reserved route can be the actively-previewed/ridden route (its geometry open)
    /// or mid-detail-download — because embedded-sdmmc refuses a truncate-open of an open file
    /// (`FileAlreadyOpen`). The ride loop re-derives + reopens geometry after the plan (it forces
    /// its reconcile), so dropping the handles here is always safe. `None` = no card / no dir /
    /// open failure — the caller degrades to the generic routing-failure tier.
    pub fn nav_route_begin(&mut self) -> Option<RawFile> {
        // Close the active geometry unconditionally (cheap; the loop reopens off the fresh scan) —
        // and a detail download parked on the nav file, if any.
        self.reconcile_route(None);
        if let Ok(nav) = ShortFileName::create_from_str(NAV_ROUTE_FILE) {
            if matches!(&self.open_object, Some((on, ..)) if *on == nav) {
                self.close_object();
            }
        }
        let dir = self.routes_dir_or_create()?;
        self.vmgr.open_file_in_dir(dir, NAV_ROUTE_FILE, Mode::ReadWriteCreateOrTruncate).ok()
    }

    /// A [`ByteSink`](obc_route::ByteSink) over the open nav-route file — what
    /// [`plan_route`](obc_route::plan_route) streams the emitted OBCR through.
    pub fn nav_sink(&self, file: RawFile) -> Sink<'_> {
        SdByteSink::new(&self.vmgr, file)
    }

    /// Flush + close the nav-route file after the plan. On failure (`ok == false`) the partial
    /// file is deleted — a torn emit must not linger where the catalog scan would list it as an
    /// unreadable route (the reserved name is rewritten on every plan anyway).
    pub fn nav_route_finish(&mut self, file: RawFile, ok: bool) {
        let _ = self.vmgr.flush_file(file);
        let _ = self.vmgr.close_file(file);
        if !ok {
            if let Some(dir) = self.routes_dir {
                let _ = self.vmgr.delete_file_in_dir(dir, NAV_ROUTE_FILE);
            }
        }
    }

    /// The committed nav route's object id, resolved against the tables the **last catalog scan**
    /// filled — call after the post-plan [`scan_routes`](Storage::scan_routes). `None` when the
    /// reserved file isn't in the catalog (the emit failed, or the scan couldn't read it).
    pub fn nav_route_id(&self) -> Option<u16> {
        let nav = ShortFileName::create_from_str(NAV_ROUTE_FILE).ok()?;
        let pos = self.route_files.iter().position(|n| *n == nav)?;
        self.route_ids.get(pos).copied()
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

    /// The [`TrackSink`](obc_ports::TrackSink) for the open log, or `None` when not recording.
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
            // Raise the saved-ride flag for the ride loop's per-pass drain (`take_ride_saved`) —
            // the edge that gets the fresh `RD{id}.ORD` into the Rides menu (and the phone's
            // catalog) without a reboot. Set here, at the single commit point, so every caller
            // (deferred run and back-to-back flush alike) raises it.
            self.ride_saved = true;
        }
    }

    /// Drain the saved-ride flag: `true` exactly once after [`run_pending_save`] committed a ride
    /// object. The ride loop checks this once per pass and raises the store edge from it — on `ble`
    /// by posting [`crate::object_store::note_ride_saved`] (the BLE plane re-scans its catalog and
    /// bumps the revision, so the phone's `storeChanged`/digest and the Rides menu learn from the
    /// same edge), map-only by re-feeding the Rides menu directly.
    pub fn take_ride_saved(&mut self) -> bool {
        core::mem::take(&mut self.ride_saved)
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

    /// Promote the CRC-verified upload temp to `/UPDATE.BIN` in the card **root** — the `fwImage`
    /// commit target (epic #615 S6, #621). Parametrizes the route promote's atomic story onto the DFU
    /// staging file rather than forking it: the same held-back-magic copy
    /// ([`copy_with_held_magic`](Self::copy_with_held_magic)) — here the OBCU magic (`b"OBCU"`, bytes
    /// `0..4`) — so a torn copy leaves a zero-magic `UPDATE.BIN` the armer's scan rejects on a bad
    /// header, exactly as a torn route's zero-magic OBCR is rejected. A commit **overwrites** any
    /// existing `UPDATE.BIN` (deleted first). The OBCU header is validated (a cheap 64-byte decode) —
    /// the transfer CRC only proved the bytes are what the app sent, not that they are an update image
    /// (the route path's OBCR-parse analogue); the armer re-runs the full CRC scan before erasing the
    /// slot. Returns the promoted byte length, or `None` with the temp dropped (invalid container /
    /// torn copy — a retry is a whole fresh upload). The temp lives in `/routes` like a route upload;
    /// only the promote target differs.
    pub fn commit_fwimage(&mut self) -> Option<u32> {
        self.upload_close();
        let dir = self.routes_dir?; // UPLOAD.TMP sinks into /routes (route-upload path, unchanged)

        // Validate: the temp must decode as an OBCU container header. The transfer CRC only proved the
        // bytes match what the app sent — this rejects a well-CRC'd but non-update payload cheaply.
        let src_file = self.vmgr.open_file_in_dir(dir, UPLOAD_TMP, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(src_file).unwrap_or(0);
        let mut header = [0u8; obc_dfu::HEADER_LEN];
        let valid = matches!(self.vmgr.read(src_file, &mut header), Ok(n) if n == obc_dfu::HEADER_LEN)
            && obc_dfu::ImageHeader::decode(&header).is_some();
        if !valid {
            let _ = self.vmgr.close_file(src_file);
            let _ = self.vmgr.delete_file_in_dir(dir, UPLOAD_TMP);
            defmt::warn!("SD: fwImage upload is not a valid OBCU container — rejected");
            return None;
        }

        // Overwrite semantics: drop any existing UPDATE.BIN before promoting the new stage.
        let Ok(update_name) = ShortFileName::create_from_str(UPDATE_BIN) else {
            let _ = self.vmgr.close_file(src_file);
            let _ = self.vmgr.delete_file_in_dir(dir, UPLOAD_TMP);
            return None;
        };
        let _ = self.vmgr.delete_file_in_dir(self.root, &update_name);

        // Copy temp → /UPDATE.BIN, OBCU magic held back until the body is durable (the commit point).
        let copied = match self.vmgr.open_file_in_dir(self.root, UPDATE_BIN, Mode::ReadWriteCreateOrTruncate) {
            Ok(dst_file) => {
                let ok = self.copy_with_held_magic(src_file, dst_file, len);
                if !ok {
                    defmt::warn!("SD: fwImage copy failed — /UPDATE.BIN left zero-magic (inert; armer rejects)");
                }
                let _ = self.vmgr.close_file(dst_file);
                ok
            }
            Err(e) => {
                defmt::warn!("SD: cannot create /UPDATE.BIN: {}", defmt::Debug2Format(&e));
                false
            }
        };
        let _ = self.vmgr.close_file(src_file);
        let _ = self.vmgr.delete_file_in_dir(dir, UPLOAD_TMP);
        if !copied {
            // A torn/failed copy left a zero-magic (invisible-to-the-armer) UPDATE.BIN; a retry is a
            // whole fresh upload — exactly the route path's torn-commit story.
            return None;
        }
        defmt::info!("SD: fwImage committed → /UPDATE.BIN ({=u32} B)", len);
        Some(len)
    }

    /// Whether a staged `/UPDATE.BIN` exists in the card root — the `installFw` `noStaged` cheap
    /// existence check (spec §4.4). Presence only (a directory scan, no read): the full CRC validation
    /// is the on-device confirm flow's, never a BLE command handler's.
    pub fn has_update_bin(&self) -> bool {
        ShortFileName::create_from_str(UPDATE_BIN).ok().and_then(|n| self.find_root_entry(&n)).is_some()
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

    // ==================== trips (epic #526 TR4, #653) ====================
    //
    // Trips store flat in `/routes` as `TP{id}.OBT` beside `RT{id}.OBR` (spec §7.7), and ride the same
    // atomic-commit machinery as routes: the upload sinks into `UPLOAD.TMP` (only one transfer at a
    // time, so the temp is shared) and `upload_commit_trip` copies it to its final name with the first
    // four bytes (version + reserved + stage_count) **held back as zeros** via `copy_with_held_magic`
    // — a torn commit leaves `version = 0`, which `TripSummary::read` rejects (`BadVersion`) and
    // `is_aborted_commit` (first-4-bytes-zero) sweeps, exactly as a torn route's zeroed OBCR magic is.

    /// Visit every trip file in `/routes` — the uploaded `TP{id}.OBT` files and any side-loaded `.obt`.
    pub fn for_each_trip_file(&self, mut f: impl FnMut(&ShortFileName)) {
        let Some(dir) = self.routes_dir else { return };
        self.iter_dir_lfn(dir, |e, long| {
            if is_trip_entry(e, long) {
                f(&e.name);
            }
        });
    }

    /// Read a stored trip object: its byte length, decoded [`TripMeta`] (name + windowed stage ids),
    /// and the **true** stored `stage_count` (from the header, even when it exceeds the resident stage
    /// cap). One open, two tiny header reads. `None` when the file doesn't validate as a trip object v1
    /// (incl. a torn commit's held-back zero version). The `tripList` build and the wire-catalog rescan
    /// both read through this. #480: the actively-open download handle (`open_object`) is read through
    /// rather than re-opened.
    pub fn read_trip(&self, name: &ShortFileName) -> Option<(u32, TripMeta, u16)> {
        let dir = self.routes_dir?;
        fn read(src: &Source<'_>, len: u32) -> Option<(u32, TripMeta, u16)> {
            let meta = TripMeta::read(src).ok()?;
            let summary = TripSummary::read(src).ok()?;
            Some((len, meta, summary.stage_count))
        }
        if let Some((on, of, olen)) = &self.open_object {
            if on == name {
                return read(&SdByteSource::new(&self.vmgr, *of, *olen), *olen);
            }
        }
        let file = self.vmgr.open_file_in_dir(dir, name, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0);
        let out = read(&SdByteSource::new(&self.vmgr, file, len), len);
        let _ = self.vmgr.close_file(file);
        out
    }

    /// Promote the CRC-verified upload temp into the trip catalog — the trip twin of
    /// [`upload_commit`](Self::upload_commit). `replace` is the file the trip id already owns (deleted
    /// only *after* the temp validated); `None` names it `TP{fresh_id}.OBT`. Validates the temp parses
    /// as a trip (the transfer CRC only proved the bytes match what the app sent, not that they are a
    /// trip). Returns the final name + byte length, or `None` (temp dropped on an invalid payload, kept
    /// on a transient copy failure — a retry is a whole fresh upload).
    pub fn upload_commit_trip(
        &mut self,
        replace: Option<&ShortFileName>,
        fresh_id: u16,
    ) -> Option<(ShortFileName, u32)> {
        self.upload_close();
        let dir = self.routes_dir?;

        let src_file = self.vmgr.open_file_in_dir(dir, UPLOAD_TMP, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(src_file).unwrap_or(0);
        let valid = TripSummary::read(&SdByteSource::new(&self.vmgr, src_file, len)).is_ok();
        if !valid {
            let _ = self.vmgr.close_file(src_file);
            let _ = self.vmgr.delete_file_in_dir(dir, UPLOAD_TMP);
            defmt::warn!("SD: trip upload is not a valid trip object — rejected");
            return None;
        }

        let final_name = match replace {
            Some(name) => {
                // A trip isn't held open as route geometry, but a detail download could hold it as
                // `open_object` — release that first so the delete + truncate-open below aren't refused.
                self.close_object_if(name);
                if let Err(e) = self.vmgr.delete_file_in_dir(dir, name) {
                    defmt::warn!(
                        "SD: trip replace: cannot delete old {}: {}",
                        defmt::Debug2Format(name),
                        defmt::Debug2Format(&e)
                    );
                }
                name.clone()
            }
            None => match self.fresh_object_name(dir, "TP", fresh_id, "OBT") {
                Some(name) => name,
                None => {
                    let _ = self.vmgr.close_file(src_file);
                    defmt::warn!("SD: trip upload name TP{=u16}.OBT unavailable", fresh_id);
                    return None;
                }
            },
        };

        let copied = match self.vmgr.open_file_in_dir(dir, &final_name, Mode::ReadWriteCreateOrTruncate) {
            Ok(dst_file) => {
                let ok = self.copy_with_held_magic(src_file, dst_file, len);
                if !ok {
                    defmt::warn!("SD: trip upload copy failed — commit aborted (a replaced trip's old file is gone)");
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
        let _ = self.vmgr.delete_file_in_dir(dir, UPLOAD_TMP);
        if !copied {
            return None;
        }
        defmt::info!("SD: trip committed → routes/{} ({=u32} B)", defmt::Debug2Format(&final_name), len);
        Some((final_name, len))
    }

    /// Close the detail-download handle if it currently holds `name` — the trip analogue of
    /// [`close_route_if_open`](Self::close_route_if_open) (trips have no route-geometry handle, only
    /// the shared `open_object` download slot).
    fn close_object_if(&mut self, name: &ShortFileName) {
        if matches!(&self.open_object, Some((on, ..)) if on == name) {
            self.close_object();
        }
    }

    /// Delete a stored trip file (the `deleteObject` trip type / a replace-upload's swap / the
    /// on-device cascade). Releases our own download handle on it first (an open file can't be deleted).
    pub fn delete_trip_file(&mut self, name: &ShortFileName) -> bool {
        let Some(dir) = self.routes_dir else { return false };
        self.close_object_if(name);
        match self.vmgr.delete_file_in_dir(dir, name) {
            Ok(()) => true,
            Err(e) => {
                defmt::warn!("SD: delete trip {} failed: {}", defmt::Debug2Format(name), defmt::Debug2Format(&e));
                false
            }
        }
    }

    /// The on-device long-press **cascade** delete (epic #526 TR3/TR4), map-only build: delete the
    /// trip's member route files *and* the trip file itself, resolving the trip's stage route ids from
    /// its resident [`TripMeta`] (parallel to [`trip_files`](Storage::trip_files)). `true` = the trip
    /// file was deleted; the caller re-scans routes + trips. The `ble` build routes the cascade through
    /// [`ObjectStore::delete_trip_cascade`](crate::object_store::ObjectStore::delete_trip_cascade)
    /// instead, so the wire revision + `storeChanged` stay coherent. A dangling stage id (no such route
    /// file) is simply skipped.
    pub fn delete_trip_cascade_by_id(&mut self, id: u16) -> bool {
        let Some(pos) = self.trip_ids.iter().position(|&x| x == id) else { return false };
        // Snapshot the stage ids + the trip file before mutating (the scan tables are rebuilt after).
        let stages: Vec<u16, { obc_route::MAX_TRIP_STAGES }> = self.trip_metas[pos].stage_ids.clone();
        let trip_file = self.trip_files[pos].clone();
        // Release any active route geometry once up front — `delete_route_file` also closes the handle
        // on the specific file it deletes, but a member being previewed must not block the sweep.
        self.reconcile_route(None);
        for stage_id in &stages {
            // Delete the member route by id (a no-op if the stage id is dangling — already gone).
            let _ = self.delete_route_by_id(*stage_id);
        }
        self.delete_trip_file(&trip_file)
    }

    /// Read the trip-CRC sidecar (`/routes/TRIPS.CRC`) — the trip twin of
    /// [`load_route_crcs`](Self::load_route_crcs); reuses the [`RouteCrcs`] `u16 → u32` codec. A
    /// missing/torn file = the empty map (every trip serves `0 = unknown`).
    pub fn load_trip_crcs(&self) -> RouteCrcs {
        let Some(dir) = self.routes_dir else { return RouteCrcs::new() };
        let Ok(file) = self.vmgr.open_file_in_dir(dir, TRIP_CRCS, Mode::ReadOnly) else {
            return RouteCrcs::new();
        };
        let mut buf = [0u8; ROUTE_CRCS_MAX_LEN];
        let n = self.vmgr.read(file, &mut buf).unwrap_or(0);
        let _ = self.vmgr.close_file(file);
        decode_route_crcs(&buf[..n])
    }

    /// Upsert trip `id`'s whole-object CRC into the sidecar at upload commit (persisting only on a
    /// change) — the trip twin of [`set_route_crc`](Self::set_route_crc).
    pub fn set_trip_crc(&mut self, id: u16, crc: u32) {
        let mut map = self.load_trip_crcs();
        if map.insert(id, crc) {
            self.write_trip_crcs(&map);
        }
    }

    /// Retire trip `id`'s CRC entry (a deleted trip — ids never reuse, belt-and-braces tidiness).
    pub fn forget_trip_crc(&mut self, id: u16) {
        let mut map = self.load_trip_crcs();
        if map.remove(id) {
            self.write_trip_crcs(&map);
        }
    }

    /// Overwrite the trip-CRC sidecar (truncating). A write failure is warned, not fatal — the worst
    /// case is a trip serves `0 = unknown` and re-fills lazily next list build.
    pub fn write_trip_crcs(&mut self, map: &RouteCrcs) {
        let Some(dir) = self.routes_dir else { return };
        let mut buf = [0u8; ROUTE_CRCS_MAX_LEN];
        let n = encode_route_crcs(map, &mut buf);
        match self.vmgr.open_file_in_dir(dir, TRIP_CRCS, Mode::ReadWriteCreateOrTruncate) {
            Ok(file) => {
                if self.vmgr.write(file, &buf[..n]).is_err() {
                    defmt::warn!("SD: trip-crc sidecar write failed — a trip may serve crc 0 next list build");
                }
                let _ = self.vmgr.flush_file(file);
                let _ = self.vmgr.close_file(file);
            }
            Err(e) => defmt::warn!("SD: cannot open trip-crc sidecar: {}", defmt::Debug2Format(&e)),
        }
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

// ==================== The DFU armer plane (epic #615 S4, #619) ====================
//
// The storage half of the app-side armer: locate + validate the staged `UPDATE.BIN` and write
// the `ROLLBACK.BIN` snapshot, both resolved to raw block extents through the same
// `obc_storage::fat_extents` machinery as the map (#500). The *decision logic* — the scan
// matrix, the arm sequencing — is pure and host-tested in `obc_dfu::armer`; these methods are
// its thin `StageIo`/snapshot adapters over FatFs + the raw card. Everything here runs inside
// the ride loop's drained request at shallow per-pass depth, in frames that pop on return —
// the transient `ExtentTable` (~2 KB) and the `StagedRef`s never sit resident.
impl Storage {
    /// Locate an 8.3 `name` in the card root, returning the entry facts the extent build needs:
    /// `(entry_block, entry_offset, byte length)` — the same public `DirEntry` capture as the
    /// map-open scan.
    fn find_root_entry(&self, name: &ShortFileName) -> Option<(embedded_sdmmc::BlockIdx, u32, u32)> {
        let mut found = None;
        self.iter_dir_lfn(self.root, |e, _| {
            if found.is_none() && !e.attributes.is_directory() && e.name == *name {
                found = Some((e.entry_block, e.entry_offset, e.size));
            }
        });
        found
    }

    /// The staging scan (#619 §1): find `UPDATE.BIN` in the card root, decode + validate its
    /// OBCU header, run the **full CRC-32 pass** over the image body through the byte source,
    /// gate the size, and resolve the whole-file extent chain (spec §2.3 — the header is part
    /// of the chain). Typed errors surface verbatim to the debug link now and S5's UI later.
    /// Read-only: a failed scan costs nothing.
    pub fn dfu_scan_update(&mut self) -> Result<obc_dfu::StagedRef, ScanError> {
        let name = ShortFileName::create_from_str(UPDATE_BIN).map_err(|_| ScanError::Io)?;
        let Some((entry_block, entry_offset, len)) = self.find_root_entry(&name) else {
            return Err(ScanError::Missing);
        };
        let file = self.vmgr.open_file_in_dir(self.root, UPDATE_BIN, Mode::ReadOnly).map_err(|_| ScanError::Io)?;
        let mut stage = SdStage { vmgr: &self.vmgr, card: self.card, file, len, entry_block, entry_offset };
        // The CRC staging buffer matches this module's transfer idiom (`copy_with_held_magic`'s
        // 512-byte stack chunk) — no new resident statics; the frame pops with the scan.
        let mut chunk = [0u8; 512];
        let result = obc_dfu::armer::scan(&mut stage, &mut chunk);
        let _ = self.vmgr.close_file(file);
        result
    }

    /// Write the rollback snapshot (#619 §2): `installed`'s raw image — `image`, the caller's
    /// memory-mapped view of the app slot — re-wrapped as a full OBCU container at
    /// `/ROLLBACK.BIN` (truncate-and-reuse, the `TRACK.OBT` idiom), then extent-resolved exactly
    /// like the update file (whole-file chain, spec §2.3).
    ///
    /// `Ok(None)` = the slot's bytes no longer CRC-match the installed header (a dev SWD reflash
    /// since the last install) — a snapshot would record a rollback the bootloader must reject,
    /// so none is taken and any stale `ROLLBACK.BIN` is removed. Errors abort the arm.
    pub fn dfu_write_rollback(
        &mut self,
        installed: &obc_dfu::ImageHeader,
        image: &[u8],
    ) -> Result<Option<obc_dfu::StagedRef>, ScanError> {
        debug_assert_eq!(image.len() as u32, installed.image_len);
        let crc = obc_dfu::crc32(image);
        if crc != installed.image_crc32 {
            defmt::warn!("dfu: running image doesn't match the installed record (SWD reflash?) — no rollback");
            let _ = self.vmgr.delete_file_in_dir(self.root, ROLLBACK_BIN); // don't leave a stale snapshot
            return Ok(None);
        }

        let file = self
            .vmgr
            .open_file_in_dir(self.root, ROLLBACK_BIN, Mode::ReadWriteCreateOrTruncate)
            .map_err(|_| ScanError::Io)?;
        // Header, then the raw image straight from the memory-mapped slot (embedded-sdmmc chunks
        // the long write into blocks itself). Flush before the extent resolve — the chain must
        // be final on card.
        let ok = self.vmgr.write(file, &installed.encode()).is_ok()
            && self.vmgr.write(file, image).is_ok()
            && self.vmgr.flush_file(file).is_ok();
        let _ = self.vmgr.close_file(file);
        if !ok {
            defmt::warn!("dfu: rollback snapshot write failed — arm aborted");
            let _ = self.vmgr.delete_file_in_dir(self.root, ROLLBACK_BIN);
            return Err(ScanError::Io);
        }

        // Resolve the fresh file's chain off its directory entry, exactly like the update file.
        let name = ShortFileName::create_from_str(ROLLBACK_BIN).map_err(|_| ScanError::Io)?;
        let Some((entry_block, entry_offset, len)) = self.find_root_entry(&name) else {
            return Err(ScanError::Io);
        };
        let mut extents = [obc_dfu::Extent::default(); obc_dfu::MAX_EXTENTS];
        let count = resolve_extents(self.card, entry_block, entry_offset, len, &mut extents).map_err(|e| match e {
            ExtentsError::TooFragmented { extents } => ScanError::TooFragmented { extents },
            ExtentsError::Io => ScanError::Io,
        })?;
        defmt::info!(
            "dfu: rollback snapshot written ({=u32} B raw image, {=usize} extent(s))",
            installed.image_len,
            count
        );
        obc_dfu::StagedRef::new(*installed, installed.image_len, crc, &extents[..count])
            .map(Some)
            .ok_or(ScanError::TooFragmented { extents: count as u32 })
    }
}

/// The armer's [`StageIo`] over the open `UPDATE.BIN`: byte reads through the manager's seek
/// path (a scan is one forward pass — the extent-mapped fast path matters for the bootloader's
/// reads, not this one) and the whole-file extent resolve off the raw card.
struct SdStage<'a> {
    vmgr: &'a Vmgr,
    card: &'static Sd,
    file: RawFile,
    len: u32,
    entry_block: embedded_sdmmc::BlockIdx,
    entry_offset: u32,
}

impl StageIo for SdStage<'_> {
    fn stage_len(&mut self) -> Option<u32> {
        Some(self.len)
    }

    fn read_stage(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), obc_dfu::engine::IoError> {
        SdByteSource::new(self.vmgr, self.file, self.len).read_at(offset, buf).map_err(|_| obc_dfu::engine::IoError)
    }

    fn stage_extents(&mut self, out: &mut [obc_dfu::Extent; obc_dfu::MAX_EXTENTS]) -> Result<usize, ExtentsError> {
        resolve_extents(self.card, self.entry_block, self.entry_offset, self.len, out)
    }
}

/// Resolve a root file's FAT chain to `obc_dfu` extents via [`ExtentTable::build`] (the map path's
/// machinery, #500). The table is a ~2 KB transient in this popped frame — unlike the map's
/// session-long `.bss` table, it's read once into `out` and dropped. Two fragmentation walls:
/// `fat_extents`' own cap (128) and `obc_dfu::MAX_EXTENTS` (96, the boot-state page's wire cap).
fn resolve_extents(
    card: &'static Sd,
    entry_block: embedded_sdmmc::BlockIdx,
    entry_offset: u32,
    len: u32,
    out: &mut [obc_dfu::Extent],
) -> Result<usize, ExtentsError> {
    match ExtentTable::build(card, entry_block, entry_offset, len) {
        Ok(table) => {
            let count = table.extent_count();
            if count > out.len() {
                return Err(ExtentsError::TooFragmented { extents: count as u32 });
            }
            for (slot, (lba, blocks)) in out.iter_mut().zip(table.runs()) {
                *slot = obc_dfu::Extent { start_block: lba, blocks };
            }
            Ok(count)
        }
        Err(BuildError::TooFragmented(n)) => Err(ExtentsError::TooFragmented { extents: n }),
        Err(e) => {
            defmt::warn!("dfu: extent resolve failed: {}", defmt::Debug2Format(&e));
            Err(ExtentsError::Io)
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

/// Whether a `/routes` directory entry is a trip file (epic #526 TR4): a BLE-uploaded `TP{id}.OBT`
/// (plain 8.3 like the route uploads' `.OBR`) **or** a side-loaded `.obt` (long-filename match, the
/// trip twin of `.obcr`). Dot-prefixed clutter is excluded on both arms. (The ride log's `TRACK.OBT`
/// shares the `OBT` extension but lives in `/tracks`, never `/routes`, so it can't collide here.)
fn is_trip_entry(e: &embedded_sdmmc::DirEntry, long: Option<&str>) -> bool {
    if e.attributes.is_directory() {
        return false;
    }
    long_has_ext(long, b".obt") || (e.name.extension() == b"OBT" && !long.is_some_and(|n| n.starts_with('.')))
}

/// The **durable trip object id** in an uploaded trip's filename — `TP{id}.OBT` → `id` (spec §7.7;
/// trip ids draw from a device counter separate from routes/rides, §4.1). `None` for a side-loaded
/// `.obt` (no id in the name — it gets a session-scoped one from the reserved band).
pub fn uploaded_trip_id(name: &ShortFileName) -> Option<u16> {
    id_in_name(name, b"TP", b"OBT")
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
