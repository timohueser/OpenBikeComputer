//! microSD storage for the nRF54L board: map / routes / track log over FatFs.
//!
//! This owns the concrete transport → [`VolumeManager`] stack and reconciles the FAT filesystem to
//! the shared app's *intent*, exactly as the simulator's `RouteStore`/`TrackStore` reconcile a
//! folder of files on the host. The reusable, board-agnostic adapters it hands the format code live
//! in [`obc_storage::sd`] ([`SdByteSource`]/[`SdByteSink`]/[`SdTrackSink`]); everything here is
//! nRF-specific.
//!
//! The `Storage` impl and every adapter below are generic over the concrete **block-device type**
//! (they speak `embedded_sdmmc`'s `BlockDevice` / `TimeSource`). So routes and the chosen map both
//! **stream** from the card and the ride is logged to a temp `.obct` converted to the durable ride
//! object on Finish.
//!
//! ## Card layout (FAT16/FAT32)
//!   `/<name>.obcm`   — a side-loaded map (long filename, dragged on from a computer)
//!   `/MP{id}.OBM`    — a map the device received over USB (issue #927): the durable object id lives
//!                      in the 8.3 name, exactly as it does for routes/rides/trips. `OBM` is the
//!                      3-char twin of `.obcm`, the same trick `_NAV.OBR` uses for `.obcr` —
//!                      embedded-sdmmc creates short names only. The upload streams **straight into
//!                      this file** with its 4-byte magic held back, so a torn write leaves a
//!                      zero-magic file the scan refuses and the boot sweep reclaims.
//!   `/MAP.SEL`       — which map the renderer streams from (see `obc_app::store_meta`); absent or
//!                      torn = no preference, and the loader takes the first readable map
//!   `/routes/*.obcr` — the route catalog the Route menu lists (side-loaded, long filenames)
//!   `/routes/RT{id}.OBR` — BLE-uploaded routes (the durable object id lives in the name);
//!                      the in-flight upload lives here as `UPLOAD.TMP` until commit
//!   `/tracks/`       — saved rides (created if absent); the in-progress log lives here as
//!                      `TRACK.OBT` and is deleted once converted. Each Finish writes **one**
//!                      artifact: the BLE ride object `RD{id}.ORD` (the durable ride object id
//!                      lives in the name, mirroring `RT{id}.OBR`). The device writes no GPX —
//!                      the phone owns human-format export after sync.
//!
//! ## The transport: **native 4-bit SD over Nordic's sEMMC soft peripheral** (epic #1158)
//!
//! The card is not on a SPI bus. The FLPR (VPR00) runs Nordic's sEMMC image and *is* the SD host
//! controller; [`crate::semmc`] is the M33-side driver and [`crate::flpr_mux`] decides whether the
//! coprocessor is currently drawing the panel or clocking the card. Wiring (fixed by the soft
//! peripheral, not by us):
//!
//! ```text
//!   P2.00 D3   P2.01 CLK   P2.02 D0   P2.03 D2   P2.04 D1   P2.05 CMD
//! ```
//!
//! Reads run 4-bit at **32 MHz** (14.7 MB/s measured, CMD18 × 256 blocks) and writes at 21.3 MHz
//! (8.2 MB/s, card-program-limited) — against 1.07 MB/s over the SPI transport this replaced. The
//! only thing this file does about any of that is [`SemmcCard`]: a `BlockDevice` over
//! [`crate::semmc::Semmc`]. Everything above it — [`Storage`], the FAT layer, the extent fast path,
//! the object store, the boot-fault rule — is transport-agnostic and did not move.

#[cfg(feature = "sd-bench")]
use embassy_time::Instant;
use embedded_sdmmc::{
    Block, BlockCount, BlockDevice, BlockIdx, LfnBuffer, Mode, RawDirectory, RawFile, ShortFileName, TimeSource,
    Timestamp, VolumeIdx, VolumeManager,
};
use heapless::{String, Vec};
use obc_app::retention::{
    decode_route_retention, encode_route_retention, RouteRetentionMeta, RouteRetentionStore, ROUTE_RETENTION_MAX_LEN,
};
use obc_app::ride::{decode_synced_rides, encode_synced_rides, SyncedRides, SYNCED_RIDES_MAX_LEN};
use obc_app::route::{decode_route_crcs, encode_route_crcs, RouteCrcs, ROUTE_CRCS_MAX_LEN};
use obc_app::store_meta::{decode_store_epoch, encode_store_epoch, STORE_EPOCH_LEN};
use obc_app::{Retention, MAX_RIDES, MAX_ROUTES, UI_RIDES_CAP};
use obc_dfu::armer::{ExtentsError, ScanError, StageIo};
use obc_formats::io::ByteSource;
use obc_formats::obcr::NAME_CAP;
use obc_route::{
    ride_elevation_profile, ride_preview_polyline, track_to_ride, Profile, RideInfo, RideStats, RouteIndex,
    RouteObjectInfo, RouteSummary,
};
use obc_storage::fat_extents::{BuildError, ExtentSource, ExtentTable, SharedBlockDevice};
use obc_storage::route_name;
use obc_storage::{SdByteSink, SdByteSource, SdTrackSink};


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

/// The route-retention sidecar in `/routes` (auto-expiry epic #638, S3) — route object id →
/// `(retention u8, last_used u32)`, the device-local expiry state the sweep reads. In `/routes` (not
/// the byte-pinned OBCR file, not RRAM) so it survives a reflash and travels with the card/routes; a
/// missing/torn file decodes to the **empty** store (every route reads `Never` → nothing deletes, the
/// safe direction that self-heals when the app re-pushes retention in S7). Mirrors the [`ROUTE_CRCS`]
/// sidecar's file handling; codec + torn-line semantics live in `obc-app::retention` (host-tested).
const ROUTE_RETENTION: &str = "ROUTES.RET";

/// Which step of a CRC-framed sidecar rewrite failed (finding #876-5). A truncating rewrite is only
/// **durable** when open, write, flush, **and** close all succeed; a swallowed flush/close error is a
/// torn persist. Callers whose failure direction is safe by design (a torn retention/synced sidecar
/// decodes conservatively → nothing deletes) may treat this best-effort, but the `setRouteRetention`
/// reply must never claim `ok` ahead of durability — it maps a failure to `command` `Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarWriteError {
    /// The directory was absent, or the file could not be opened for a truncating rewrite.
    Open,
    /// The payload write failed.
    Write,
    /// The flush after the write failed (bytes may not have reached the card).
    Flush,
    /// The close failed (the directory entry / length may not be committed).
    Close,
}

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

/// The **selected map** file in the card root (issue #927): the 8.3 filename of the `.obcm` the
/// renderer streams from, in the same tiny CRC-framed shape as [`EPOCH_FILE`]. Card-resident rather
/// than an RRAM setting because the thing it names lives on the card — swap in another card and it
/// brings its own selection; an RRAM line would point at a file that may not be in the slot now.
/// A missing/torn file reads as **no preference**, and [`Storage::open_map`] falls back to the first
/// readable map, which is exactly the pre-#927 behaviour. Codec + torn semantics live in
/// `obc-app::store_meta` (host-tested), like every other card sidecar.
const MAP_SELECTED: &str = "MAP.SEL";

/// How many maps one card's catalog scan reports (issue #927). Maps are hundreds of megabytes, so a
/// card holding more than a handful is not a real configuration — this is a scan bound, not a store
/// cap: an upload is never refused for exceeding it, the extra map simply isn't listed.
pub const MAX_MAPS: usize = 8;

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

/// The concrete SD stack for this board: [`SemmcCard`] — the card in native 4-bit mode on the FLPR
/// — under a 16-file/4-dir [`VolumeManager`].
///
/// **Why more than 4 open files** (the default 4 loses mid-ride uploads): riding with tracking holds three
/// handles for the whole session — the map stream, the active route's geometry, and the ORD track
/// log. A BLE route upload adds its temp (4), and `upload_commit`'s copy-promote (embedded-sdmmc
/// can't rename) holds the reopened temp **and**
/// the final `.OBR` at once — a 5-handle peak, which the 4-slot default answered with a failed
/// commit exactly and only mid-ride. Each slot is 64 bytes of `FileInfo`, so the RAM cost is
/// noise — which is why the budget below was never trimmed back after the set that forced it up
/// was deleted.
type Sd = SemmcCard;
/// What the manager actually owns: the card **by shared reference** ([`SharedBlockDevice`]), so
/// the raw `&'static Sd` twin stays available for the map's extent-mapped direct block reads
/// (#500) — `VolumeManager::device()` can't hand it back out (its 0.9 signature can only return
/// the `TimeSource` type), so the share happens here, one level up. The card itself lives in
/// [`SD_CARD`].
type SdShared = SharedBlockDevice<'static, Sd>;
/// The open-handle budget (see the file-count note above) — one set of consts so the manager and
/// the `obc-platform` wrapper aliases below can never drift apart.
const SD_MAX_DIRS: usize = 4;
/// **Why 16, and why it is not 6 again.** The pre-volume-set budget was 6: the 5-handle mid-ride
/// peak documented above plus one slot of headroom. A mounted volume set then needed one handle per
/// shard for the mount lifetime, and 16 is where that landed.
///
/// The set is gone (FS7.5-c3b, #1420) and this is **deliberately not reverted to 6**. The cost is
/// 640 B of `.bss` that the resource baseline already accounts for, and the reason to leave it is
/// that this whole module is FS11's to delete (#1393) — shrinking a budget inside a stack that is
/// being retired trades a measured, harmless allocation for a fresh chance to under-size the one
/// thing that fails *only* mid-ride. If FS11 slips, this is a two-line change with a number
/// already attached.
///
/// The cost is measured, not guessed: the fork's `FileInfo` (`filesystem/files.rs`) is `RawFile`
/// 4 · `RawVolume` 4 · `current_cluster` 8 · `current_offset` 4 · `Mode` 1 · `DirEntry` 40 ·
/// `dirty` 1, i.e. **64 B** at `align 4` on thumbv8m. `6 → 16` is ten slots, **+640 B of `.bss`**
/// — the manager's `open_files` array is a `heapless::Vec<FileInfo, SD_MAX_FILES>` and nothing
/// else scales with it. Nothing on the stack changes.
const SD_MAX_FILES: usize = 16;
const SD_MAX_VOLUMES: usize = 1;
type Vmgr = VolumeManager<SdShared, NullTime, SD_MAX_DIRS, SD_MAX_FILES, SD_MAX_VOLUMES>;
/// [`SdByteSource`] over this board's manager (the wrappers are generic over the handle budget).
type Source<'a> = SdByteSource<'a, SdShared, NullTime, SD_MAX_DIRS, SD_MAX_FILES, SD_MAX_VOLUMES>;
/// [`SdByteSink`] over this board's manager — the router's OBCR emit writes through it.
type Sink<'a> = SdByteSink<'a, SdShared, NullTime, SD_MAX_DIRS, SD_MAX_FILES, SD_MAX_VOLUMES>;
/// [`SdTrackSink`] over this board's manager.
type TrackSinkT<'a> = SdTrackSink<'a, SdShared, NullTime, SD_MAX_DIRS, SD_MAX_FILES, SD_MAX_VOLUMES>;

/// The block device's home — a `.bss` slot written once by [`init`] (the warm-reset-safe
/// `init_static` pattern, see `main.rs`), so both the [`VolumeManager`] (via [`SdShared`]) and the
/// extent read path can borrow it for `'static`. [`SemmcCard`] is a zero-sized handle (the driver
/// state lives in `flpr_mux`), so this slot costs nothing; it stays because the `'static` borrow
/// shape above it is what the extent fast path is built on.
static mut SD_CARD: core::mem::MaybeUninit<Sd> = core::mem::MaybeUninit::uninit();

/// **The open map's resident extent table** — the one direct-read table this board keeps
/// (`#500`), written once per boot by [`build_map_extents`](Storage::build_map_extents).
///
/// It used to be one arm of an `ExtentSlots` union, whose other arm was eleven smaller tables — one
/// per shard of a mounted volume set — because `open_map` made a one-map-**or**-one-set choice per
/// boot and the two could never be live together. A map is one file (FS7.5, #1420), so there is no
/// second arm, no `ManuallyDrop` union-field ceremony, and no four const asserts pinning the
/// larger arm's size and alignment over the smaller. The union's whole size was the set's;
/// what stays resident is the 128-run table a standalone map always needed.
///
/// It stays **outside** [`Storage`] because that value crosses `main`'s async frame by value, and
/// an inline table measurably produced two extra resident copies (#270/#500).
static mut MAP_EXTENTS: core::mem::MaybeUninit<ExtentTable> = core::mem::MaybeUninit::uninit();

/// The `'static` direct-read source over the open map, for the one consumer that needs the map's
/// bytes to outlive a `&self` borrow: the **embedded terrain window** (`OBCM_Spec.md` §1.3).
///
/// [`map_source`](Storage::map_source) hands out a fresh, cheap source per redraw and borrows
/// `self`, which is exactly right for the render loop and useless to `TerrainElevation`, whose
/// parsed reader holds its source for the session. This slot is that source, placed once.
///
/// It is the **extent path only**, and — read this before repeating the earlier claim that it is
/// "the sidecar's rule kept" — **that is a real behaviour change, not a rule preserved.**
///
/// The sidecar's rule was about the *sidecar's own* fragmentation: `MAP.OBD` had its own chain and
/// its own extent table, so a `MAP.OBM` too fragmented for a 128-run table (#504's `MAP_SLOW` case)
/// still got terrain as long as the small, freshly-written raster mapped cleanly. Terrain is inside
/// the map file now, so the map's chain **is** the terrain's chain: a fragmented map now loses
/// elevation as well as speed. The two failures were independent and are one.
///
/// The reason the seek path is still not admitted is the sidecar's, and it is unchanged: a terrain
/// sample sits inside the nav emit loop, and reinserting a FAT walk per 512 B tile would put SD
/// seeks under the router. What changed is only how much rides on the map's own fragmentation.
///
/// Nothing here restores the independence — there is no second file to be independent of, and
/// buying it back would mean a second copy of the region on the card. What this slice owes instead
/// is that the case is **never silent**: `mount_terrain` warns when a map carries a §1.3 region and
/// the extent table refused, so a rider whose routes went flat has a line that says why, next to
/// the `MAP_SLOW` notice that says the same thing about speed.
static mut MAP_STATIC_SOURCE: core::mem::MaybeUninit<ExtentSource<'static, Sd>> = core::mem::MaybeUninit::uninit();

/// Exact target-side bytes of the board-private map read statics — the extent table and the
/// `'static` source over it — exported numerically for the compile-time RAM budget and the resource
/// report in `main.rs` without exposing their concrete types.
pub(crate) const MAP_EXTENT_BYTES: usize =
    core::mem::size_of::<ExtentTable>() + core::mem::size_of::<ExtentSource<'static, Sd>>();

/// One map on the card, as [`Storage::scan_maps_into`] reports it (issue #927) — **the map
/// catalog**, and the reason there is no catalog *file*.
///
/// Every field here is derived from the card at scan time: the directory entry gives the name, the
/// long-name stem and the byte length, and a single 40-byte header read gives the OBCM version and
/// the global bbox. Nothing has to be written, so nothing can go stale, and a card that has never
/// met this firmware enumerates exactly as one that has.
///
/// What is **not** here is equally deliberate. The 40-byte OBCM header carries no name, no build
/// date and no source-snapshot date (`OBCM_Spec.md` §1), and a map upload is a stream of opaque
/// bytes with a 12-byte descriptor in front of it — so the device is never *told* a display name or
/// a build date and has nothing to record. That is a protocol gap, not a filesystem one; see the
/// notes on #914/#915.
#[derive(Debug, Clone)]
pub struct MapSummary {
    /// The durable object id, for a map this device received (`MP{id}.OBM`). `None` for a
    /// side-loaded `.obcm`, which carries no device-assigned identity — the filename is all it has.
    pub id: Option<u16>,
    /// The 8.3 filename, which is what [`MAP_SELECTED`] records and what reopens the file.
    pub file: ShortFileName,
    /// The display name: the long filename's stem when the file has one, else the 8.3 stem. For an
    /// uploaded map that is `MP{id}` — the honest consequence of having no name on the wire.
    pub name: String<24>,
    /// Size on the card, from the directory entry (no read). `u64` because a map is exactly the
    /// thing that outgrows one `u32` file.
    pub byte_len: u64,
    /// The OBCM format version from header byte 4. Reported, never filtered: a map built for
    /// another version is still on the card, and a consumer that wants to *flag* it (#915) needs
    /// to see it.
    pub obcm_version: u8,
    /// Whether [`MAP_SELECTED`] names this map.
    pub selected: bool,
    /// Directory-entry location, so a chosen map's extent table can be built without a second scan.
    entry_block: embedded_sdmmc::BlockIdx,
    entry_offset: u32,
}

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
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), obc_formats::io::Error> {
        match self {
            MapSource::Extent(s) => s.read_at(offset, buf),
            MapSource::Seek(s) => s.read_at(offset, buf),
        }
    }

    fn len(&self) -> u64 {
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
    /// [`scan_rides_into`](Storage::scan_rides_into) last returned — so a ride's durable object id resolves back
    /// to the `RD{id}.ORD` file for detail reads and object-store deletes.
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
    /// RAM — `map_source` hands out a fresh source over it each redraw.
    open_map: Option<(RawFile, u32)>,
    /// The open map's 8.3 filename. Kept because embedded-sdmmc refuses every second open of an open
    /// file (`FileAlreadyOpen`), so [`scan_maps_into`](Storage::scan_maps_into) must read the loaded
    /// map's header **through this handle** — without the name it cannot tell which catalog entry is
    /// the open one, and the loaded map would be the one map missing from its own catalog (the
    /// `open_object` trap from issue #480, in the map plane).
    open_map_name: Option<ShortFileName>,
    /// The open map's FAT chain resolved to extent runs (#500): when present, `map_source` serves
    /// direct block reads (zero per-read FAT traffic) instead of the manager's O(offset) seek.
    /// `None` = build refused (fragmented past the cap / odd geometry) or failed verification —
    /// the seek path still works, just slowly, and open_map logged why. A reference into the
    /// [`MAP_EXTENTS`] `.bss` slot — see its doc for why the table must not live in here by value.
    map_extents: Option<&'static ExtentTable>,
    /// The fault the boot must show when [`map_source`](Storage::map_source) has nothing to hand
    /// out — `None` until [`open_map`](Storage::open_map) has run, then whatever
    /// [`obc_app::boot_fault`] answers for the card that was actually scanned.
    ///
    /// It exists because *NO MAP* and *MAP UNREADABLE* are different sentences to a rider, and the
    /// card cannot tell them apart from a failed `map_source` alone. Every path in `open_map` that
    /// gives up **with a map-named file on the card** records it — a refused set mount, a chosen
    /// map the FAT layer will not open, a zero-length one, and the scan's own rejects (a torn
    /// magic, a file too short to hold a header). A card that genuinely holds nothing leaves the
    /// answer at *NO MAP*.
    map_boot_fault: Option<obc_app::BootFault>,
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
    /// The loaded map's display name — its filename stem, captured in [`open_map`](Storage::open_map)
    /// (T8 item 6). Empty until a map opens; the System settings screen renders it (`grimsel · v10`)
    /// via [`App::set_map_info`](obc_app::App::set_map_info).
    map_name: String<24>,
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

/// **Bring the card up, and mount nothing on it.** Boot the sEMMC soft peripheral and identify the
/// card (4-bit, High Speed, 32 MHz reads).
///
/// Split out of [`init`] by FS7.5-c1, because bring-up is the **only** step the two storage stacks
/// share. What is on the card decides the rest: a flat store owns the raw card from LBA 0 and a FAT
/// volume is a filesystem on it, so exactly one of them can be there. Boot therefore brings the card
/// up once, lets `FlatStore::mount` classify it (`FLAT_Store_Format.md` §5.6 step 1 — see
/// `crate::flat_store`), and only then mounts FAT with [`mount_fat`] on a card that is not a flat
/// store.
///
/// **The order is about honest reporting, not about correctness.** The two classifiers are disjoint
/// by construction — a flat card's zero MBR footer and this stack's `0xAA55` requirement, in both
/// directions; `crate::flat_store`'s module docs carry the argument — so neither can accept the
/// other's card whichever runs first. What the order buys is the *message*: mounting FAT first would
/// have reported *STORAGE FAULT* for a perfectly good flat card, sending its owner to look at a
/// filesystem that was never on it.
///
/// **That ordering is held structurally rather than by a test**: there is exactly one caller of
/// each of these two functions, in `main`'s boot block, and no test harness exists in this crate to
/// pin it. A second call site is what would break it, and the honest guard against that is that
/// there is nowhere else in the image that wants one.
///
/// Card identification is the slow part — the ACMD41 power-up poll is bounded at 1.5 s.
/// [`flpr_mux::bring_up_storage`](crate::flpr_mux::bring_up_storage) is what holds the FLPR in
/// storage mode across the whole of it (see its doc, and PR #1160's `Semmc::start` contract).
///
/// ## ⚠️ Synchronous on purpose — the async-fn frame trap (#677, #1108)
///
/// The obvious shape for this is `async fn`, and it was one for a while: `Semmc::start` yields at
/// the card settle, the CMD8 deliver-and-abort and each ACMD41 poll. Awaited from `main`, that
/// chain's coroutine flattens into `main`'s **task-body poll frame**, whose slot set is allocated on
/// entry on *every* poll for the life of the program — measured **6,912 → 13,376 B** for a function
/// that runs once at boot, and `#[inline(never)]` does not fix it (it governs the future
/// constructor, not the coroutine body). Synchronous — the shape that ships — the same build
/// measures **7,328 B**, which is what the resource guard pins as `task_frame_measured`.
///
/// It is safe to block here: bring-up runs before the ride loop, the BLE stack and the USB plane
/// exist, and the panel's anti-DC-bias COM wave is on the P3 `InterruptExecutor` (or, on `com-hw`,
/// on TIMER + DPPI + GPIOTE), so it preempts thread mode rather than competing with it. See
/// `Semmc::start`'s note for the full accounting.
pub fn bring_up_card() -> Result<(), obc_app::BootFault> {
    let info = match crate::flpr_mux::bring_up_storage() {
        Ok(info) => info,
        Err(e) => return Err(bring_up_fault(e)),
    };
    defmt::info!(
        "SD: card up over sEMMC — {=u32} MB, 4-bit, {=u32} MHz reads (high-speed {=bool}), RCA 0x{=u16:04x}; FLPR mode: {=str}",
        (info.blocks >> 11),
        info.read_clk_hz / 1_000_000,
        info.high_speed,
        info.rca,
        crate::flpr_mux::mode_name()
    );
    Ok(())
}

/// **Mount the v1 FAT stack** on a card [`bring_up_card`] has already brought up and
/// `crate::flat_store` has already classified as *not* a flat store.
pub fn mount_fat() -> Result<Storage, obc_app::BootFault> {
    // Into its `.bss` slot before anything else: the manager and the extent read path both want
    // `'static` borrows of the one card.
    // SAFETY: sole writer of SD_CARD; this runs once per boot on the one thread-mode executor,
    // and a warm-reset re-run overwrites in place (no `Drop`), the `init_static` contract.
    let card: &'static Sd = unsafe { crate::init_static(core::ptr::addr_of_mut!(SD_CARD), SemmcCard) };
    Storage::mount(card).ok_or_else(|| {
        defmt::error!("SD: the card is up but the FAT volume would not mount — STORAGE FAULT");
        obc_app::BootFault::StorageFault
    })
}

/// **Which fault screen a failed bring-up earns** — the honesty rule (#1163 review, P3): a fault
/// line the rider can act on, not one catch-all.
///
/// Three classes, because the driver can genuinely tell them apart:
///
/// - [`SemmcError::NoCard`] is the only one that means what "NO SD CARD" says — the host came up and
///   identification found nothing (empty socket, dead card, broken bus).
/// - [`SemmcError::UnsupportedCard`] means a working card that is SDSC. Dropping SDSC is deliberate
///   (byte-addressed, ≤2 GB, no map fits), but a card that worked over the retired SPI path now
///   fails, and "NO SD CARD" would send its owner hunting for a card that is already inserted.
/// - everything else is the storage subsystem itself — the soft peripheral that would not boot, a
///   barrier that never echoed, a transport error during identification. None of those are evidence
///   about whether a card is present, so they read as the honest superset.
fn bring_up_fault(e: crate::semmc::SemmcError) -> obc_app::BootFault {
    use crate::semmc::SemmcError;
    match e {
        SemmcError::NoCard => {
            defmt::warn!("SD: card identification found no card — NO SD CARD");
            obc_app::BootFault::NoCard
        }
        SemmcError::UnsupportedCard => {
            defmt::error!("SD: card is SDSC (CSD v1, <=2 GB) — rejected; CARD UNSUPPORTED");
            obc_app::BootFault::CardUnsupported
        }
        other => {
            defmt::error!("SD: the sEMMC host did not come up ({}) — STORAGE FAULT, not a missing card", other);
            obc_app::BootFault::StorageFault
        }
    }
}

// ═══════════════════════════ the block device ═══════════════════════════

/// **The card as an `embedded_sdmmc::BlockDevice`** — the whole transport (epic #1158).
///
/// A zero-sized handle: the driver state is the one [`Semmc`](crate::semmc::Semmc) in
/// [`crate::flpr_mux`], which also decides whether the FLPR is currently drawing the panel or
/// clocking the card. Every method here is one `flpr_mux::with_storage` call — mode ensured, driver
/// borrowed, transfer issued — so there is exactly one place that can get the ordering wrong.
///
/// Blocking, like the SPI transport before it: `BlockDevice` is a synchronous trait and everything
/// above it (FAT, extents, the object store) is built on that. The transfers are ~30× shorter now.
#[derive(Clone, Copy)]
pub(crate) struct SemmcCard;

// ── On-device map-read profiler (`sd-bench`) ────────────────────────────────────────────────

/// Cumulative physical-read counters at the concrete block-device boundary.
///
/// The renderer already counts logical `ByteSource::read_at` calls and requested bytes. These
/// counters deliberately sit lower: one sample covers the complete `with_storage` span (FLPR mode
/// acquisition, sEMMC command(s), and a rare alignment-bounce copy), so their delta is the M33's
/// actual awake time attributable to card reads. Relaxed atomics are enough: storage is synchronous
/// on the one thread-mode executor; atomics only make a snapshot unambiguously race-free to read.
#[cfg(feature = "sd-bench")]
#[derive(Clone, Copy)]
pub(crate) struct ReadPerf {
    pub(crate) us: u32,
    pub(crate) commands: u32,
    pub(crate) blocks: u32,
    pub(crate) single_commands: u32,
    pub(crate) multi_commands: u32,
}

#[cfg(feature = "sd-bench")]
impl ReadPerf {
    pub(crate) const ZERO: Self = Self { us: 0, commands: 0, blocks: 0, single_commands: 0, multi_commands: 0 };

    pub(crate) fn since(self, before: Self) -> Self {
        Self {
            us: self.us.wrapping_sub(before.us),
            commands: self.commands.wrapping_sub(before.commands),
            blocks: self.blocks.wrapping_sub(before.blocks),
            single_commands: self.single_commands.wrapping_sub(before.single_commands),
            multi_commands: self.multi_commands.wrapping_sub(before.multi_commands),
        }
    }

    pub(crate) fn add_assign(&mut self, other: Self) {
        self.us = self.us.wrapping_add(other.us);
        self.commands = self.commands.wrapping_add(other.commands);
        self.blocks = self.blocks.wrapping_add(other.blocks);
        self.single_commands = self.single_commands.wrapping_add(other.single_commands);
        self.multi_commands = self.multi_commands.wrapping_add(other.multi_commands);
    }
}

#[cfg(feature = "sd-bench")]
static READ_US: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "sd-bench")]
static READ_COMMANDS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "sd-bench")]
static READ_BLOCKS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "sd-bench")]
static READ_SINGLE_COMMANDS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
#[cfg(feature = "sd-bench")]
static READ_MULTI_COMMANDS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[cfg(feature = "sd-bench")]
pub(crate) fn read_perf_snapshot() -> ReadPerf {
    use core::sync::atomic::Ordering::Relaxed;
    ReadPerf {
        us: READ_US.load(Relaxed),
        commands: READ_COMMANDS.load(Relaxed),
        blocks: READ_BLOCKS.load(Relaxed),
        single_commands: READ_SINGLE_COMMANDS.load(Relaxed),
        multi_commands: READ_MULTI_COMMANDS.load(Relaxed),
    }
}

#[cfg(feature = "sd-bench")]
fn note_read_perf(started: Instant, commands: usize, blocks: usize, single_commands: usize) {
    use core::sync::atomic::Ordering::Relaxed;
    let elapsed = started.elapsed().as_micros().min(u64::from(u32::MAX)) as u32;
    READ_US.fetch_add(elapsed, Relaxed);
    READ_COMMANDS.fetch_add(commands as u32, Relaxed);
    READ_BLOCKS.fetch_add(blocks as u32, Relaxed);
    READ_SINGLE_COMMANDS.fetch_add(single_commands as u32, Relaxed);
    READ_MULTI_COMMANDS.fetch_add((commands - single_commands) as u32, Relaxed);
}

// ═══════════════════════ the alignment bounce ═══════════════════════
//
// **The map-upload write pipeline that used to live here is gone** (FS7.5-c3b, #1420). It coalesced
// a `VolumeManager::write`'s per-cluster calls back into one CMD25 while a staged USB upload
// streamed through the scratch arena, and it was enabled by exactly one thing — that stage. Protocol
// v4 writes each 4 KiB stream record straight to the flat store, so nothing enables it and nothing
// ever could; what is left below is the alignment bounce, which the ride path and the flat binding
// both still need.

/// **The alignment bounce** (epic #1158).
///
/// The sEMMC firmware's DMA requires 32-bit-aligned buffers, and `embedded_sdmmc::Block` cannot
/// promise one: it is `#[repr(transparent)]` over `[u8; 512]`, and that transparency is
/// load-bearing — `Block::slice_from_bytes` reinterprets a caller's plain byte buffer as `&[Block]`
/// without copying, which is exactly what lets the fork's `VolumeManager::write` hand a whole
/// cluster run to one CMD25. `#[repr(align(4))]` cannot coexist with `#[repr(transparent)]`, and
/// adding it in our fork would make that reinterpretation unsound for every misaligned byte buffer
/// in the tree. So the fork is left alone and the alignment is handled here.
///
/// A block is 512 B — itself a multiple of 4 — so the *whole span* is aligned iff its first byte is:
/// one test, no per-block arithmetic. When it fails, the transfer is chunked through this buffer
/// instead of degrading to one command per block: a misaligned run still moves in
/// [`BOUNCE_BLOCKS`]-block CMD18/CMD25 batches. It should never fire in practice — the ride path's
/// buffers are aligned — so [`WARNED_BOUNCE`] reports the first one, which is how a future
/// regression that quietly cut read throughput would be noticed on glass rather than in a profile.
const BOUNCE_BLOCKS: usize = 4;
/// The bounce buffer's resident size — named in the `resource-report` table (`sd_bounce`) so its
/// 2 KB of `.data` stays legible in the report rather than anonymous.
pub(crate) const BOUNCE_BYTES: usize = BOUNCE_BLOCKS * BLOCK_LEN;
#[repr(C, align(4))]
struct Bounce([u8; BOUNCE_BYTES]);
static mut BOUNCE: Bounce = Bounce([0; BOUNCE_BYTES]);
/// One-shot latch so a misaligned buffer is diagnosed once, not per block.
static WARNED_BOUNCE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

const BLOCK_LEN: usize = Block::LEN;

/// `Block` is `#[repr(transparent)]` over `[u8; 512]`, which is what makes the byte views below
/// sound. Pinned here because the whole bounce/fast-path split is built on it.
const _: () = assert!(core::mem::size_of::<Block>() == BLOCK_LEN);

fn warn_bounce(addr: usize) {
    if !WARNED_BOUNCE.swap(true, core::sync::atomic::Ordering::Relaxed) {
        defmt::warn!("SD: misaligned block buffer at 0x{=usize:08x} — bouncing (throughput cost)", addr);
    }
}

/// **Lend the alignment bounce to the flat store's binding** (FS7.5-c1, `crate::flat_store`).
///
/// One buffer for both stacks, and shared rather than duplicated because 2 KiB of `.bss` on this
/// part is 2 KiB of main stack (`_stack_start − __euninit`) and the two can never want it at the
/// same instant: every use — this stack's and the flat one's — is inside a
/// [`flpr_mux::with_storage`](crate::flpr_mux::with_storage) closure, and that borrow is
/// non-re-entrant by assertion. It also does not outlive the FAT stack the way a second buffer
/// would: c4 deletes this module and the buffer with it, at which point the flat binding places its
/// own — one line, and a decision made when there is a measurement to make it against.
///
/// The size is FAT's, not the flat store's: 4 blocks, where §5.5's commit window is 8. So a
/// *misaligned* commit body moves in two card commands per window instead of one. It costs nothing
/// when the store's buffers happen to be word-aligned, which `warn_bounce`'s one-shot line is how
/// anyone finds out — and a mount, whose window is 2 KiB, is exactly one chunk either way.
///
/// # Safety
/// The caller must be inside a `flpr_mux::with_storage` closure (which is where the exclusivity
/// argument above comes from) and must not call any other bounce user from within `f`.
pub(crate) unsafe fn with_bounce<R>(addr: usize, f: impl FnOnce(&mut [u8]) -> R) -> R {
    warn_bounce(addr);
    // SAFETY: the caller's obligation above makes this the sole live borrow.
    let bounce = unsafe { &mut *core::ptr::addr_of_mut!(BOUNCE) };
    f(&mut bounce.0)
}

/// **Report the mounted volume's cluster size, once, at mount.**
///
/// Purely diagnostic. The fast uploader coalesces adjacent FAT cluster calls into one physical
/// write, so correctness and batching no longer depend on one exact cluster size; the line keeps
/// an on-glass throughput result interpretable.
///
/// Read straight off the card rather than asked of `VolumeManager`, which does not expose it. The
/// checks are `obc-storage`'s `fat_extents.rs`, borrowed rather than re-invented — with one
/// deliberate difference: that one requires an MBR and fails a superfloppy card outright, because it
/// is building extents the ride path depends on and would rather refuse than guess. This is a boot
/// log line, so it reads sector 0 as a BPB first and only then looks for a partition table. The two
/// therefore disagree about a superfloppy: `fat_extents` rejects it, this reports its cluster size.
/// That is not drift to reconcile — a card the volume manager cannot mount never reaches an upload,
/// so the only cost of being lenient here is a truthful log line for a card nothing else will use.
///
/// **A boot signature alone does not tell an MBR from a BPB.** A *superfloppy* card — no partition
/// table, the volume boot record directly in sector 0, common on SD — carries `0xAA55` at 510 too,
/// so a signature check that then read bytes 0x1C6.. as a partition LBA would take part of the BPB
/// or the boot code as a sector number. What separates them is what sector 0 looks like *as a BPB*:
/// a real BPB declares `BPB_BytsPerSec == 512` and a non-zero `BPB_SecPerClus`. So: parse sector 0
/// as a BPB first, and only go looking for a partition table when it is not one.
///
/// Two block reads at boot, and nothing downstream depends on the result.
fn report_cluster_size() {
    let mut block = [0u8; 512];
    // `with_storage` answers `Err` when the FLPR is not in storage mode; the inner result is the
    // card's. Either failure means "no answer", and no answer is not worth a fault here.
    let read =
        |lba: u32, buf: &mut [u8]| matches!(crate::flpr_mux::with_storage(|sd| sd.read_blocks(lba, buf)), Ok(Ok(())));
    /// Does this block parse as a FAT BPB the volume manager would mount?
    fn is_bpb(block: &[u8; 512]) -> bool {
        u16::from_le_bytes([block[510], block[511]]) == 0xAA55
            && u16::from_le_bytes([block[11], block[12]]) == 512
            && block[13] != 0
    }
    if !read(0, &mut block) {
        return;
    }
    if !is_bpb(&block) {
        // Not a volume boot record, so sector 0 should be an MBR. Partition entry 0 is 16 B at
        // 0x1BE; bytes 8..12 are the LBA of its first sector, and byte 4 its type — checked against
        // the same FAT types the manager mounts, so a foreign first partition is reported as
        // unknown rather than followed.
        let part = &block[0x1BE..0x1BE + 16];
        let fat_type = matches!(part[4], 0x01 | 0x04 | 0x06 | 0x0B | 0x0C | 0x0E);
        let start = u32::from_le_bytes([part[8], part[9], part[10], part[11]]);
        if (part[0] & 0x7F) != 0 || !fat_type || start == 0 || !read(start, &mut block) || !is_bpb(&block) {
            defmt::warn!("SD: no FAT BPB in sector 0 or partition 0 — upload flush shape unknown");
            return;
        }
    }
    let bytes_per_sector = u16::from_le_bytes([block[11], block[12]]) as u32;
    let sectors_per_cluster = block[13] as u32;
    defmt::info!("SD: {=u32} B clusters", bytes_per_sector * sectors_per_cluster);
}

/// Log a failed transfer at the transport boundary, decoding an abort's `STATUS` word.
///
/// The FAT layer above swallows the error type into its own `Error::DeviceError`, so this is the
/// last place the *reason* exists — and the reason is the difference between "the card is gone",
/// "the clock is too high for this wiring" and "the firmware wedged", which is exactly the triage a
/// bad card or a marginal harness needs from an RTT log.
fn log_transfer_error(op: &'static str, lba: u32, blocks: usize, e: crate::semmc::SemmcError) {
    match e {
        crate::semmc::SemmcError::Aborted(status) => defmt::warn!(
            "SD: {=str} of {=usize} block(s) @ {=u32} aborted — {=str} (STATUS 0x{=u32:08x})",
            op,
            blocks,
            lba,
            crate::semmc::SemmcError::abort_reason(status),
            status
        ),
        other => {
            defmt::warn!("SD: {=str} of {=usize} block(s) @ {=u32} failed — {}", op, blocks, lba, other)
        }
    }
}

impl BlockDevice for SemmcCard {
    type Error = crate::semmc::SemmcError;

    fn read(&self, blocks: &mut [Block], start_block_idx: BlockIdx) -> Result<(), Self::Error> {
        if blocks.is_empty() {
            return Ok(());
        }
        let (addr, n) = (blocks.as_ptr() as usize, blocks.len());
        #[cfg(feature = "sd-bench")]
        let bench_started = Instant::now();
        let r = crate::flpr_mux::with_storage(|sd| {
            if addr.is_multiple_of(4) {
                // SAFETY: `Block` is `#[repr(transparent)]` over `[u8; 512]` (asserted above), so a
                // `&mut [Block]` is exactly this byte span, exclusively borrowed for the call.
                let buf = unsafe { core::slice::from_raw_parts_mut(blocks.as_mut_ptr().cast::<u8>(), n * BLOCK_LEN) };
                return sd.read_blocks(start_block_idx.0, buf);
            }
            warn_bounce(addr);
            // SAFETY: sole borrow — this runs inside `with_storage`, which is non-re-entrant, and
            // no interrupt handler touches the bounce.
            let bounce = unsafe { &mut *core::ptr::addr_of_mut!(BOUNCE) };
            for (i, chunk) in blocks.chunks_mut(BOUNCE_BLOCKS).enumerate() {
                let len = chunk.len() * BLOCK_LEN;
                sd.read_blocks(start_block_idx.0 + (i * BOUNCE_BLOCKS) as u32, &mut bounce.0[..len])?;
                for (b, src) in chunk.iter_mut().zip(bounce.0[..len].chunks_exact(BLOCK_LEN)) {
                    b.contents.copy_from_slice(src);
                }
            }
            Ok(())
        })?;
        #[cfg(feature = "sd-bench")]
        {
            let commands = if addr.is_multiple_of(4) { 1 } else { n.div_ceil(BOUNCE_BLOCKS) };
            let single_commands = if addr.is_multiple_of(4) {
                usize::from(n == 1)
            } else {
                // Every full bounce chunk is one BOUNCE_BLOCKS-block CMD18. Only a one-block
                // remainder is CMD17; 2- and 3-block remainders are still CMD18, not singles.
                usize::from(n % BOUNCE_BLOCKS == 1)
            };
            note_read_perf(bench_started, commands, n, single_commands);
        }
        if let Err(e) = r {
            log_transfer_error("read", start_block_idx.0, n, e);
        }
        r
    }

    fn write(&self, blocks: &[Block], start_block_idx: BlockIdx) -> Result<(), Self::Error> {
        if blocks.is_empty() {
            return Ok(());
        }
        let (addr, n) = (blocks.as_ptr() as usize, blocks.len());
        let r = crate::flpr_mux::with_storage(|sd| {
            if addr.is_multiple_of(4) {
                // SAFETY: as in `read` — `Block` is `#[repr(transparent)]` over `[u8; 512]`, and the
                // shared borrow covers the whole span for the call.
                let buf = unsafe { core::slice::from_raw_parts(blocks.as_ptr().cast::<u8>(), n * BLOCK_LEN) };
                return sd.write_blocks(start_block_idx.0, buf);
            }
            warn_bounce(addr);
            // SAFETY: as in `read`.
            let bounce = unsafe { &mut *core::ptr::addr_of_mut!(BOUNCE) };
            for (i, chunk) in blocks.chunks(BOUNCE_BLOCKS).enumerate() {
                let len = chunk.len() * BLOCK_LEN;
                for (b, dst) in chunk.iter().zip(bounce.0[..len].chunks_exact_mut(BLOCK_LEN)) {
                    dst.copy_from_slice(&b.contents);
                }
                sd.write_blocks(start_block_idx.0 + (i * BOUNCE_BLOCKS) as u32, &bounce.0[..len])?;
            }
            Ok(())
        })?;
        if let Err(e) = r {
            log_transfer_error("write", start_block_idx.0, n, e);
        }
        r
    }

    fn num_blocks(&self) -> Result<BlockCount, Self::Error> {
        crate::flpr_mux::with_storage(|sd| sd.num_blocks())?.map(BlockCount)
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
    fn mount(card: &'static Sd) -> Option<Storage> {
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
        report_cluster_size();
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
            sideload_ids: Vec::new(),
            next_sideload: SIDELOAD_ID_BASE as u32,
            open_route: None,
            open_map: None,
            open_map_name: None,
            map_extents: None,
            map_boot_fault: None,
            open_track: None,
            pending_save: None,
            ride_saved: false,
            open_object: None,
            map_name: String::new(),
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
    /// Fills the **caller's** catalog rather than returning one: a `Vec<RouteSummary, MAX_ROUTES>`
    /// is ~6 KB, and a by-value return keeps the builder's copy and the caller's alive in one
    /// frame (measured 12.3 KB in `ride::load_routes`, 2026-07-24 — two copies of exactly this).
    pub fn scan_routes_into(&mut self, catalog: &mut Vec<RouteSummary, MAX_ROUTES>) {
        catalog.clear();
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
        let Some(dir) = self.routes_dir else { return };

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
            let Some(id) = route_name::uploaded_id(n.base_name(), n.extension()).or_else(|| self.sideload_id(n)) else {
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
    }

    /// Each catalog entry's object id, parallel to the catalog [`scan_routes_into`](Storage::scan_routes_into)
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
    /// Fills the **caller's** catalog rather than returning one — see [`scan_routes_into`] for
    /// why (the by-value return doubles the catalog on the caller's frame).
    ///
    /// [`scan_routes_into`]: Self::scan_routes_into
    pub fn scan_rides_into(&mut self, catalog: &mut Vec<obc_app::RideSummary, UI_RIDES_CAP>) {
        catalog.clear();
        self.ride_files.clear();
        self.ride_ids.clear();
        let synced = self.load_synced_set();
        let Some(dir) = self.tracks_dir else { return };

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
                    let sum = obc_app::RideSummary::from_info(&info, synced.contains(*id), synced.synced_at(*id));
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
    }

    /// Each ride catalog entry's durable object id, parallel to the catalog
    /// [`scan_rides_into`](Storage::scan_rides_into) last returned — the second argument to
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

    /// Record a batch of ride ids as synced at `synced_at` in **one** sidecar read-modify-write (the
    /// `ackRides` command can carry dozens of ids — a per-id rewrite would be that many file
    /// round-trips). Returns how many ids were **newly** flagged; `0` = the sidecar was not
    /// rewritten. Ids already flagged (or dropped by a full set) count as nothing-new.
    pub fn mark_rides_synced(&mut self, ids: impl Iterator<Item = u16>, synced_at: u32) -> usize {
        let mut set = self.load_synced_set();
        // The merge rule itself lives in `SyncedRides::ack` (obc-app), where it is host-tested:
        // add-only, idempotent, first-stamp-wins — which is what lets a desktop ack and a phone
        // heal commute (E1, #911). This function only owns the read-modify-write around it.
        let added = set.ack(ids, synced_at);
        if added > 0 {
            let _ = self.write_synced_set(&set);
        }
        added
    }

    /// The **full** compact ride-retention inventory (finding #876-2): every synced ride's
    /// `id + synced + synced_at`, up to [`MAX_RIDES`], read straight off the synced-set sidecar — so
    /// the auto-delete sweep + the eager `synced_at` stamp reach a synced+expired ride even when it
    /// sits below the newest-[`UI_RIDES_CAP`] the Rides menu shows. An unsynced ride carries no
    /// retention state and is never in the set. One file read; the board hands this to
    /// [`App::set_ride_retention_inventory`](obc_app::App::set_ride_retention_inventory) after each
    /// ride rescan.
    pub fn ride_retention_inventory(&self) -> Vec<obc_app::RideRetentionRecord, MAX_RIDES> {
        let mut out = Vec::new();
        for (id, synced_at) in self.load_synced_set().entries() {
            let _ =
                out.push(obc_app::RideRetentionRecord { id: u64::from(id), synced: true, synced_at_utc: synced_at });
        }
        out
    }

    /// Stamp a **legacy** synced-without-timestamp ride's `synced_at` (auto-expiry epic #638, S3 —
    /// the sweep's [`StampRideSynced`](obc_app::HostCommand::StampRideSynced) countdown-start). Only
    /// ever fills a `0` stamp; rewrites the sidecar only when it changed.
    pub fn stamp_ride_synced_at(&mut self, id: u16, synced_at: u32) {
        let mut set = self.load_synced_set();
        if set.stamp_synced_at(id, synced_at) {
            let _ = self.write_synced_set(&set);
        }
    }

    /// Retire ride `id`'s synced flag from the sidecar (a deleted ride — ids never reuse, so this is
    /// belt-and-braces tidiness). Rewrites the sidecar only when the flag was present.
    pub fn forget_ride_synced(&mut self, id: u16) {
        let mut set = self.load_synced_set();
        if set.remove(id) {
            let _ = self.write_synced_set(&set);
        }
    }

    /// Read the route-CRC sidecar (`/routes/ROUTES.CRC`) into a [`RouteCrcs`] map (epic #632 item 6).
    /// A missing, torn, or malformed sidecar decodes to the **empty** map (every route serves
    /// `0 = unknown`) — never a panic (the codec + torn-line semantics are host-tested in
    /// `obc-app::settings`). One file read.
    pub fn load_route_crcs(&self) -> RouteCrcs {
        self.load_crc_sidecar(ROUTE_CRCS)
    }

    fn load_crc_sidecar(&self, name: &str) -> RouteCrcs {
        let Some(dir) = self.routes_dir else { return RouteCrcs::new() };
        let Ok(file) = self.vmgr.open_file_in_dir(dir, name, Mode::ReadOnly) else {
            return RouteCrcs::new(); // absent = no CRC known
        };
        let mut buf = [0u8; ROUTE_CRCS_MAX_LEN];
        let n = self.vmgr.read(file, &mut buf).unwrap_or(0);
        let _ = self.vmgr.close_file(file);
        decode_route_crcs(&buf[..n])
    }

    /// Retire route `id`'s CRC entry from the sidecar (a deleted route — ids never reuse, so this is
    /// belt-and-braces tidiness). Rewrites only when the entry was present.
    pub fn forget_route_crc(&mut self, id: u16) {
        let mut map = self.load_route_crcs();
        if map.remove(id) {
            self.write_route_crcs(&map);
        }
    }

    /// The centralized CRC-framed sidecar rewrite (finding #876-5): open (truncating) → write →
    /// flush → close, checking **every** step, and returning the first that failed. The file is
    /// always flushed + closed even after a write error so the open-file budget is never leaked, and
    /// the failing step is named in the log (the consequence line lives at the call site, which knows
    /// whether the failure is safe-by-design or must surface to the phone). Replaces the
    /// copy-pasted `open → write(ignore err) → flush(ignore) → close(ignore)` blocks whose swallowed
    /// flush/close error made a torn persist look like success.
    fn rewrite_sidecar(
        &mut self,
        dir: Option<RawDirectory>,
        name: &str,
        bytes: &[u8],
    ) -> Result<(), SidecarWriteError> {
        let Some(dir) = dir else { return Err(SidecarWriteError::Open) };
        let file = self
            .vmgr
            .open_file_in_dir(dir, name, Mode::ReadWriteCreateOrTruncate)
            .map_err(|_| SidecarWriteError::Open)?;
        // Write, then flush + close **unconditionally** to release the handle, then report the first
        // failing step.
        let wrote = self.vmgr.write(file, bytes).is_ok();
        let flushed = self.vmgr.flush_file(file).is_ok();
        let closed = self.vmgr.close_file(file).is_ok();
        let step = if !wrote {
            Some(SidecarWriteError::Write)
        } else if !flushed {
            Some(SidecarWriteError::Flush)
        } else if !closed {
            Some(SidecarWriteError::Close)
        } else {
            None
        };
        match step {
            Some(e) => {
                defmt::warn!(
                    "SD: sidecar rewrite failed (write {=bool} flush {=bool} close {=bool})",
                    wrote,
                    flushed,
                    closed
                );
                Err(e)
            }
            None => Ok(()),
        }
    }

    /// Overwrite the route-CRC sidecar (truncating). A write failure is warned, not fatal — the
    /// worst case is a route serves `0 = unknown` and re-fills lazily next list build, never a crash.
    pub fn write_route_crcs(&mut self, map: &RouteCrcs) {
        if !self.write_crc_sidecar(ROUTE_CRCS, map) {
            defmt::warn!("SD: route-crc sidecar not persisted — a route may serve crc 0 next list build");
        }
    }

    fn write_crc_sidecar(&mut self, name: &str, map: &RouteCrcs) -> bool {
        let mut buf = [0u8; ROUTE_CRCS_MAX_LEN];
        let n = encode_route_crcs(map, &mut buf);
        self.rewrite_sidecar(self.routes_dir, name, &buf[..n]).is_ok()
    }

    /// Read the route-retention sidecar (`/routes/ROUTES.RET`) into a [`RouteRetentionStore`]
    /// (auto-expiry epic #638, S3). A missing, torn, or malformed sidecar decodes to the **empty**
    /// store (every route reads `Never` → nothing deletes) — never a panic (the codec + torn-line
    /// semantics are host-tested in `obc-app::retention`). One file read.
    pub fn load_route_retention(&self) -> RouteRetentionStore {
        let Some(dir) = self.routes_dir else { return RouteRetentionStore::new() };
        let Ok(file) = self.vmgr.open_file_in_dir(dir, ROUTE_RETENTION, Mode::ReadOnly) else {
            return RouteRetentionStore::new(); // absent = nothing has retention (all Never)
        };
        let mut buf = [0u8; ROUTE_RETENTION_MAX_LEN];
        let n = self.vmgr.read(file, &mut buf).unwrap_or(0);
        let _ = self.vmgr.close_file(file);
        decode_route_retention(&buf[..n])
    }

    /// Each catalog entry's retention meta, parallel to [`route_ids`](Storage::route_ids) — the
    /// second argument to [`App::set_routes_with_meta`](obc_app::App::set_routes_with_meta) so the
    /// auto-expiry sweep reads device-truth retention alongside the summaries. One sidecar read.
    pub fn route_retention_metas(&self) -> Vec<RouteRetentionMeta, MAX_ROUTES> {
        let store = self.load_route_retention();
        let mut out = Vec::new();
        for &id in &self.route_ids {
            let _ = out.push(store.get(id));
        }
        out
    }

    /// Set route `id`'s retention **level** in the sidecar (the app's `setRouteRetention` command,
    /// §4.4 cmd 6 / epic #638 S4) **without touching `last_used`** — changing retention never resets
    /// the usage clock. Read-modify-write within the call; persists (truncating rewrite) only when the
    /// level actually changed, and returns whether it did so the caller bumps the route store revision
    /// on a real change only (setting the same value twice is a no-op — the idempotence pin). A row
    /// that reverts to `Never` with `last_used == 0` is dropped (the empty default reads that way).
    /// Returns `Ok(true)` on a durable change, `Ok(false)` when the value was already that level (a
    /// no-op — nothing to persist), or `Err` when the sidecar rewrite did not reach the card
    /// (finding #876-5). The caller (`ObjectStore::set_route_retention`) bumps the revision and
    /// replies `ok` **only** on `Ok(true)`; an `Err` is surfaced as `command` `Error`, never a false
    /// `ok`.
    pub fn set_route_retention_level(&mut self, id: u16, retention: Retention) -> Result<bool, SidecarWriteError> {
        let mut store = self.load_route_retention();
        let meta = RouteRetentionMeta { retention, last_used_utc: store.get(id).last_used_utc };
        if !store.set(id, meta) {
            return Ok(false); // already that level — durable by definition, nothing rewritten
        }
        self.write_route_retention(&store)?;
        Ok(true)
    }

    /// Stamp route `id`'s `last_used` in the sidecar (auto-expiry epic #638, S3 — the sweep's
    /// clock-start / active re-stamp, the once-per-activation stamp, and the upload-commit stamp),
    /// keeping its retention level. Persists only when it changed.
    pub fn stamp_route_last_used(&mut self, id: u16, utc: u32) {
        let mut store = self.load_route_retention();
        if store.stamp_last_used(id, utc) {
            // Best-effort: a torn stamp is safe (the route keeps its old/`0` `last_used` and is
            // re-stamped or left unexpired next sweep — never a wrong deletion). The helper logs.
            let _ = self.write_route_retention(&store);
        }
    }

    /// Retire route `id`'s retention entry from the sidecar (a deleted route — ids never reuse, so
    /// belt-and-braces). Rewrites only when the entry was present (setting a route back to the
    /// default drops its row).
    pub fn forget_route_retention(&mut self, id: u16) {
        let mut store = self.load_route_retention();
        if store.set(id, RouteRetentionMeta::default()) {
            let _ = self.write_route_retention(&store); // best-effort tidy; the helper logs a failure
        }
    }

    /// Overwrite the route-retention sidecar (truncating), returning whether the whole rewrite —
    /// open, write, flush, close — reached the card (finding #876-5). A torn write is safe by design
    /// (a route reads `Never` next list build → nothing deletes), so the stamp/forget callers ignore
    /// the result; only [`set_route_retention_level`](Storage::set_route_retention_level) propagates
    /// it so `setRouteRetention` never claims `ok` ahead of durability.
    pub fn write_route_retention(&mut self, store: &RouteRetentionStore) -> Result<(), SidecarWriteError> {
        let mut buf = [0u8; ROUTE_RETENTION_MAX_LEN];
        let n = encode_route_retention(store, &mut buf);
        self.rewrite_sidecar(self.routes_dir, ROUTE_RETENTION, &buf[..n])
    }

    /// Read the card-resident store-epoch nonce (`/EPOCH.OBE`, protocol v2 #632 item 5 / #776), or
    /// `None` when the file is **absent** (a fresh/foreign-formatted card) or torn/foreign — "no
    /// epoch", which the boot mint rule ([`obc_app::store_meta::store_epoch_mint`]) treats as clause 1
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

    /// Overwrite the synced-ride sidecar (truncating), returning whether the whole rewrite reached
    /// the card (finding #876-5). A torn write is safe by design — a ride reads as unsynced next boot
    /// (the safe default: never deleted, and the phone re-acks on reconnect) — so the synced-flag
    /// callers treat it best-effort; the helper logs the failing step.
    fn write_synced_set(&mut self, set: &SyncedRides) -> Result<(), SidecarWriteError> {
        let mut buf = [0u8; SYNCED_RIDES_MAX_LEN];
        let n = encode_synced_rides(set, &mut buf);
        self.rewrite_sidecar(self.tracks_dir, SYNCED_SET, &buf[..n])
    }

    /// Build the stored ride `id`'s recorded-track elevation [`Profile`] — the Ride detail's band
    /// fill (epic #678 T2 / #680), answering
    /// [`App::ride_track_request`](obc_app::App::ride_track_request). Resolves the id
    /// through the scan-parallel [`ride_ids`](Storage::ride_ids)/[`ride_files`](Storage::ride_files)
    /// tables and streams the `RD{id}.ORD` once through the shared `ride_elevation_profile`
    /// (~448 B per SD read, no whole-track buffer — the ~36 KB stack budget's discipline; the
    /// returned `Profile` is the nrf-mem ~3 KB build). An in-flight BLE download's open handle is
    /// read through rather than re-opened (embedded-sdmmc refuses a second open, #480), exactly as
    /// [`scan_rides_into`](Storage::scan_rides_into) does. `None` = unknown id / unopenable / torn file —
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

    /// Open the card's **selected** map and hold it open for the session, so the map can **stream**
    /// from it (issue #37) rather than be read resident into RAM. Returns the file length on
    /// success, or `None` if the card holds no map / it can't be opened. Call once at startup;
    /// [`map_source`](Self::map_source) then hands out a reader over the open handle.
    ///
    /// Before #927 this was "the first `*.obcm` the directory scan yields", which was an answer only
    /// while a card could hold one map. Now it is, in order:
    ///
    /// 1. the map [`MAP_SELECTED`] names, if it is still on the card;
    /// 2. else the newest readable **volume set** (`MS{id}.OBS`), comparing only MS ids;
    /// 3. else the newest readable single-file upload (`MP{id}.OBM`), comparing only MP ids;
    /// 4. else the first readable map of any kind (a side-loaded `.obcm`);
    /// 5. else the first map at all — so a card holding only a wrong-version map still reaches the
    ///    **MAP UNREADABLE** fault screen rather than the indistinguishable **NO MAP** one.
    ///
    /// Every way this returns `None` also records [`boot_fault`](Self::boot_fault), by the one rule
    /// in [`obc_app::boot_fault`]: giving up is not the same as an empty card, and a rider whose map
    /// is sitting in the root must not be told to go and add one. That covers both open failures
    /// and — via `scan_maps_into`'s count — files the catalog never
    /// saw because their header would not parse.
    pub fn open_map(&mut self) -> Option<u32> {
        if let Some((_, len)) = self.open_map {
            return Some(len);
        }
        let mut maps: Vec<MapSummary, MAX_MAPS> = Vec::new();
        let unlistable = self.scan_maps_into(&mut maps);
        let Some(keep) = choose_map_index(&maps) else {
            // An empty *catalog* is not an empty *card*: a torn or unopenable map file is dropped by
            // the scan and still sits in the root under a name the rider recognises. The rule gives
            // NO MAP only when nothing at all was found, exactly as before.
            self.map_boot_fault = Some(obc_app::boot_fault(&map_choices(&maps), unlistable));
            return None;
        };
        let chosen = maps[keep].clone();
        let (name, display, entry_block, entry_offset) =
            (chosen.file.clone(), chosen.name.clone(), chosen.entry_block, chosen.entry_offset);
        defmt::info!(
            "SD: {=usize} map(s) on the card; loading {} (v{=u8}, {=u64} B)",
            maps.len(),
            defmt::Debug2Format(&name),
            chosen.obcm_version,
            chosen.byte_len
        );
        self.map_name = display;
        // Both failures below are about a map the catalog *listed*: its header read fine minutes ago
        // and the file is in the root under a name the rider knows. **MAP UNREADABLE** is the honest
        // report — NO MAP would send them looking for a file that is right there.
        let Ok(file) = self.vmgr.open_file_in_dir(self.root, &name, Mode::ReadOnly) else {
            defmt::warn!("SD: the chosen map {} is on the card but will not open", defmt::Debug2Format(&name));
            self.map_boot_fault = Some(obc_app::boot_fault(&map_choices(&maps), unlistable));
            return None;
        };
        let len = self.vmgr.file_length(file).unwrap_or(0);
        if len == 0 {
            defmt::warn!("SD: the chosen map {} opened with zero length", defmt::Debug2Format(&name));
            let _ = self.vmgr.close_file(file);
            self.map_boot_fault = Some(obc_app::boot_fault(&map_choices(&maps), unlistable));
            return None;
        }
        self.open_map = Some((file, len));
        self.open_map_name = Some(name);
        self.build_map_extents(entry_block, entry_offset, file, len);
        // Last, and only on the success path: the map that just opened is the survivor, so the
        // uploads it superseded can go. Every early return above leaves the card untouched — a map
        // that could not be opened has proved nothing, and deleting its predecessor on the strength
        // of a failed open is how a rider ends up with no map at all.
        self.retire_superseded_maps(&maps, Some(keep));
        Some(len)
    }

    /// Which boot fault to put on glass when [`map_source`](Storage::map_source) hands out nothing.
    ///
    /// **NO MAP** unless [`open_map`](Storage::open_map) found a map-named file and could not stream
    /// from it — see [`map_boot_fault`](Storage::map_boot_fault) and `obc_app::boot_fault`, where the
    /// rule lives and is tested.
    pub fn boot_fault(&self) -> obc_app::BootFault {
        self.map_boot_fault.unwrap_or(obc_app::BootFault::NoMap)
    }

    /// Delete the uploaded maps the one just opened superseded — the card side of the **one map**
    /// rule (#992). Returns how many were reclaimed.
    ///
    /// Which files are eligible is [`obc_app::is_superseded_upload`]'s decision, tested where tests
    /// run; this is the binding, and it adds one board-only guard the pure rule cannot know about:
    /// **never delete the file that is open**. That cannot trigger given the caller — `keep` is the
    /// open one — and it is here because the cost of the two states disagreeing some day is a
    /// deleted file under a live handle.
    ///
    /// **Timing, deliberately: at open, not at commit.** A map upload lands while the *previous*
    /// map is held open for the session, and the renderer streams from that handle — so the moment
    /// the replacement commits is exactly the moment its predecessor cannot be touched. This runs at
    /// the next `open_map` instead, which is also when the new map takes effect. The consequence,
    /// stated rather than discovered: between an upload and the next boot the card carries **both**
    /// maps, and the free-space guard at announce (§4.1 rule 2) sees it — a replacement whose old
    /// and new copies do not fit together is refused until the device is restarted once.
    ///
    /// **One delete removes the whole prefix.** A superseded volume set is a manifest plus up to 32
    ///
    /// `keep` is `None` only when nothing loaded, and then nothing is superseded.
    fn retire_superseded_maps(&mut self, maps: &[MapSummary], keep: Option<usize>) -> usize {
        let choices = map_choices(maps);
        // Collected first because the scan borrow and the delete `&mut` cannot overlap.
        let mut doomed: Vec<ShortFileName, MAX_MAPS> = Vec::new();
        let Some(keeper) = keep else { return 0 };
        for (i, m) in maps.iter().enumerate() {
            if obc_app::is_superseded_upload(&choices, keeper, i) && !self.map_file_is(&m.file) {
                let _ = doomed.push(m.file.clone());
            }
        }
        let mut retired = 0;
        for name in doomed {
            match self.vmgr.delete_file_in_dir(self.root, &name) {
                Ok(()) => {
                    defmt::info!("SD: retired superseded map {}", defmt::Debug2Format(&name));
                    retired += 1;
                }
                Err(e) => defmt::warn!(
                    "SD: could not retire the superseded map {} ({}) — the next boot will try again",
                    defmt::Debug2Format(&name),
                    defmt::Debug2Format(&e)
                ),
            }
        }
        retired
    }

    /// Scan the card root into the **map catalog** (issue #927) — the "which maps are on this card"
    /// surface the selection rule, the id allocator, and (later) the device dashboard all read.
    ///
    /// Every fact in a [`MapSummary`] is **derived from the card**: the filename and its long-name
    /// stem come from the directory entry, the byte length from the entry too, and the OBCM version
    /// plus the global bbox from **one 40-byte header read** per map. There is no sidecar and nothing
    /// to keep in sync — which is also why the catalog carries no build date and no display name
    /// beyond the filename: the OBCM header has neither, and no channel exists to deliver them (see
    /// the module notes and #915).
    ///
    /// A file whose magic isn't `OBCM` is **not a map**: that is precisely the signature a torn
    /// upload leaves (the held-back magic never patched in), so the scan is what makes an interrupted
    /// transfer invisible instead of a half-map the renderer would try to parse.
    ///
    /// Returns how many map-named entries were dropped that way — the count
    /// [`obc_app::boot_fault`] needs, and the *only* thing that looks at them. They stay out of the
    /// catalog for every other consumer (the renderer must not parse one; `next_map_id_from_scan`
    /// must stay free to reuse a torn upload's id), but a dropped entry is still a file the rider
    /// sees in the card root, so a boot that finds nothing to stream from must say **MAP
    /// UNREADABLE** rather than **NO MAP**.
    ///
    /// **A map is one `.OBM` file.** The volume set that used to complicate this — one logical map
    /// listed from a manifest, its shards excluded by name so a shard opened alone could not be
    /// mistaken for a map — is retired with OBCM v14 (#1420), and with it the per-shard opens this
    /// scan used to pay.
    pub fn scan_maps_into(&self, out: &mut Vec<MapSummary, MAX_MAPS>) -> usize {
        out.clear();
        let mut unlistable = 0usize;
        let selected = self.load_selected_map();
        // Two phases because the `iter_dir_lfn` callback borrows the manager and the identity read
        // opens a file.
        let mut entries: Vec<(ShortFileName, String<24>, embedded_sdmmc::BlockIdx, u32, u32), MAX_MAPS> = Vec::new();
        self.iter_dir_lfn(self.root, |e, long| {
            if !is_map_entry(e, long) {
                return;
            }
            let _ =
                entries.push((e.name.clone(), map_display_name(&e.name, long), e.entry_block, e.entry_offset, e.size));
        });
        for (file, name, entry_block, entry_offset, byte_len) in entries {
            let Some(obcm_version) = self.map_identity(&file) else {
                unlistable += 1;
                continue;
            };
            let (id, byte_len) = (uploaded_map_id(&file), byte_len as u64);
            let selected = selected
                .as_ref()
                .is_some_and(|s| file.base_name() == s.base_name() && file.extension() == s.extension());
            let entry = MapSummary { id, file, name, byte_len, obcm_version, selected, entry_block, entry_offset };
            if out.push(entry).is_err() {
                defmt::warn!("SD: more than {=usize} maps on the card — the rest are not listed", MAX_MAPS);
                break;
            }
        }
        unlistable
    }

    /// One map's OBCM version from its 40-byte header, or `None` when the file is shorter than a
    /// header, unreadable, or doesn't carry the `OBCM` magic (a torn upload, or clutter that happens
    /// to sit on an `.OBM`/`.obcm` name).
    ///
    /// It used to return the header bbox too — the map's footprint, for coverage checks that were
    /// never written. The one consumer was `open_volume_set`, which compared it against the
    /// manifest's assembly bbox before mounting; that mount is gone (FS7.5-c2, #1420), and a field
    /// no code reads is not a catalog, it is a header read nobody looks at.
    ///
    /// The **version is returned, not checked**: a map built for another OBCM version is still a map
    /// and still belongs in the catalog — the consumer decides. Only the magic gates membership.
    ///
    /// The currently-open map is read **through its existing handle**: embedded-sdmmc refuses every
    /// second open of an open file (`FileAlreadyOpen`), which would otherwise drop the loaded map out
    /// of its own catalog — the same trap `route_object_info` documents (issue #480).
    fn map_identity(&self, name: &ShortFileName) -> Option<u8> {
        let mut header = [0u8; obc_formats::obcm::HEADER_LEN];
        let read_through = |src: &dyn ByteSource, header: &mut [u8; obc_formats::obcm::HEADER_LEN]| {
            (src.len() as usize >= header.len()) && src.read_at(0, header).is_ok()
        };
        let ok = match self.open_map {
            Some((f, len)) if self.map_file_is(name) => {
                read_through(&SdByteSource::new(&self.vmgr, f, len), &mut header)
            }
            _ => {
                let file = self.vmgr.open_file_in_dir(self.root, name, Mode::ReadOnly).ok()?;
                let len = self.vmgr.file_length(file).unwrap_or(0);
                let ok = read_through(&SdByteSource::new(&self.vmgr, file, len), &mut header);
                let _ = self.vmgr.close_file(file);
                ok
            }
        };
        if !ok || header[0..4] != obc_formats::obcm::MAGIC {
            return None;
        }
        Some(header[4])
    }

    /// Whether `name` is the map file currently held open — the guard that routes
    /// [`map_identity`](Self::map_identity) through the live handle instead of a refused second open.
    fn map_file_is(&self, name: &ShortFileName) -> bool {
        self.open_map.is_some() && self.open_map_name.as_ref() == Some(name)
    }

    /// **Read-only since FS7.5-c3b, and kept until FS11 (#1393) for one reason: cards in the field.**
    ///
    /// Nothing writes `MAP.SEL` any more — `save_selected_map` went with the v1 command surface that
    /// set it (`FLAT_Store_Protocol.md` §5.2.2), and the flat store has no such file. A read whose
    /// writer is gone would normally go too, by the standing rule about never-exercised paths. This
    /// one stays because the rule is about capabilities nobody exercises, and this file **is**
    /// exercised: a FAT card that has been in a device carries a `MAP.SEL` an earlier firmware
    /// wrote, and a rider who chose a map expects that choice to survive the update that removed the
    /// way to change it. Dropping the read would silently re-pick their map.
    ///
    /// **It dies with this module in FS11**, which retires the FAT read path entirely; there is no
    /// second decision to make and no separate follow-up to file.
    ///
    /// The card's recorded map selection ([`MAP_SELECTED`]), or `None` for absent / torn / a name
    /// this device would never have written — all of which mean **no preference**.
    pub fn load_selected_map(&self) -> Option<ShortFileName> {
        let name = ShortFileName::create_from_str(MAP_SELECTED).ok()?;
        let file = self.vmgr.open_file_in_dir(self.root, &name, Mode::ReadOnly).ok()?;
        let mut buf = [0u8; obc_app::store_meta::SELECTED_MAP_LEN];
        let n = self.vmgr.read(file, &mut buf).unwrap_or(0);
        let _ = self.vmgr.close_file(file);
        ShortFileName::create_from_str(obc_app::store_meta::decode_selected_map(&buf[..n])?).ok()
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
                // `MAP_EXTENTS`). SAFETY: `open_map` opens one map once per boot, so this is the
                // sole write to the slot; it must never be overwritten after the `'static`
                // reference escapes.
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
    ///
    /// It was generic over the table's capacity while a mounted set had eleven 64-run tables beside
    /// the map's 128-run one. One map, one table, one capacity (FS7.5-c2): the parameter had exactly
    /// one instantiation left. `ExtentTableWithCapacity`'s own const generic is now in the same
    /// position one layer down — noted for FS11, which is where `fat_extents` is scheduled to go.
    fn verify_extents(&self, table: &ExtentTable, file: RawFile, len: u32) -> bool {
        let slow = Source::new(&self.vmgr, file, len);
        let fast = ExtentSource::new(self.card, table);
        let mut a = [0u8; 64];
        let mut b = [0u8; 64];
        for off in [0, len.saturating_sub(a.len() as u32)] {
            let n = a.len().min(len as usize);
            let off = u64::from(off);
            if slow.read_at(off, &mut a[..n]).is_err() || fast.read_at(off, &mut b[..n]).is_err() || a[..n] != b[..n] {
                return false;
            }
        }
        true
    }

    /// A [`ByteSource`](obc_formats::io::ByteSource) over the open map file, for reading the header
    /// ([`obc_reader::MapTables::parse`]) or building a per-frame [`Reader`](obc_reader::Reader). `None` if
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

    /// A **`'static`** [`ByteSource`](obc_formats::io::ByteSource) over the open map, or `None` when
    /// the map reads through the FAT-seek path — see [`MAP_STATIC_SOURCE`] for why the seek path is
    /// not offered and why this exists beside [`map_source`](Self::map_source).
    ///
    /// Call **once**, at boot, after [`open_map`](Self::open_map). The slot is written once per boot
    /// and the reference handed out is the only one; a second call would overwrite bytes a live
    /// `&'static` is reading.
    #[cfg(has_nav)]
    pub fn static_map_source(&self) -> Option<&'static dyn ByteSource> {
        let table = self.map_extents?;
        // SAFETY: the one call site is `main`'s boot path, before anything is spawned. The retained
        // map handle pins the FAT chain `table` describes for the session, and `ExtentSource` owns
        // nothing to drop.
        Some(unsafe {
            crate::init_static(core::ptr::addr_of_mut!(MAP_STATIC_SOURCE), ExtentSource::new(self.card, table))
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

    /// A [`ByteSource`](obc_formats::io::ByteSource) over the active route's open file, for opening a
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
    /// ~12.3 KB index returned through `Option<RouteIndex>` rode the stack at the ride pass's deepest
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

    /// A [`ByteSink`](obc_formats::io::ByteSink) over the open nav-route file — what
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
    /// filled — call after the post-plan [`scan_routes_into`](Storage::scan_routes_into). `None` when the
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
    /// same edge).
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
        if let Some((i, f, len)) = self.open_route {
            if self.route_files.get(i) == Some(name) {
                let info = RouteObjectInfo::read(&SdByteSource::new(&self.vmgr, f, len)).ok()?;
                return Some((len, info));
            }
        }
        self.with_routes_object(name, |src, len| Some((len, RouteObjectInfo::read(src).ok()?)))
    }

    /// Read a `/routes` object through a matching retained detail handle, or a scoped fresh handle.
    /// The callback never outlives this call and a fresh handle is closed on success or refusal.
    fn with_routes_object<T>(
        &self,
        name: &ShortFileName,
        read: impl FnOnce(&Source<'_>, u32) -> Option<T>,
    ) -> Option<T> {
        if let Some((open_name, file, len)) = &self.open_object {
            if name == open_name {
                return read(&SdByteSource::new(&self.vmgr, *file, *len), *len);
            }
        }
        let file = self.vmgr.open_file_in_dir(self.routes_dir?, name, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0);
        let out = read(&SdByteSource::new(&self.vmgr, file, len), len);
        let _ = self.vmgr.close_file(file);
        out
    }

    /// Whether a catalog file is an **aborted commit** — the held-back magic still zeroed
    /// because the commit's final patch never ran. Only that exact signature is sweepable; a
    /// merely unreadable file (a transient bus glitch) must be kept.
    pub fn is_aborted_commit(&self, name: &ShortFileName) -> bool {
        self.with_routes_object(name, |src, _| {
            let mut magic = [0u8; 4];
            ByteSource::read_at(src, 0, &mut magic).ok()?;
            Some(magic == [0u8; 4])
        })
        .unwrap_or(false)
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

    /// Sweep abandoned map uploads from the card root (issue #927): delete every `MP*.OBM` whose
    /// held-back magic was never patched in. Run once at boot, the map twin of the route/trip
    /// `is_aborted_commit` sweep — without it an interrupted transfer's hundreds of megabytes would
    /// sit on the card forever, invisible to every catalog that could explain them. Returns how many
    /// were reclaimed.
    pub fn sweep_aborted_maps(&mut self) -> usize {
        let mut candidates: Vec<ShortFileName, MAX_MAPS> = Vec::new();
        self.iter_dir_lfn(self.root, |e, long| {
            if is_map_entry(e, long) && uploaded_map_id(&e.name).is_some() {
                let _ = candidates.push(e.name.clone());
            }
        });
        let mut swept = 0;
        for name in candidates {
            // Only the exact torn signature is sweepable: a *readable* header keeps the file, and so
            // does an unreadable one whose magic is intact (a transient bus glitch, or a map for
            // another OBCM version — neither is ours to delete).
            if self.map_identity(&name).is_some() || !self.is_zero_magic_root(&name) {
                continue;
            }
            if self.vmgr.delete_file_in_dir(self.root, &name).is_ok() {
                defmt::info!("SD: swept abandoned map upload {}", defmt::Debug2Format(&name));
                swept += 1;
            }
        }
        swept
    }

    /// Whether a card-root file's first four bytes are zeros — the held-back-magic signature of a
    /// commit that never finished. The root twin of [`is_aborted_commit`](Self::is_aborted_commit);
    /// an unreadable or short file is **not** claimed (a bus glitch must never green-light a
    /// delete).
    ///
    /// It used to answer through `obc_app::RootMagic`, a three-way verdict the *volume-set* sweep
    /// needed all of — a file that opened and held fewer than four bytes was one this device created
    /// and did not get to write, and had to be told apart from one the card refused. With the set
    /// gone there is one caller and it wants a bool, so the three-way answer went with the sweep that
    /// asked for it rather than staying behind as a shape nothing reads.
    fn is_zero_magic_root(&self, name: &ShortFileName) -> bool {
        let Ok(file) = self.vmgr.open_file_in_dir(self.root, name, Mode::ReadOnly) else {
            return false;
        };
        let mut magic = [0u8; 4];
        let read = self.vmgr.read(file, &mut magic);
        let _ = self.vmgr.close_file(file);
        matches!(read, Ok(4)) && magic == [0, 0, 0, 0]
    }

    /// Whether a staged `/UPDATE.BIN` exists in the card root — the `installFw` `noStaged` cheap
    /// existence check (spec §4.4). Presence only (a directory scan, no read): the full CRC validation
    /// is the on-device confirm flow's, never a BLE command handler's.
    pub fn has_update_bin(&self) -> bool {
        ShortFileName::create_from_str(UPDATE_BIN).ok().and_then(|n| self.find_root_entry(&n)).is_some()
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

    /// The staging scan (#619 §1, signed in #997): find `UPDATE.BIN` in the card root, decode +
    /// validate its OBCU header, run the **full CRC-32 pass and the Ed25519 verification** over the
    /// image body in one pass through the byte source, gate the size, and resolve the whole-file
    /// extent chain (spec §2.3 — the header is part of the chain). Typed errors surface verbatim to
    /// the debug link now and S5's UI later. Read-only: a failed scan costs nothing.
    ///
    /// The trusted key is [`obc_dfu::RELEASE_PUBKEY`] — the production key compiled into this image
    /// (`firmware/obc-dfu/keys/obcu-release.pub`). This is the *only* place the firmware names it;
    /// `obc_dfu::armer::scan` takes it as a parameter so tests inject their own key without any
    /// build-flag surgery on the shipping path.
    pub fn dfu_scan_update(&mut self) -> Result<obc_dfu::StagedRef, ScanError> {
        let name = ShortFileName::create_from_str(UPDATE_BIN).map_err(|_| ScanError::Io)?;
        let Some((entry_block, entry_offset, len)) = self.find_root_entry(&name) else {
            return Err(ScanError::Missing);
        };
        let file = self.vmgr.open_file_in_dir(self.root, UPDATE_BIN, Mode::ReadOnly).map_err(|_| ScanError::Io)?;
        let mut stage = SdStage { vmgr: &self.vmgr, card: self.card, file, len, entry_block, entry_offset };
        // The CRC/signature staging buffer matches this module's transfer idiom
        // (`copy_with_held_magic`'s 512-byte stack chunk) — no new resident statics; the frame pops
        // with the scan, verifier state (~200 B) included.
        let mut chunk = [0u8; 512];
        let result = obc_dfu::armer::scan(&mut stage, &mut chunk, &obc_dfu::RELEASE_PUBKEY);
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
        // The snapshot is an **unsigned** container (`ImageHeader::unsigned`): the device cannot
        // re-create the release signature from slot bytes, and nothing verifies this file — it never
        // passes through `armer::scan`, and the bootloader's rollback path checks it by CRC. Writing
        // a signed *marker* with no trailer behind it would be a lie in a file `obc-mkimage inspect`
        // reads. The recorded `StagedRef` below carries the same unsigned header, so the installer's
        // header-equality check still matches the bytes on card.
        let snapshot_header = installed.unsigned();
        let ok = self.vmgr.write(file, &snapshot_header.encode()).is_ok()
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
        obc_dfu::StagedRef::new(snapshot_header, installed.image_len, crc, &extents[..count])
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
        SdByteSource::new(self.vmgr, self.file, self.len)
            .read_at(offset.into(), buf)
            .map_err(|_| obc_dfu::engine::IoError)
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
    !e.attributes.is_directory() && route_name::is_admitted(e.name.extension(), long)
}

/// The **durable ride object id** in a stored ride's filename — `RD{id}.ORD` → `id`. The same
/// durability contract as the routes': the app's synced-set and tombstones key on these ids across
/// device reboots.
pub fn stored_ride_id(name: &ShortFileName) -> Option<u16> {
    id_in_name(name, b"RD", b"ORD")
}

/// Whether a card-root directory entry belongs to the **map** catalog (issue #927): a side-loaded
/// `.obcm` (long-filename match, as before) **or** a device-received `*.OBM`.
///
/// The 8.3 arm is the whole answer to "the firmware can read long filenames but cannot create them".
/// embedded-sdmmc 0.9's `write_new_directory_entry` takes a `ShortFileName`, and the 4-char `.obcm`
/// extension needs an LFN it can't write — so, exactly as `/routes` already does for `.obcr`↔`.OBR`
/// and `_NAV.OBR`, the catalog accepts a dedicated 3-char twin the device *can* create. `OBM` rather
/// than the literal 8.3 truncation `OBC`, because `OBC` is what a host's LFN shortening produces for
/// **both** `.obcm` and `.obcr`: matching it would make a stray route in the card root look like a
/// map. `OBM` is unambiguous by construction.
///
/// Dot-prefixed clutter is excluded on both arms (a macOS `._x.OBM` AppleDouble also fails the
/// header read, but why open it at all).
///
/// The rule itself is `obc_app::classify_map_entry` — pure, and therefore tested where tests run;
/// this is the binding from a FAT directory entry to its three inputs. The board crate has no CI
/// test harness (bare metal), so nothing decidable may be decided here.
fn is_map_entry(e: &embedded_sdmmc::DirEntry, long: Option<&str>) -> bool {
    classify_entry(e, long) == obc_app::MapEntry::Map
}

/// Bind one FAT directory entry to the pure classifier.
fn classify_entry(e: &embedded_sdmmc::DirEntry, long: Option<&str>) -> obc_app::MapEntry {
    obc_app::classify_map_entry(&short_name_bytes(&e.name), long, e.attributes.is_directory())
}

/// A short name as the `BASE.EXT` bytes the pure classifier takes. Both halves come back
/// space-trimmed from embedded-sdmmc, so this is a straight join.
fn short_name_bytes(name: &ShortFileName) -> heapless::Vec<u8, 12> {
    let mut out: heapless::Vec<u8, 12> = heapless::Vec::new();
    let _ = out.extend_from_slice(name.base_name());
    let _ = out.push(b'.');
    let _ = out.extend_from_slice(name.extension());
    out
}

/// The **durable map object id** in a received map's filename — `MP{id}.OBM` → `id`, the same
/// filenames-guard-stored-ids rule (spec §4.1) as routes/rides/trips. `None` for a side-loaded
/// `.obcm`, which carries no id at all.
pub fn uploaded_map_id(name: &ShortFileName) -> Option<u16> {
    id_in_name(name, b"MP", b"OBM")
}

/// A map's display name: the **long** filename's stem when the entry has one (`freiburg.obcm` →
/// `freiburg`), else the 8.3 base with its padding trimmed (`MP7.OBM` → `MP7`). Truncated to the
/// [`MapSummary::name`] cap; never empty for a real entry.
fn map_display_name(short: &ShortFileName, long: Option<&str>) -> String<24> {
    let mut out: String<24> = String::new();
    if let Some(long) = long {
        let stem = long.rsplit_once('.').map(|(s, _)| s).unwrap_or(long);
        for ch in stem.chars() {
            if out.push(ch).is_err() {
                break;
            }
        }
    }
    if out.is_empty() {
        for &b in short.base_name().iter().take_while(|&&b| b != b' ') {
            if out.push(b as char).is_err() {
                break;
            }
        }
    }
    out
}

/// The scanned catalog as the host-tested classifiers want it — one [`obc_app::MapChoice`] per map,
/// in scan order, so an index into this is an index into `maps`.
///
/// **A volume set is never readable** since FS7.5-c2 (#1420): the reader mounts one OBCM file, so a
/// manifest names a shape this firmware has no way to open. It is still *listed* — the files are on
/// the card, and `readable: false` is exactly what makes `obc_app::boot_fault` answer MAP
/// UNREADABLE rather than sending a rider to look for a map that is right there.
///
/// A single map is readable when its OBCM version matches. The scan has already validated its
/// header and bbox; `open_map` adds the extent checks before rendering a pixel.
fn map_choices(maps: &[MapSummary]) -> Vec<obc_app::MapChoice, MAX_MAPS> {
    let mut choices: Vec<obc_app::MapChoice, MAX_MAPS> = Vec::new();
    for m in maps.iter().take(MAX_MAPS) {
        let _ = choices.push(obc_app::MapChoice {
            selected: m.selected,
            uploaded_id: m.id,
            readable: m.obcm_version == obc_formats::obcm::VERSION,
            set: false,
        });
    }
    choices
}

/// Pick the map [`Storage::open_map`] loads, by the rule documented there — the board's binding of
/// the host-tested [`obc_app::choose_map`] classifier to the scanned catalog. Returns the **index**
/// into `maps`, because the retirement rule that follows the choice
/// ([`obc_app::is_superseded_upload`]) is stated in terms of the same indices.
fn choose_map_index(maps: &[MapSummary]) -> Option<usize> {
    obc_app::choose_map(&map_choices(maps))
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
