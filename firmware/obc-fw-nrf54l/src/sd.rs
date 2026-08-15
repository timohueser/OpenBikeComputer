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
//!   `/WEATHER.A`, `/WEATHER.B` — OBCW generations. Uploads stream only into the inactive slot
//!                      with zero magic; full outer/internal/structural validation precedes the
//!                      magic-patch eligibility point. The active valid file is never truncated.
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
use obc_app::{GateOwner, Retention, SinkSession, TripInput, MAX_RIDES, MAX_ROUTES, MAX_TRIPS, UI_RIDES_CAP};
use obc_ble::ObjectType;
use obc_crc::Crc32;
use obc_dfu::armer::{ExtentsError, ScanError, StageIo};
use obc_formats::io::ByteSource;
use obc_formats::obcr::NAME_CAP;
use obc_map_scene::BBox;
use obc_route::{
    ride_elevation_profile, ride_preview_polyline, track_to_ride, Profile, RideInfo, RideStats, RouteIndex,
    RouteObjectInfo, RouteSummary, TripMeta, TripSummary,
};
use obc_storage::fat_extents::{
    BuildError, ExtentSource, ExtentSourceWithCapacity, ExtentTable, ExtentTableWithCapacity, SharedBlockDevice,
};
use obc_storage::weather::{self as weather_store, WeatherSlotIo};
use obc_storage::{route_name, trip_name};
use obc_storage::{SdByteSink, SdByteSource, SdTrackSink};
use obc_weather::{Candidate as WeatherCandidate, Slot as WeatherSlot, SlotSelection, SlotValidation};

mod rides;
pub(crate) use rides::Rides;
mod maps;
pub(crate) use maps::{MapTransfers, Maps};
mod routes;
pub(crate) use routes::{NavCommit, Routes};
mod trips;
pub(crate) use trips::Trips;

const RIDE_CATALOG_CAP: usize = MAX_RIDES;
pub(crate) type StoredRideCatalog = obc_storage::RideCatalog<RIDE_CATALOG_CAP>;
pub(crate) type StoredRouteCatalog = obc_storage::Catalog<u32, MAX_ROUTES, SIDELOAD_ID_BASE>;
type StoredTripCatalog = obc_storage::Catalog<Option<TripMeta>, MAX_TRIPS, SIDELOAD_ID_BASE>;
pub(crate) type UploadSession = SinkSession<RawFile>;
pub(crate) type DownloadSession = SinkSession<RawFile>;

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

/// Free bytes a map upload must leave on the card after itself (issue #927). A map that fills the
/// card to the last cluster strands the ride log, the route uploads, and every sidecar — and the
/// rider finds out mid-ride, not at the desk where the upload happened. 16 MiB is generous against
/// a ride log (a long day is tens of KB) and negligible against a map.
pub const MAP_FREE_HEADROOM: u64 = 16 << 20;

/// The in-flight BLE route upload, inside `/routes`. Its extension never matches the catalog scan,
/// so a partial upload — a drop, a power cut — is invisible until [`Storage::upload_commit`]
/// promotes it. Truncated-and-reused per upload.
const UPLOAD_TMP: &str = "UPLOAD.TMP";

/// The one upload handle's strategy owner. Ordinary temps additionally carry the exact wire and
/// repository; stale or other-wire actions cannot match that keyed session (#1292).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum UploadOwner {
    /// `/routes/UPLOAD.TMP`: routes, trips and firmware images, staged and then promoted.
    Temp(GateOwner, UploadDestination),
    /// A single map streaming straight into its final `MP{id}.OBM` with the magic held back (#927).
    Map,
    /// One file of a volume set — a shard or the manifest (`OBCA_Spec.md` §5, #1039).
    Set,
    /// The inactive `/WEATHER.A` or `/WEATHER.B` generation. It never uses `/routes/UPLOAD.TMP`.
    Weather(WeatherSlot),
}

/// One retained route/trip/ride detail handle. The name lets repositories borrow it for scans;
/// owner scopes link teardown, while the raw-file capability keys every stream read and close.
struct OpenObject {
    name: ShortFileName,
    owner: GateOwner,
    file: RawFile,
    len: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum UploadDestination {
    Route,
    Trip,
    Firmware,
}

impl UploadDestination {
    const fn from_object_type(ty: ObjectType) -> Option<Self> {
        match ty {
            ObjectType::Route => Some(Self::Route),
            ObjectType::Trip => Some(Self::Trip),
            ObjectType::FwImage => Some(Self::Firmware),
            _ => None,
        }
    }
}

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
/// Hidden local-planner stage. Its `.TMP` extension is never admitted by [`is_route_entry`], so a
/// cancelled or interrupted search cannot replace the last committed `_NAV.OBR` publication.
const NAV_TMP: &str = "NAV.TMP";

/// First id of the reserved **session-scoped** band handed to side-loaded `.obcr` files (their
/// names carry no durable id — the canonical [`obc_storage::Catalog`] assigns them session
/// ids). Uploaded ids grow monotonically from
/// 0 and reject at this floor — 65,024 lifetime uploads before a card must be cleared, i.e. never.
pub(crate) const SIDELOAD_ID_BASE: u16 = 0xFF00;

fn clear_session_crcs(crcs: &mut RouteCrcs) {
    while let Some(id) = crcs.entries().iter().find_map(|(id, _)| (*id >= SIDELOAD_ID_BASE).then_some(*id)) {
        crcs.remove(id);
    }
}

/// The concrete SD stack for this board: [`SemmcCard`] — the card in native 4-bit mode on the FLPR
/// — under a 16-file/4-dir [`VolumeManager`].
///
/// **Why more than 4 open files** (the default 4 loses mid-ride uploads): riding with tracking holds three
/// handles for the whole session — the map stream, the active route's geometry, and the ORD track
/// log. A BLE route upload adds its temp (4), and `upload_commit`'s copy-promote (embedded-sdmmc
/// can't rename, see the note above [`Storage::upload_commit`]) holds the reopened temp **and**
/// the final `.OBR` at once — a 5-handle peak, which the 4-slot default answered with a failed
/// commit exactly and only mid-ride. A mounted volume set adds one handle per shard on top of
/// that (see [`SD_MAX_FILES`]); each slot is 64 bytes of `FileInfo`, so the RAM cost is noise.
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
/// **Why 16.** The pre-volume-set budget was 6: the 5-handle mid-ride peak documented above plus
/// one slot of headroom. `OBCA_Spec.md` §5 makes one logical map 1..32 physical files, and the
/// epic's device-cost review requires a mounted set to hold **every** shard's handle open for the
/// mount lifetime — re-opening per query would put a FAT directory walk in the render loop. A DACH
/// set is core + coarse + ~6 geometry = 8 handles, so the budget is that 8 plus the unchanged
/// 5-handle peak (of which the map's own slot is now the set's), plus one session-long weather
/// bundle reader and no unused margin. The 11-shard ceiling therefore still fits exactly at the
/// worst ride/upload/weather peak rather than failing only when rain data is mounted.
///
/// The cost is measured, not guessed: the fork's `FileInfo` (`filesystem/files.rs`) is `RawFile`
/// 4 · `RawVolume` 4 · `current_cluster` 8 · `current_offset` 4 · `Mode` 1 · `DirEntry` 40 ·
/// `dirty` 1, i.e. **64 B** at `align 4` on thumbv8m. `6 → 16` is ten slots, **+640 B of `.bss`**
/// — the manager's `open_files` array is a `heapless::Vec<FileInfo, SD_MAX_FILES>` and nothing
/// else scales with it. Nothing on the stack changes.
const SD_MAX_FILES: usize = 16;
const SD_MAX_VOLUMES: usize = 1;
/// Handles a ride holds at its peak, and therefore the handles a mount may **not** have: the
/// active route's geometry, the ORD track log, a BLE upload temp, and `upload_commit`'s
/// copy-promote pair (the reopened temp **and** the final `.OBR` at once) — the 5-handle peak the
/// file-count note above documents, of which the map's own slot is now the set's.
const SD_RIDE_PEAK_FILES: usize = 5;
/// **The real ceiling on a mountable volume set for this board: 11 shards.**
///
/// `OBCA_Spec.md` §5.2 allows `1..=32`, and this board cannot honour that — a mount holds every
/// shard's handle open for its lifetime (re-opening per query would put a FAT directory walk in the
/// render loop), so the largest set it can mount is `SD_MAX_FILES − SD_RIDE_PEAK_FILES`. That is
/// comfortably past the shape §5.1 projects for the largest v1 set — DACH is core + coarse + ~6
/// geometry = **8 files** — but it is short of the format's cap, and the difference must be a
/// stated refusal rather than a failed open halfway through a ride.
///
/// [`SetShardStore`] carries the number into the type, so `obc_reader::MountedSet::mount` refuses a
/// larger set with `MountError::Handles(11)` — an error that names *this device's* cap, which is
/// the number a rider needs, rather than the format's.
pub(crate) const SD_SET_MAX_SHARDS: usize = SD_MAX_FILES - SD_RIDE_PEAK_FILES;
/// A set shard's FAT-run budget. Shards are generated and uploaded as a sequential publish tree,
/// and are smaller than the standalone maps for which [`obc_storage::fat_extents::MAX_EXTENTS`]
/// was sized. 64 still exceeds the reference card's worst measured map (46 runs) while avoiding
/// eleven unnecessarily large 128-run tables in resident RAM. A more fragmented shard is refused
/// with its true run count and can be fixed by re-copying the publish tree onto the card.
const SET_MAX_EXTENTS: usize = 64;
/// The per-shard mount records, sized to this board's ceiling. A device mount places one of these
/// in `.bss` (never on a frame — 14 KB of `heapless::Vec` inside an embassy task frame is the #270
/// trap) and mounts into it; see `obc_reader::volume`'s module docs.
pub(crate) type SetShardStore = obc_reader::SetShards<'static, SD_SET_MAX_SHARDS>;
type Vmgr = VolumeManager<SdShared, NullTime, SD_MAX_DIRS, SD_MAX_FILES, SD_MAX_VOLUMES>;
/// [`SdByteSource`] over this board's manager (the wrappers are generic over the handle budget).
type Source<'a> = SdByteSource<'a, SdShared, NullTime, SD_MAX_DIRS, SD_MAX_FILES, SD_MAX_VOLUMES>;
/// The smaller resident extent table used only for published set shards; standalone maps retain
/// [`ExtentTable`]'s 128-run default.
type SetExtentTable = ExtentTableWithCapacity<SET_MAX_EXTENTS>;
type SetExtentSource<'a> = ExtentSourceWithCapacity<'a, Sd, SET_MAX_EXTENTS>;
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

/// The mutually-exclusive homes of either one standalone map table or every mounted-set table.
/// `open_map` makes that choice once per boot and never switches it: a single map initialises only
/// `map`, while [`open_volume_set`](Storage::open_volume_set) initialises distinct `set` slots and
/// retains their handles for the session. A union therefore saves the otherwise permanently idle
/// standalone table without introducing reuse or lifetime transitions under a live reference.
///
/// This remains outside [`Storage`] because that value crosses `main`'s async frame by value; the
/// old inline table measurably produced two extra resident copies (#270/#500). `ManuallyDrop` is
/// only a union-field requirement — extent tables own no resources and are written once in place.
union ExtentSlots {
    map: core::mem::ManuallyDrop<core::mem::MaybeUninit<ExtentTable>>,
    set: core::mem::ManuallyDrop<[core::mem::MaybeUninit<SetExtentTable>; SD_SET_MAX_SHARDS]>,
}

const _: () =
    assert!(core::mem::size_of::<[SetExtentTable; SD_SET_MAX_SHARDS]>() >= core::mem::size_of::<ExtentTable>());
const _: () = assert!(core::mem::align_of::<SetExtentTable>() >= core::mem::align_of::<ExtentTable>());
const _: () =
    assert!(core::mem::size_of::<ExtentSlots>() == core::mem::size_of::<[SetExtentTable; SD_SET_MAX_SHARDS]>());
const _: () = assert!(core::mem::align_of::<ExtentSlots>() == core::mem::align_of::<SetExtentTable>());

static mut EXTENT_SLOTS: ExtentSlots =
    ExtentSlots { set: core::mem::ManuallyDrop::new([const { core::mem::MaybeUninit::uninit() }; SD_SET_MAX_SHARDS]) };

/// One immutable direct-read source per mounted set shard. The source records stay separate from
/// [`EXTENT_SLOTS`] because every one is needed together; their table pointers target distinct
/// `set` slots for the session. Rebuilding one per viewport query would put a FAT walk in the
/// render loop, and the open handles in [`OpenSet`] pin the chains they describe.
static mut SET_SOURCES: [core::mem::MaybeUninit<SetExtentSource<'static>>; SD_SET_MAX_SHARDS] =
    [const { core::mem::MaybeUninit::uninit() }; SD_SET_MAX_SHARDS];

/// The terrain sidecar's resident extent table + direct-read source (EL7, epic #1068). A *second*
/// file is open beside the map for the session, so these cannot share [`EXTENT_SLOTS`] (which is a
/// union: one map **or** one set) — terrain gets its own pair, at the set shards' 64-run capacity
/// rather than the map's 128, because a baked terrain artifact is a small, freshly-written file.
///
/// Only the seek-free path is admitted here, for the same reason a set refuses one: a terrain
/// sample sits inside the nav emit loop, and reinserting a FAT walk per 512 B tile would put SD
/// seeks under the router. A file that will not extent-map simply yields no terrain.
#[cfg(has_nav)]
static mut TERRAIN_EXTENTS: core::mem::MaybeUninit<SetExtentTable> = core::mem::MaybeUninit::uninit();
#[cfg(has_nav)]
static mut TERRAIN_SOURCE: core::mem::MaybeUninit<SetExtentSource<'static>> = core::mem::MaybeUninit::uninit();

/// The 8.3 extension of a terrain artifact (`OBCT_Spec.md` §4.6: `.obcd`, three-char twin `OBD` —
/// deliberately not `.OBT`, which is a recorded ride log).
#[cfg(has_nav)]
const TERRAIN_EXT: &str = "OBD";

/// Exact target-side bytes of the terrain sidecar's board-private statics (table + source), for the
/// compile-time RAM budget and the resource report in `main.rs`. Reported unconditionally, like the
/// `nav_*` sizes: it is the *type*'s cost, and a profile that gates the router out still wants the
/// number legible in its report.
pub(crate) const TERRAIN_EXTENT_BYTES: usize =
    core::mem::size_of::<SetExtentTable>() + core::mem::size_of::<SetExtentSource<'static>>();

/// Exact target-side bytes of the board-private volume-set statics, exported numerically for the
/// compile-time RAM budget and resource report in `main.rs` without exposing their concrete types.
pub(crate) const SET_EXTENT_TABLES_BYTES: usize = core::mem::size_of::<ExtentSlots>();
pub(crate) const SET_SOURCES_BYTES: usize = core::mem::size_of::<[SetExtentSource<'static>; SD_SET_MAX_SHARDS]>();

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
    /// The durable object id, for a map this device received (`MP{id}.OBM`, or `MS{id}.OBS` for a
    /// volume set). `None` for a side-loaded `.obcm`, which carries no device-assigned identity —
    /// the filename is all it has. The two conventions number **independently**, so an id is only
    /// comparable with another of the same [`shards`](MapSummary::shards)-ness.
    pub id: Option<u16>,
    /// The 8.3 filename, which is what [`MAP_SELECTED`] records and what reopens the file. For a
    /// volume set this is the **manifest** (`MS{id}.OBS`) — the one file that says the shards
    /// beside it are one map (`OBCA_Spec.md` §5.4).
    pub file: ShortFileName,
    /// `Some(count)` when this entry is an OBCA **volume set** (§5): one manifest plus `count`
    /// shard files, presented as ONE map with one summed size. `None` for a single `MP{id}.OBM`
    /// or side-loaded `.obcm`. Shard count is an implementation detail a rider never sees (§5.4);
    /// it lives here only so a delete can reach the whole prefix.
    pub shards: Option<u8>,
    /// The display name: the long filename's stem when the file has one, else the 8.3 stem. For an
    /// uploaded map that is `MP{id}` — the honest consequence of having no name on the wire.
    pub name: String<24>,
    /// Size on the card, from the directory entry (no read). For a volume set, the **sum** over
    /// every shard plus the manifest — the only size figure a UI may show (§5.4), and `u64`
    /// because a set is exactly the thing that outgrows one `u32` file.
    pub byte_len: u64,
    /// The OBCM format version from header byte 4 (from the manifest's `OBCM Version` for a set,
    /// which §5.3 pins equal across every shard). Reported, never filtered: a map built for
    /// another version is still on the card, and a consumer that wants to *flag* it (#915) needs
    /// to see it.
    pub obcm_version: u8,
    /// The global bounding box from header bytes 5..21 — the map's footprint, for coverage checks.
    /// For a set, the manifest's assembly bbox (§4.2), which §5.3 pins equal to the core's header.
    pub bbox: BBox,
    /// Whether [`MAP_SELECTED`] names this map.
    pub selected: bool,
    /// Directory-entry location, so a chosen map's extent table can be built without a second scan.
    entry_block: embedded_sdmmc::BlockIdx,
    entry_offset: u32,
}

/// What a validated `MS{id}.OBS` manifest contributes to the catalog — everything a
/// [`MapSummary`] needs about a volume set, and nothing the mount would need later. Deliberately
/// small: [`Storage::set_identity`] parses the manifest and drops it, because the catalog's job is
/// to say *this is one map, this big, over this ground*, not to hold a mount open.
struct SetIdentity {
    shard_count: u8,
    obcm_version: u8,
    bbox: BBox,
    /// Summed over every record the manifest carries — the OBCM shards **and** the terrain raster,
    /// as `OBCA_Spec.md` §5.4 requires of the one size figure a UI may show (#1044). The manifest's
    /// own bytes are added by the caller.
    total_bytes: u64,
    /// The manifest's display name (§5.2), empty when it carries none.
    name: String<24>,
}

/// What [`Storage::map_source`] hands out: extent-mapped direct block reads when the map's chain
/// resolved at open (#500), the manager's seek+read path otherwise. One enum rather than a trait
/// object so the render/nav paths stay monomorphic (no vtable on the per-chunk hot path).
pub enum MapSource<'a> {
    /// Direct block reads through the resolved [`ExtentTable`] — zero FAT traffic per read.
    Extent(ExtentSource<'a, Sd>),
    /// The plain seek path — correct on any card, O(offset) on backward seeks.
    Seek(Source<'a>),
    /// A core shard of a mounted set. Its source is resident beside its per-shard extent table.
    Set(&'a SetExtentSource<'static>),
}

impl ByteSource for MapSource<'_> {
    // `inline(never)`: reached from the deepest render/nav frames — keep the dispatch (and both
    // arms' machinery) out of those frames' locals, whatever the inliner decides later; a call
    // per multi-ms SD read is free. See the matching note on `ExtentSource::read_at`.
    #[inline(never)]
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), obc_formats::io::Error> {
        match self {
            MapSource::Extent(s) => s.read_at(offset, buf),
            MapSource::Seek(s) => s.read_at(offset, buf),
            MapSource::Set(s) => s.read_at(offset, buf),
        }
    }

    fn len(&self) -> u32 {
        match self {
            MapSource::Extent(s) => s.len(),
            MapSource::Seek(s) => s.len(),
            MapSource::Set(s) => s.len(),
        }
    }
}

struct OpenSetShard {
    file: RawFile,
    len: u32,
    source: &'static SetExtentSource<'static>,
}

/// A mounted set's session-long storage ownership. Every shard handle stays open, pinning its FAT
/// chain and making every render dispatch a bbox test + direct block read, never a directory walk.
struct OpenSet {
    id: u16,
    manifest_name: ShortFileName,
    core_index: u8,
    shards: Vec<OpenSetShard, SD_SET_MAX_SHARDS>,
    /// The `Bytes` of the manifest's `terrain` record (`OBCA_Spec.md` §5.2), or `None` when the set
    /// names no raster. Kept from the mount rather than re-read, so [`Storage::open_terrain`] never
    /// has to parse the manifest a second time — and so a `MS<id>.OBD` the manifest does *not* name
    /// is recognised as an orphan of a replaced set (§5.4) instead of being mounted.
    terrain_bytes: Option<u32>,
}

/// Why the board could not turn its already-open shard handles into a reader mount. Kept separate
/// from [`obc_reader::MountError`] because re-reading the small manifest from FAT is board policy,
/// not part of the format reader.
#[derive(Debug, Clone, Copy)]
pub(crate) enum DeviceMountError {
    Manifest,
    Sources,
    Reader(#[allow(dead_code)] obc_reader::MountError),
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
    /// The one trip catalog shared by the on-device folders and companion-link repository. The
    /// three aligned columns have exactly the same size/layout as the former `trip_ids` /
    /// `trip_files` / `trip_metas` fields; an optional metadata niche lets a just-committed trip
    /// remain addressable through a transient metadata reread failure until the live rescan.
    trip_catalog: StoredTripCatalog,
    /// The active route's open geometry file: `(durable object id, handle, length)`. Reopened only
    /// when the selected route changes; the id remains stable across reorder and projection gaps.
    open_route: Option<(u16, RawFile, u32)>,
    /// The map `.obcm`, opened once at startup and held open for the whole session: `(handle,
    /// length)`. The map streams through this (issue #37) instead of being read resident into
    /// RAM — `map_source` hands out a fresh source over it each redraw.
    open_map: Option<(RawFile, u32)>,
    /// The mutually-exclusive volume-set form of `open_map`: parsed manifest + every shard handle
    /// and its resident extent source, all retained for the session.
    open_set: Option<OpenSet>,
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
    /// The map's **terrain sidecar** (EL7, epic #1068), when one opened: `(handle, length)`, held
    /// open for the session exactly like the map so its FAT chain stays pinned under the resident
    /// extent source. `None` = no `.OBD` beside the map, or it would not open / extent-map — every
    /// one of which is a *no terrain* answer, never a fault (see [`open_terrain`](Storage::open_terrain)).
    #[cfg(has_nav)]
    open_terrain: Option<(RawFile, u32)>,
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
    /// The companion object's open route/ride file (a detail download in flight). A separate slot
    /// from `open_route` so a download can't disturb an active
    /// ride's geometry. The name is kept so the catalog scan can recognise (and read through)
    /// this handle instead of a second open — embedded-sdmmc refuses every second open of an
    /// open file (`FileAlreadyOpen`, even ReadOnly), which would silently drop the route from
    /// the catalog (issue #480).
    open_object: Option<OpenObject>,
    /// The in-flight upload's open file handle **and which path owns it** — the temp staging file,
    /// a single map's final `MP{id}.OBM`, or one file of a volume set. See [`UploadOwner`]: the tag
    /// is what stops one transport's teardown closing a handle the other transport is streaming
    /// through (issue #1039).
    open_upload: Option<(RawFile, UploadOwner)>,
    /// Fully validated boot choice over `/WEATHER.A` and `/WEATHER.B`. Only metadata is resident;
    /// OBCW bytes remain on SD.
    weather_active: Option<WeatherCandidate>,
    /// Location/frame facts used only by the board's refresh policy, populated from the same
    /// session-open validated header as `weather_active`.
    weather_policy: Option<WeatherPolicyFacts>,
    /// Session-long read handle for [`weather_active`](Storage::weather_active), opened after the
    /// A/B validation pass and replaced on commit. Holding it mirrors the map/route streams and is
    /// important on embedded-sdmmc: reopening the file for every sample would put a directory walk
    /// and open/close pair on the weather screen's hot path.
    open_weather: Option<(RawFile, u32)>,
    /// Full-validation proof for `open_weather`. Readers reconstructed from this token skip the
    /// whole-object CRC/tile walk; the held read-only handle is the stable-source invariant.
    weather_mount: Option<obc_weather::ValidatedBundle>,
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

/// Bring up the SD card: boot the sEMMC soft peripheral, identify the card (4-bit, High Speed,
/// 32 MHz reads) and mount the FAT volume. Returns `None` on any failure (no card, not FAT,
/// unreadable) so the caller degrades gracefully — never panicking (acceptance criterion).
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
pub fn init() -> Result<Storage, obc_app::BootFault> {
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
    // Into its `.bss` slot before anything else: the manager and the extent read path both want
    // `'static` borrows of the one card.
    // SAFETY: sole writer of SD_CARD; `init` runs once per boot on the one thread-mode executor,
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

// ═══════════════════════ map-upload write pipeline ═══════════════════════

/// One USB map append's raw writes, collected until the FAT call returns.
///
/// `VolumeManager::write` deliberately stops a device write at every cluster, even when the
/// pre-allocated chain continues at the next LBA. During a staged map upload those calls all point
/// into one immutable arena half. Coalescing adjacent `(LBA, pointer)` spans here turns them back
/// into the single CMD25 the card can execute efficiently, without teaching the generic FAT layer
/// anything board-specific.
#[derive(Clone, Copy)]
struct QueuedUploadWrite {
    lba: u32,
    ptr: *const u8,
    len: usize,
}

#[repr(C, align(4))]
struct UploadCacheBlock([u8; BLOCK_LEN]);

struct UploadWritePipe {
    enabled: bool,
    queued: Option<QueuedUploadWrite>,
    active_lba: u32,
    active_blocks: u32,
    cache_lba: u32,
    cache_valid: bool,
    cache: UploadCacheBlock,
}

impl UploadWritePipe {
    const fn new() -> Self {
        Self {
            enabled: false,
            queued: None,
            active_lba: 0,
            active_blocks: 0,
            cache_lba: 0,
            cache_valid: false,
            cache: UploadCacheBlock([0; BLOCK_LEN]),
        }
    }
}

/// Solely accessed while the cooperative executor holds the shared store. The USB stage is the
/// only code that enables it, and it disables it before releasing the upload handle. No ISR reads
/// this state (the VPR IRQ only sets `semmc::COMPLETION`).
static mut UPLOAD_WRITE_PIPE: UploadWritePipe = UploadWritePipe::new();

#[inline(always)]
fn upload_pipe() -> &'static mut UploadWritePipe {
    // SAFETY: the shared-store/one-executor rule above provides the unique access; references are
    // never retained across an await or returned to a caller.
    unsafe { &mut *core::ptr::addr_of_mut!(UPLOAD_WRITE_PIPE) }
}

fn upload_pipe_enabled() -> bool {
    upload_pipe().enabled
}

fn upload_pipe_begin() -> bool {
    let pipe = upload_pipe();
    if pipe.enabled || pipe.active_blocks != 0 || pipe.queued.is_some() {
        defmt::error!("SD: deferred upload pipe was not idle at begin");
        return false;
    }
    pipe.enabled = true;
    pipe.cache_valid = false;
    true
}

fn upload_pipe_finish_active() -> Result<(), crate::semmc::SemmcError> {
    let (lba, blocks) = {
        let pipe = upload_pipe();
        if pipe.active_blocks == 0 {
            return Ok(());
        }
        let out = (pipe.active_lba, pipe.active_blocks);
        // Clear before entering the driver so an error path cannot leave a phantom DMA owner.
        pipe.active_blocks = 0;
        out
    };
    let result = crate::flpr_mux::with_storage(|sd| sd.finish_write_blocks())?;
    if let Err(e) = result {
        log_transfer_error("deferred write", lba, blocks as usize, e);
    }
    result
}

fn upload_pipe_start_queued() -> Result<(), crate::semmc::SemmcError> {
    let queued = match upload_pipe().queued.take() {
        Some(queued) => queued,
        None => return Ok(()),
    };
    debug_assert!(queued.len.is_multiple_of(BLOCK_LEN));
    let blocks = (queued.len / BLOCK_LEN) as u32;
    // SAFETY: the queue was formed from a currently borrowed immutable staging slice. Stage does
    // not reuse that arena half until the next append has joined this write; the pipe stores no
    // slice, only the DMA address needed to start the transfer here.
    let bytes = unsafe { core::slice::from_raw_parts(queued.ptr, queued.len) };
    let result = crate::flpr_mux::with_storage(|sd| {
        // SAFETY: the double-buffer lifetime rule above holds until `upload_pipe_finish_active`.
        unsafe { sd.start_write_blocks(queued.lba, bytes) }
    })?;
    if result.is_ok() {
        let pipe = upload_pipe();
        pipe.active_lba = queued.lba;
        pipe.active_blocks = blocks;
    }
    result
}

fn upload_pipe_flush() -> Result<(), crate::semmc::SemmcError> {
    upload_pipe_finish_active()?;
    upload_pipe_start_queued()?;
    upload_pipe_finish_active()
}

fn upload_pipe_end() -> bool {
    let result = upload_pipe_flush();
    let pipe = upload_pipe();
    pipe.enabled = false;
    pipe.queued = None;
    pipe.cache_valid = false;
    result.is_ok()
}

fn upload_pipe_queue(lba: u32, buf: &[u8]) -> Result<(), crate::semmc::SemmcError> {
    debug_assert!(upload_pipe_enabled());
    debug_assert!(buf.len().is_multiple_of(BLOCK_LEN));
    let next = QueuedUploadWrite { lba, ptr: buf.as_ptr(), len: buf.len() };
    let merge = upload_pipe().queued.is_some_and(|queued| {
        queued.lba + (queued.len / BLOCK_LEN) as u32 == next.lba && queued.ptr.addr() + queued.len == next.ptr.addr()
    });
    if merge {
        upload_pipe().queued.as_mut().unwrap().len += next.len;
        return Ok(());
    }
    if upload_pipe().queued.is_some() {
        // Fragmented chain, a FAT-cache miss, or a partial-block bounce broke the run. Complete the
        // earlier run synchronously before queuing this one; correctness degrades gracefully while
        // a contiguous pre-allocation stays on the deferred fast path.
        upload_pipe_start_queued()?;
        upload_pipe_finish_active()?;
    }
    upload_pipe().queued = Some(next);
    Ok(())
}

fn upload_cache_read(blocks: &mut [Block], lba: u32) -> bool {
    if blocks.len() != 1 {
        return false;
    }
    let pipe = upload_pipe();
    if !pipe.enabled || !pipe.cache_valid || pipe.cache_lba != lba {
        return false;
    }
    blocks[0].contents.copy_from_slice(&pipe.cache.0);
    true
}

fn upload_cache_note_read(blocks: &[Block], lba: u32) {
    if blocks.len() == 1 && upload_pipe_enabled() {
        let pipe = upload_pipe();
        pipe.cache.0.copy_from_slice(&blocks[0].contents);
        pipe.cache_lba = lba;
        pipe.cache_valid = true;
    }
}

fn upload_cache_note_write(lba: u32, blocks: usize) {
    let pipe = upload_pipe();
    if pipe.cache_valid && pipe.cache_lba >= lba && pipe.cache_lba < lba.saturating_add(blocks as u32) {
        pipe.cache_valid = false;
    }
}

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
    let cluster = bytes_per_sector * sectors_per_cluster;
    let half = crate::usb::STAGE_HALF_LEN as u32;
    if half.is_multiple_of(cluster) {
        defmt::info!(
            "SD: {=u32} B clusters; staged upload run {=u32} B / {=u32} blocks / {=u32} cluster(s)",
            cluster,
            half,
            half / BLOCK_LEN as u32,
            half / cluster
        );
    } else {
        defmt::info!(
            "SD: {=u32} B clusters; staged upload run {=u32} B / {=u32} blocks (cross-cluster coalescing active)",
            cluster,
            half,
            half / BLOCK_LEN as u32
        );
    }
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
        if upload_pipe_enabled() {
            if upload_cache_read(blocks, start_block_idx.0) {
                return Ok(());
            }
            // A real card command cannot overlap the queued/deferred write. A FAT-sector hit above
            // is RAM-only and intentionally can: that is what lets one `VolumeManager::write`
            // coalesce across its cluster boundaries.
            upload_pipe_flush()?;
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
        } else {
            upload_cache_note_read(blocks, start_block_idx.0);
        }
        r
    }

    fn write(&self, blocks: &[Block], start_block_idx: BlockIdx) -> Result<(), Self::Error> {
        if blocks.is_empty() {
            return Ok(());
        }
        let (addr, n) = (blocks.as_ptr() as usize, blocks.len());
        upload_cache_note_write(start_block_idx.0, n);
        let byte_len = n * BLOCK_LEN;
        let stable_stage = addr.is_multiple_of(4) && crate::arena::usb_stage_contains(addr, byte_len);
        if upload_pipe_enabled() && stable_stage {
            // SAFETY is carried by the arena-range check: Stage holds this immutable half until
            // the deferred transfer is joined.
            let buf = unsafe { core::slice::from_raw_parts(blocks.as_ptr().cast::<u8>(), n * BLOCK_LEN) };
            return upload_pipe_queue(start_block_idx.0, buf);
        }
        if upload_pipe_enabled() {
            // Partial file sectors and FAT/directory metadata live in dependency-owned temporary
            // `Block`s. Whether aligned or not, their lifetime ends at this call, so drain the fast
            // pipe and write them synchronously. Misaligned values use the established bounce path.
            upload_pipe_flush()?;
        }
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
        if upload_pipe_enabled() {
            upload_pipe_flush()?;
        }
        crate::flpr_mux::with_storage(|sd| sd.num_blocks())?.map(BlockCount)
    }
}

impl Storage {
    pub(crate) fn maps(&mut self) -> Maps<'_> {
        Maps::new(self)
    }

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
            trip_catalog: StoredTripCatalog::new(),
            open_route: None,
            open_map: None,
            open_set: None,
            open_map_name: None,
            map_extents: None,
            #[cfg(has_nav)]
            open_terrain: None,
            map_boot_fault: None,
            open_track: None,
            pending_save: None,
            ride_saved: false,
            open_object: None,
            open_upload: None,
            weather_active: None,
            weather_policy: None,
            open_weather: None,
            weather_mount: None,
            map_name: String::new(),
        })
    }

    /// Borrow the sole route repository. The view cannot outlive this storage lock and callers drop
    /// it before any `await`.
    pub(crate) fn routes<'a>(&'a mut self, catalog: &'a mut StoredRouteCatalog) -> Routes<'a> {
        Routes::new(self, catalog)
    }

    /// Borrow the sole trip repository. The view cannot outlive this storage lock and callers drop
    /// it before any `await`.
    pub(crate) fn trips(&mut self) -> Trips<'_> {
        Trips::new(self)
    }

    /// Borrow the sole stored-ride repository. The view cannot outlive this storage lock and every
    /// caller drops it before an `await`.
    pub(crate) fn rides<'a>(&'a mut self, catalog: &'a mut StoredRideCatalog) -> Rides<'a> {
        Rides::new(self, catalog)
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

    /// Record ride `id` as synced at `synced_at` (UTC unix seconds, `0` when the clock is untrusted —
    /// the sweep starts the countdown lazily) and persist the sidecar, but only when it's a **new**
    /// entry (a re-download of an already-flagged ride rewrites nothing, keeping its first-sync
    /// stamp). Returns `true` if the sidecar changed. Called at a ride download's completion (epic
    /// #447 P7). Read-modify-write within the call — the handle is opened, written truncating, and
    /// closed here, so it never counts against the open-file budget across an `await`.
    pub fn mark_ride_synced(&mut self, id: u16, synced_at: u32) -> bool {
        self.mark_rides_synced(core::iter::once(id), synced_at) > 0
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
            let _ = out.push(obc_app::RideRetentionRecord { id, synced: true, synced_at_utc: synced_at });
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
    fn load_route_crcs(&self) -> RouteCrcs {
        self.load_crc_sidecar(ROUTE_CRCS)
    }

    fn load_crc_sidecar(&self, name: &str) -> RouteCrcs {
        self.load_crc_sidecar_status(name).0
    }

    /// Return decoded rows plus whether the read authoritatively proved the on-card contents. A
    /// transient open/read/close failure is not equivalent to an absent sidecar for session-id
    /// rebinding: callers keep FF00+ masked and must not overwrite durable low-id rows from an
    /// untrustworthy empty decode.
    fn load_crc_sidecar_status(&self, name: &str) -> (RouteCrcs, bool) {
        let Some(dir) = self.routes_dir else { return (RouteCrcs::new(), false) };
        let file = match self.vmgr.open_file_in_dir(dir, name, Mode::ReadOnly) {
            Ok(file) => file,
            Err(embedded_sdmmc::Error::NotFound) => return (RouteCrcs::new(), true),
            Err(_) => return (RouteCrcs::new(), false),
        };
        let mut buf = [0u8; ROUTE_CRCS_MAX_LEN];
        let read = self.vmgr.read(file, &mut buf);
        let closed = self.vmgr.close_file(file).is_ok();
        match read {
            Ok(n) if closed => (decode_route_crcs(&buf[..n]), true),
            _ => (RouteCrcs::new(), false),
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
    fn write_route_crcs(&mut self, map: &RouteCrcs) -> bool {
        let persisted = self.write_crc_sidecar(ROUTE_CRCS, map);
        if !persisted {
            defmt::warn!("SD: route-crc sidecar not persisted — a route may serve crc 0 next list build");
        }
        persisted
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
    fn load_route_retention(&self) -> RouteRetentionStore {
        self.load_route_retention_status().0
    }

    fn load_route_retention_status(&self) -> (RouteRetentionStore, bool) {
        let Some(dir) = self.routes_dir else { return (RouteRetentionStore::new(), false) };
        let file = match self.vmgr.open_file_in_dir(dir, ROUTE_RETENTION, Mode::ReadOnly) {
            Ok(file) => file,
            Err(embedded_sdmmc::Error::NotFound) => return (RouteRetentionStore::new(), true),
            Err(_) => return (RouteRetentionStore::new(), false),
        };
        let mut buf = [0u8; ROUTE_RETENTION_MAX_LEN];
        let read = self.vmgr.read(file, &mut buf);
        let closed = self.vmgr.close_file(file).is_ok();
        match read {
            Ok(n) if closed => (decode_route_retention(&buf[..n]), true),
            _ => (RouteRetentionStore::new(), false),
        }
    }

    /// Overwrite the route-retention sidecar (truncating), returning whether the whole rewrite —
    /// open, write, flush, close — reached the card (finding #876-5). A torn write is safe by design
    /// (a route reads `Never` next list build → nothing deletes). The borrowed [`Routes`] owner
    /// decides which mutations must propagate failure so `setRouteRetention` never claims `ok`
    /// ahead of durability.
    fn write_route_retention(&mut self, store: &RouteRetentionStore) -> Result<(), SidecarWriteError> {
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
    /// is sitting in the root must not be told to go and add one. That covers a failed set mount,
    /// both single-file open failures, and — via `scan_maps_into`'s count — files the catalog never
    /// saw because their header would not parse.
    fn open_map(&mut self) -> Option<u32> {
        if let Some((_, len)) = self.open_map {
            return Some(len);
        }
        if let Some(open) = &self.open_set {
            return open.shards.get(open.core_index as usize).map(|shard| shard.len);
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
        if let Some(shards) = chosen.shards {
            if shards as usize > SD_SET_MAX_SHARDS {
                defmt::warn!(
                    "SD: volume set {} has {=u8} shards; this board mounts at most {=usize}",
                    defmt::Debug2Format(&chosen.file),
                    shards,
                    SD_SET_MAX_SHARDS
                );
                self.map_boot_fault = Some(obc_app::boot_fault(&map_choices(&maps), unlistable));
                return None;
            }
            match self.open_volume_set(&chosen) {
                Some(core_len) => {
                    self.retire_superseded_maps(&maps, Some(keep));
                    return Some(core_len);
                }
                None => {
                    self.map_boot_fault = Some(obc_app::boot_fault(&map_choices(&maps), unlistable));
                    return None;
                }
            }
        }
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
        // report — NO MAP would send them looking for a file that is right there. (The volume-set
        // refusal above learned this first; these are the single-map paths it deliberately left
        // alone.)
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

    /// **The terrain source for the mounted map** (EL7, epic #1068) — the board's half of the one
    /// question a host answers in `obc_host_core::terrain`: open the `.OBD` sidecar named after the
    /// map (`GRIMSEL.OBM` → `GRIMSEL.OBD`; a set's manifest `MS7.OBS` → `MS7.OBD`), resolve its FAT
    /// chain to a resident extent source, and hand back a `'static` [`ByteSource`] over it. Call
    /// once, right after [`open_map`](Self::open_map).
    ///
    /// **Every failure is `None`, and `None` is not a fault.** No sidecar, a name that will not
    /// open, a chain that will not extent-map, a table that fails verification — all of them mean
    /// *this map has no terrain*, which the whole elevation seam is built to handle (routes plan
    /// and ride exactly as before, with flat profiles). Deliberately outside the NO MAP / MAP
    /// UNREADABLE rules of #1042: those exist because a rider whose **map** is missing must not be
    /// misled, and a missing terrain file takes nothing away that was ever there.
    ///
    /// **A set answers from its manifest** (EL4, #1072). The `terrain` role's presence decides
    /// whether the set has a raster at all, and its `Bytes` is checked against the file — because
    /// `MS<id>.OBD` may well exist on a card without belonging to the set that is mounted, as the
    /// leftover of one it replaced (§5.4's orphan rule). The derived name is the same one either
    /// way: `MS<id>.OBD` is exactly the sidecar of `MS<id>.OBS`, so the role lookup adds the
    /// manifest's judgement without changing which file is opened.
    #[cfg(has_nav)]
    #[inline(never)]
    fn open_terrain(&mut self) -> Option<&'static dyn ByteSource> {
        // What the manifest says about a mounted set, and `None` (no terrain) when it says nothing.
        let recorded = match &self.open_set {
            Some(set) if self.open_map_name.is_none() => Some(set.terrain_bytes?),
            _ => None,
        };
        let map_name =
            self.open_map_name.clone().or_else(|| self.open_set.as_ref().map(|s| s.manifest_name.clone()))?;
        let name = sidecar_name(&map_name)?;
        let (entry_block, entry_offset, entry_len) = self.find_root_entry(&name)?;
        if let Some(recorded) = recorded {
            if entry_len != recorded {
                defmt::warn!(
                    "SD: terrain {} is {=u32} B but the manifest records {=u32} — routes stay flat",
                    defmt::Debug2Format(&name),
                    entry_len,
                    recorded
                );
                return None;
            }
        }
        let Ok(file) = self.vmgr.open_file_in_dir(self.root, &name, Mode::ReadOnly) else {
            defmt::warn!("SD: terrain {} will not open — routes stay flat", defmt::Debug2Format(&name));
            return None;
        };
        let len = self.vmgr.file_length(file).unwrap_or(0);
        if len == 0 || len != entry_len {
            let _ = self.vmgr.close_file(file);
            return None;
        }
        let Ok(table) = SetExtentTable::build(self.card, entry_block, entry_offset, len) else {
            defmt::warn!("SD: terrain {} will not extent-map — routes stay flat", defmt::Debug2Format(&name));
            let _ = self.vmgr.close_file(file);
            return None;
        };
        // SAFETY: boot calls this once, after `open_map`; both slots are written exactly once and
        // the retained handle pins the chain the table describes for the session.
        let (table, source) = unsafe {
            let table = crate::init_static(core::ptr::addr_of_mut!(TERRAIN_EXTENTS), table);
            let source =
                crate::init_static(core::ptr::addr_of_mut!(TERRAIN_SOURCE), SetExtentSource::new(self.card, table));
            (&*table, &*source)
        };
        if !self.verify_extents(table, file, len) {
            defmt::warn!("SD: terrain {} failed extent verification — routes stay flat", defmt::Debug2Format(&name));
            let _ = self.vmgr.close_file(file);
            return None;
        }
        defmt::info!(
            "SD: terrain {} mounted ({=u32} B, {=usize} extent(s))",
            defmt::Debug2Format(&name),
            len,
            table.extent_count()
        );
        self.open_terrain = Some((file, len));
        Some(source)
    }

    /// Open every shard of one validated volume set, resolve one resident extent table/source per
    /// file, and retain every FAT handle for the session. No seek-path fallback is admitted for a
    /// set: the renderer fans out across files, so one fragmented shard must not quietly reinsert
    /// FAT walks into the hot dispatch path.
    #[inline(never)]
    fn open_volume_set(&mut self, chosen: &MapSummary) -> Option<u32> {
        let id = chosen.id?;
        let parsed = self.read_set_manifest(&chosen.file)?;
        if parsed.shard_count() > SD_SET_MAX_SHARDS
            || parsed.obcm_version != chosen.obcm_version
            || set_identity_from_manifest(&parsed).bbox != chosen.bbox
        {
            return None;
        }

        let mut opened: Vec<OpenSetShard, SD_SET_MAX_SHARDS> = Vec::new();
        for (index, record) in parsed.shards().iter().enumerate() {
            let Some(name) = set_shard_name_for(id, index) else {
                self.close_set_shards(&mut opened);
                return None;
            };
            let Some((entry_block, entry_offset, entry_len)) = self.find_root_entry(&name) else {
                self.close_set_shards(&mut opened);
                return None;
            };
            let Ok(file) = self.vmgr.open_file_in_dir(self.root, &name, Mode::ReadOnly) else {
                self.close_set_shards(&mut opened);
                return None;
            };
            let len = self.vmgr.file_length(file).unwrap_or(0);
            if len != record.bytes || entry_len != len {
                let _ = self.vmgr.close_file(file);
                self.close_set_shards(&mut opened);
                return None;
            }
            let table = match SetExtentTable::build(self.card, entry_block, entry_offset, len) {
                Ok(table) => table,
                Err(error) => {
                    defmt::warn!(
                        "SD: set shard {} extent table unavailable ({}) — refusing the set",
                        defmt::Debug2Format(&name),
                        defmt::Debug2Format(&error)
                    );
                    let _ = self.vmgr.close_file(file);
                    self.close_set_shards(&mut opened);
                    return None;
                }
            };
            // SAFETY: boot calls `open_map` once; every index addresses a distinct static slot,
            // written exactly once before any reference escapes. The retained handle pins the FAT
            // chain for the lifetime of both references.
            let (table, source) = unsafe {
                let table_slots =
                    core::ptr::addr_of_mut!(EXTENT_SLOTS.set).cast::<core::mem::MaybeUninit<SetExtentTable>>();
                let table = crate::init_static(table_slots.add(index), table);
                let source_slots =
                    core::ptr::addr_of_mut!(SET_SOURCES).cast::<core::mem::MaybeUninit<SetExtentSource<'static>>>();
                let source = crate::init_static(source_slots.add(index), SetExtentSource::new(self.card, table));
                (&*table, &*source)
            };
            if !self.verify_extents(table, file, len) {
                defmt::warn!(
                    "SD: set shard {} extent table failed verification — refusing the set",
                    defmt::Debug2Format(&name)
                );
                let _ = self.vmgr.close_file(file);
                self.close_set_shards(&mut opened);
                return None;
            }
            // The manifest was capped before the loop, so the store has one slot for every source.
            if let Err(shard) = opened.push(OpenSetShard { file, len, source }) {
                let _ = self.vmgr.close_file(shard.file);
                self.close_set_shards(&mut opened);
                return None;
            }
        }

        let core_index = parsed.core_shard();
        let Some(core_len) = opened.get(core_index).map(|shard| shard.len) else {
            self.close_set_shards(&mut opened);
            return None;
        };
        self.map_name = chosen.name.clone();
        defmt::info!(
            "SD: mounting set {} ({=usize} shards, {=u64} B)",
            defmt::Debug2Format(&chosen.file),
            parsed.shard_count(),
            parsed.total_bytes()
        );
        self.open_set = Some(OpenSet {
            id,
            manifest_name: chosen.file.clone(),
            core_index: core_index as u8,
            shards: opened,
            terrain_bytes: parsed.terrain().map(|record| record.bytes),
        });
        Some(core_len)
    }

    fn close_set_shards(&self, shards: &mut Vec<OpenSetShard, SD_SET_MAX_SHARDS>) {
        while let Some(shard) = shards.pop() {
            let _ = self.vmgr.close_file(shard.file);
        }
    }

    /// Mount the already-open set into the caller-placed reader store. The manifest and source-ref
    /// vector live only in this synchronous frame and are gone before `main` reaches another
    /// `.await`; [`obc_reader::MountedSet`] retains only the compact metadata it needs afterwards.
    #[inline(never)]
    pub(crate) fn mount_set(
        &self,
        store: &'static mut SetShardStore,
        tables: &'static obc_reader::MapTables,
        cache: &'static obc_reader::MapCache,
    ) -> Result<Option<obc_reader::MountedSet<'static>>, DeviceMountError> {
        let Some(open) = &self.open_set else { return Ok(None) };
        let manifest = self.read_set_manifest(&open.manifest_name).ok_or(DeviceMountError::Manifest)?;
        if manifest.core_shard() != open.core_index as usize {
            return Err(DeviceMountError::Manifest);
        }
        let mut sources: Vec<&'static dyn ByteSource, SD_SET_MAX_SHARDS> = Vec::new();
        for shard in &open.shards {
            sources.push(shard.source as &dyn ByteSource).map_err(|_| DeviceMountError::Sources)?;
        }
        obc_reader::MountedSet::mount(store, &manifest, sources.as_slice(), tables, cache)
            .map(Some)
            .map_err(DeviceMountError::Reader)
    }

    /// Which boot fault to put on glass when [`map_source`](Storage::map_source) hands out nothing.
    ///
    /// **NO MAP** unless [`open_map`](Storage::open_map) found a map-named file and could not stream
    /// from it — see [`map_boot_fault`](Storage::map_boot_fault) and `obc_app::boot_fault`, where the
    /// rule lives and is tested.
    fn boot_fault(&self) -> obc_app::BootFault {
        self.map_boot_fault.unwrap_or(obc_app::BootFault::NoMap)
    }

    /// Release boot-only map handles before the no-map USB recovery composition takes ownership.
    ///
    /// No reader task exists on that path, so the resident extent/source records are already dead;
    /// closing the handles simply returns their file slots and prevents a replacement upload from
    /// competing with the unreadable map it is replacing.
    pub fn prepare_map_recovery(&mut self) {
        #[cfg(has_nav)]
        if let Some((file, _)) = self.open_terrain.take() {
            let _ = self.vmgr.close_file(file);
        }
        if let Some((file, _)) = self.open_map.take() {
            let _ = self.vmgr.close_file(file);
        }
        if let Some(mut set) = self.open_set.take() {
            self.close_set_shards(&mut set.shards);
        }
        self.open_map_name = None;
        self.map_extents = None;
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
    /// shards, and reclaiming only the manifest would leave gigabytes of orphans behind; the set
    /// goes through [`Storage::delete_set`], which removes the manifest first (§5.4) and then every
    /// derived shard name. The returned count is files, not maps, for the same reason.
    ///
    /// **A set is retired against the mounted set when one loaded, otherwise against its own
    /// keeper.** A selected older set is an explicit choice and its retained handles make deleting
    /// it actively invalid. When a single-file map loaded instead, `obc_app::newest_set` names the
    /// independent `MS{id}` namespace's survivor. That claim is backed by proof: a set is listed
    /// only after `set_identity` validated the whole thing (§5.3), and a half-uploaded one has no
    /// manifest and is invisible (§5.4).
    ///
    /// `keep` is `None` only when nothing loaded. No single map is superseded then; a complete set
    /// may still retire an older set in its own namespace.
    fn retire_superseded_maps(&mut self, maps: &[MapSummary], keep: Option<usize>) -> usize {
        let choices = map_choices(maps);
        let set_keeper = obc_app::set_retirement_keeper(&choices, keep);
        // `(name, set id)` — a set is deleted by id (its shard names are derived), a single map by
        // name. Collected first because the scan borrow and the delete `&mut` cannot overlap.
        let mut doomed: Vec<(ShortFileName, Option<u16>), MAX_MAPS> = Vec::new();
        for (i, m) in maps.iter().enumerate() {
            let Some(keeper) = (if m.shards.is_some() { set_keeper } else { keep }) else { continue };
            if obc_app::is_superseded_upload(&choices, keeper, i) && !self.map_file_is(&m.file) {
                let _ = doomed.push((m.file.clone(), m.shards.and(m.id)));
            }
        }
        let mut retired = 0;
        for (name, set) in doomed {
            if let Some(id) = set {
                retired += self.delete_set(id);
                continue;
            }
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
    /// A volume set (`OBCA_Spec.md` §5) is listed as **one** map, keyed on its `MS{id}.OBS`
    /// manifest, and its `MS{id}S{kk}.OBM` shards are never listed at all — §5.4 is explicit that a
    /// shard opened alone is "exactly the kind of quiet wrongness a rider cannot diagnose" (a
    /// geometry shard has no roads and no POIs; the core draws nothing). [`is_map_entry`] excludes
    /// them by name, and [`Storage::set_identity`] refuses a set whose shards are not all present
    /// at the recorded size — so a mid-copy set is invisible rather than half a map.
    ///
    /// Two consequences of that exclusion, stated rather than discovered:
    ///
    /// - **A side-loaded file named like a shard is invisible.** A rider who hand-copies one map
    ///   onto the card as `MS4S00.OBM` gets nothing — no listing, no fault, no explanation. That is
    ///   the price of making §5.4 structural, and it is the right side to err on: a shard *looking*
    ///   like a map is the failure a rider cannot diagnose, while a file that does not appear is one
    ///   they can rename. A hand-copied whole **set** (manifest included) works exactly as an
    ///   uploaded one does.
    /// - **A set costs one open + one 40-byte header read per shard, per scan.** The scan runs at
    ///   boot and at each `next_map_id_from_scan`, so a 32-shard set is ~33 opens — bounded, but not
    ///   free, and the reason `set_identity` is the *only* thing that reads a shard at scan time
    ///   (no LOD tables, no style tables, no digests).
    pub fn scan_maps_into(&self, out: &mut Vec<MapSummary, MAX_MAPS>) -> usize {
        out.clear();
        let mut unlistable = 0usize;
        let selected = self.load_selected_map();
        // `manifest` distinguishes the two arms; everything else is the same directory-entry facts.
        // Two phases because the `iter_dir_lfn` callback borrows the manager and both identity
        // reads open a file.
        let mut entries: Vec<(ShortFileName, String<24>, embedded_sdmmc::BlockIdx, u32, u32, bool), MAX_MAPS> =
            Vec::new();
        self.iter_dir_lfn(self.root, |e, long| {
            let manifest = is_set_manifest_entry(e, long);
            if !manifest && !is_map_entry(e, long) {
                return;
            }
            let _ = entries.push((
                e.name.clone(),
                map_display_name(&e.name, long),
                e.entry_block,
                e.entry_offset,
                e.size,
                manifest,
            ));
        });
        for (file, name, entry_block, entry_offset, byte_len, manifest) in entries {
            let (id, shards, name, byte_len, obcm_version, bbox) = if manifest {
                let Some(id) = set_manifest_id(&file) else {
                    unlistable += 1;
                    continue;
                };
                let Some(set) = self.set_identity(&file, id) else {
                    defmt::info!(
                        "SD: {} names a volume set that does not validate — not listed (OBCA §5.4)",
                        defmt::Debug2Format(&file)
                    );
                    unlistable += 1;
                    continue;
                };
                // The manifest carries a real display name; the 8.3 stem (`MS7`) is the fallback.
                let display = if set.name.is_empty() { name } else { set.name };
                (
                    Some(id),
                    Some(set.shard_count),
                    display,
                    set.total_bytes + byte_len as u64,
                    set.obcm_version,
                    set.bbox,
                )
            } else {
                let Some((obcm_version, bbox)) = self.map_identity(&file) else {
                    unlistable += 1;
                    continue;
                };
                (uploaded_map_id(&file), None, name, byte_len as u64, obcm_version, bbox)
            };
            let selected = selected
                .as_ref()
                .is_some_and(|s| file.base_name() == s.base_name() && file.extension() == s.extension());
            let entry = MapSummary {
                id,
                file,
                shards,
                name,
                byte_len,
                obcm_version,
                bbox,
                selected,
                entry_block,
                entry_offset,
            };
            if out.push(entry).is_err() {
                defmt::warn!("SD: more than {=usize} maps on the card — the rest are not listed", MAX_MAPS);
                break;
            }
        }
        unlistable
    }

    /// The reader's half of `OBCA_Spec.md` §5.3 for a `MS{id}.OBS` manifest, at **scan** time:
    /// parse and validate the manifest itself, then check that every **OBCM shard** it names exists,
    /// is exactly the recorded `Bytes`, and opens as OBCM at the recorded version with the recorded
    /// header bbox. `None` means *this is not a map* — §5.4 admits no partial acceptance, so a set
    /// with a shard missing or still growing is simply absent from the catalog rather than a map
    /// with holes in it.
    ///
    /// **The `terrain` record is counted and not checked**, which is §5.3's one exception and is
    /// why the sentence above says *OBCM shard* rather than *record*: a raster that is absent or
    /// unreadable MUST NOT keep the set out of the catalog. See
    /// [`set_file_totals`](Self::set_file_totals).
    ///
    /// The SHA-256 digests are deliberately **not** checked: §5.3 lets a device defer them, and
    /// hashing gigabytes off an SD card is minutes of work at boot.
    ///
    /// `#[inline(never)]` and called only from the boot-time scan: the manifest buffer is
    /// [`obc_formats::obcs::MAX_MANIFEST_LEN`] = 1864 B of stack, which is fine here and would not
    /// be anywhere near the render path (the ~36 KB stack rule).
    #[inline(never)]
    fn set_identity(&self, manifest: &ShortFileName, id: u16) -> Option<SetIdentity> {
        let parsed = self.read_set_manifest(manifest)?;
        // A mounted set already owns one open handle per shard, and embedded-sdmmc deliberately
        // refuses opening the same file twice. Its manifest and every recorded size/header/bbox
        // were validated immediately before those handles were retained, so later catalog scans
        // may re-parse the small manifest but must not try to reopen its pinned shard files.
        if self.open_set.as_ref().is_some_and(|open| open.id == id && &open.manifest_name == manifest) {
            return Some(set_identity_from_manifest(&parsed));
        }
        // Listed, not hidden — it is a real map on the card and the rider must be able to see it —
        // but say now why it will never load, rather than at the failed open of shard 12.
        if parsed.shard_count() > SD_SET_MAX_SHARDS {
            defmt::warn!(
                "SD: volume set {} names {=usize} shards; this board mounts at most {=usize} (OBCA §5.2 allows 32)",
                defmt::Debug2Format(manifest),
                parsed.shard_count(),
                SD_SET_MAX_SHARDS
            );
        }

        let total = self.set_file_totals(&parsed, id)?;
        let mut identity = set_identity_from_manifest(&parsed);
        identity.total_bytes = total;
        Some(identity)
    }

    /// Read and parse one manifest. Kept out of the mount frame: the maximum 1,864-byte buffer is
    /// a boot/scan cost only and never reaches the render loop.
    #[inline(never)]
    fn read_set_manifest(&self, manifest: &ShortFileName) -> Option<obc_formats::obcs::SetManifest> {
        let mut buf = [0u8; obc_formats::obcs::MAX_MANIFEST_LEN];
        let file = self.vmgr.open_file_in_dir(self.root, manifest, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0) as usize;
        let read = if len >= obc_formats::obcs::HEADER_LEN && len <= buf.len() {
            let mut done = 0usize;
            while done < len {
                match self.vmgr.read(file, &mut buf[done..len]) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => done += n,
                }
            }
            done
        } else {
            0
        };
        let _ = self.vmgr.close_file(file);
        obc_formats::obcs::parse(buf.get(..read)?).ok()
    }

    /// The other half of `OBCA_Spec.md` §5.3: every record a parsed manifest names exists and is
    /// exactly the recorded `Bytes` — each OBCM shard opening as OBCM at the manifest's version
    /// with the recorded header bbox, and the terrain record (if any) opening as an OBCT container.
    /// Returns the set's total bytes on the card, or `None` for *this is not a map* — there is no
    /// partial acceptance.
    ///
    /// Shared by the two moments it has to be true: the boot scan, which decides whether a set on
    /// the card is listed at all, and the upload's own manifest commit, which decides whether the
    /// `OBCS` magic is allowed to be written. Running the same code at both is the point — a set
    /// this device accepts is by construction one it will still accept at the next boot.
    ///
    /// **The terrain record is counted here and never judged here** (#1044), and that is a rule
    /// [`OBCA_Spec.md` §5.3](../../../specs/OBCA_Spec.md) states in so many words: *a missing or
    /// unreadable terrain shard does not fail the mount — a reader MUST mount such a set, MUST fall
    /// back to no elevation, and MUST NOT present it as a fault.* This function's `None` is
    /// "**this is not a map**": the boot scan drops the set from the catalog and counts it toward
    /// the unlistable tally that raises MAP UNREADABLE. Letting a raster reach that verdict would
    /// take a rider's entire map away because they deleted an `.OBD` to reclaim space, or because a
    /// hand-copied one was truncated, or because one 32-byte header read glitched at boot, or
    /// because a future OBCT version bump made every card on earth stop listing.
    ///
    /// So the record contributes its **recorded** `Bytes` to the total and nothing else happens.
    /// The manifest is the authority on what the set claims to be (it is what `total_bytes()` sums
    /// too), the figure stays stable whatever state the file is in, and the scan does no I/O for it
    /// at all. Whether the raster actually opens is decided later and locally, by
    /// [`open_terrain`](Self::open_terrain), which already degrades to *no elevation* by design.
    ///
    /// The **upload's** commit is the one place that judges it, because there the two ends built
    /// the manifest and the raster together seconds ago — see
    /// [`validate_committed_manifest`](Self::validate_committed_manifest) and the asymmetry written
    /// out in `obc_app::terrain_record_agrees`.
    fn set_file_totals(&self, parsed: &obc_formats::obcs::SetManifest, id: u16) -> Option<u64> {
        let mut total = parsed.terrain().map_or(0u64, |terrain| terrain.bytes as u64);
        for (index, shard) in parsed.shards().iter().enumerate() {
            let name = set_shard_name_for(id, index)?;
            let (bytes, version, bbox) = self.shard_identity(&name)?;
            if bytes != shard.bytes || version != parsed.obcm_version {
                return None;
            }
            let recorded = BBox {
                min_lat: shard.bbox.min_lat,
                min_lon: shard.bbox.min_lon,
                max_lat: shard.bbox.max_lat,
                max_lon: shard.bbox.max_lon,
            };
            if bbox != recorded {
                return None;
            }
            total += bytes as u64;
        }
        Some(total)
    }

    /// The terrain shard's byte length, or `None` when the file is absent, unreadable, or does not
    /// open as an OBCT container this firmware reads (#1044).
    fn terrain_shard_len(&self, name: &ShortFileName) -> Option<u32> {
        let file = self.vmgr.open_file_in_dir(self.root, name, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0);
        let mut header = [0u8; obc_formats::obct::HEADER_LEN];
        let src = SdByteSource::new(&self.vmgr, file, len);
        let ok = (len as usize) >= header.len() && src.read_at(0, &mut header).is_ok();
        let _ = self.vmgr.close_file(file);
        (ok && obc_formats::obct::validate_header_prefix(&header).is_ok()).then_some(len)
    }

    /// One shard's `(byte length, OBCM version, header bbox)`, or `None` when the file is absent,
    /// unreadable, or not an OBCM file. A shard is never the open map (§5.4 forbids mounting one
    /// standalone), so this always opens fresh — no [`Storage::map_file_is`] detour.
    fn shard_identity(&self, name: &ShortFileName) -> Option<(u32, u8, BBox)> {
        let file = self.vmgr.open_file_in_dir(self.root, name, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0);
        let mut header = [0u8; obc_formats::obcm::HEADER_LEN];
        let src = SdByteSource::new(&self.vmgr, file, len);
        let ok = (len as usize) >= header.len() && src.read_at(0, &mut header).is_ok();
        let _ = self.vmgr.close_file(file);
        if !ok || header[0..4] != obc_formats::obcm::MAGIC {
            return None;
        }
        let rd = |o: usize| i32::from_le_bytes([header[o], header[o + 1], header[o + 2], header[o + 3]]);
        Some((len, header[4], BBox { min_lat: rd(5), min_lon: rd(9), max_lat: rd(13), max_lon: rd(17) }))
    }

    /// Delete a whole volume set: execute `obc_formats::obcs::delete_plan`, which is the ordered
    /// name list §5.4 mandates — the manifest **first**, then every derived shard name to the cap.
    ///
    /// The plan is a pure function so the ordering (the normative part) is asserted where tests
    /// run; this is only the execution, and it adds the one rule a name list cannot express: **if
    /// the manifest survives, stop**. The manifest is the atomicity token, so removing it first
    /// means a power cut mid-delete leaves *orphans* — files no manifest references, invisible as a
    /// map and reclaimable — never a manifest pointing at files that are gone. Half-deleting the
    /// shards of a set whose manifest is still there would produce exactly that broken state.
    ///
    /// **A manifest that is not there is not a manifest that survived.** `NotFound` is the one
    /// failure that proves the opposite of the rule — there is nothing left to point at the shards —
    /// so it continues rather than stopping (issue #1039). Treating it as a refusal made §5.4's
    /// replace-clear a no-op in exactly the case it exists for: an id carrying shards and no
    /// manifest. The upload that followed would write its own manifest over that id and the
    /// previous occupant's higher-index shards would be stranded **permanently** — shielded from
    /// the orphan sweep by the valid manifest beside them, and named by nothing.
    ///
    /// Returns how many files were reclaimed.
    fn delete_set(&mut self, id: u16) -> usize {
        if self.open_set.as_ref().is_some_and(|set| set.id == id) {
            defmt::warn!("SD: refusing to delete mounted volume set MS{=u16}", id);
            return 0;
        }
        let Some(plan) = obc_formats::obcs::delete_plan(id) else {
            defmt::warn!("SD: volume set {=u16} has no derived 8.3 names — nothing to delete", id);
            return 0;
        };
        let mut removed = 0usize;
        for (step, derived) in plan.iter().enumerate() {
            let Some(name) = ShortFileName::create_from_str(derived.as_str()).ok() else { continue };
            match self.vmgr.delete_file_in_dir(self.root, &name) {
                Ok(()) => removed += 1,
                Err(embedded_sdmmc::Error::NotFound) => {}
                Err(e) if step == 0 => {
                    defmt::warn!(
                        "SD: could not delete the set manifest {} ({}) — leaving its shards alone",
                        defmt::Debug2Format(&name),
                        defmt::Debug2Format(&e)
                    );
                    return 0;
                }
                // Any other per-shard failure: the plan runs to the 32-shard cap, so most names are
                // simply absent, and one that refuses to go is left for the next sweep rather than
                // aborting the reclaim of the rest.
                Err(_) => {}
            }
        }
        // Silent when there was nothing under this id — a first upload clears its freshly minted id
        // before it writes, and "removed volume set MS7 (0 files)" describes nothing that happened.
        if removed > 0 {
            defmt::info!("SD: removed volume set MS{=u16} ({=usize} files)", id, removed);
        }
        removed
    }

    /// One map's `(obcm_version, bbox)` from its 40-byte header, or `None` when the file is shorter
    /// than a header, unreadable, or doesn't carry the `OBCM` magic (a torn upload, or clutter that
    /// happens to sit on an `.OBM`/`.obcm` name).
    ///
    /// The **version is returned, not checked**: a map built for another OBCM version is still a map
    /// and still belongs in the catalog — the consumer decides. Only the magic gates membership.
    ///
    /// The currently-open map is read **through its existing handle**: embedded-sdmmc refuses every
    /// second open of an open file (`FileAlreadyOpen`), which would otherwise drop the loaded map out
    /// of its own catalog — the same trap `route_object_info` documents (issue #480).
    fn map_identity(&self, name: &ShortFileName) -> Option<(u8, BBox)> {
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
        let rd = |o: usize| i32::from_le_bytes([header[o], header[o + 1], header[o + 2], header[o + 3]]);
        // Header field order is lat, lon, lat, lon (OBCM_Spec.md §1) — not the order bbox code expects.
        let bbox = BBox { min_lat: rd(5), min_lon: rd(9), max_lat: rd(13), max_lon: rd(17) };
        Some((header[4], bbox))
    }

    /// Whether `name` is the map file currently held open — the guard that routes
    /// [`map_identity`](Self::map_identity) through the live handle instead of a refused second open.
    fn map_file_is(&self, name: &ShortFileName) -> bool {
        (self.open_map.is_some() && self.open_map_name.as_ref() == Some(name))
            || self.open_set.as_ref().is_some_and(|set| &set.manifest_name == name)
    }

    /// The card's recorded map selection ([`MAP_SELECTED`]), or `None` for absent / torn / a name
    /// this device would never have written — all of which mean **no preference**.
    fn load_selected_map(&self) -> Option<ShortFileName> {
        let name = ShortFileName::create_from_str(MAP_SELECTED).ok()?;
        let file = self.vmgr.open_file_in_dir(self.root, &name, Mode::ReadOnly).ok()?;
        let mut buf = [0u8; obc_app::store_meta::SELECTED_MAP_LEN];
        let n = self.vmgr.read(file, &mut buf).unwrap_or(0);
        let _ = self.vmgr.close_file(file);
        ShortFileName::create_from_str(obc_app::store_meta::decode_selected_map(&buf[..n])?).ok()
    }

    /// Record which map the renderer should stream from, as a truncating rewrite of
    /// [`MAP_SELECTED`]. Best-effort by design: a failed write leaves the previous selection (or
    /// none), and the loader's fallback still lands on a map — so a map upload never fails because
    /// the *preference* couldn't be persisted, it just may not come up first.
    fn save_selected_map(&mut self, name: &ShortFileName) -> bool {
        let mut text: String<16> = String::new();
        for &b in name.base_name().iter().take_while(|&&b| b != b' ') {
            let _ = text.push(b as char);
        }
        let ext = name.extension();
        if !ext.is_empty() {
            let _ = text.push('.');
            for &b in ext.iter().take_while(|&&b| b != b' ') {
                let _ = text.push(b as char);
            }
        }
        let Some(bytes) = obc_app::store_meta::encode_selected_map(text.as_str()) else {
            defmt::warn!(
                "SD: map selection {} is not a writable 8.3 name — selection unchanged",
                defmt::Debug2Format(name)
            );
            return false;
        };
        match self.vmgr.open_file_in_dir(self.root, MAP_SELECTED, Mode::ReadWriteCreateOrTruncate) {
            Ok(file) => {
                let ok = self.vmgr.write(file, &bytes).is_ok()
                    && self.vmgr.flush_file(file).is_ok()
                    && self.vmgr.close_file(file).is_ok();
                if !ok {
                    defmt::warn!("SD: could not persist the map selection — the loader falls back");
                }
                ok
            }
            Err(e) => {
                defmt::warn!("SD: cannot open /MAP.SEL: {}", defmt::Debug2Format(&e));
                false
            }
        }
    }

    /// The loaded map's display name (T8 item 6) — its filename stem, or `""` before a map opens.
    fn map_name(&self) -> &str {
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
                // Into the union's `.bss` slot before it can be captured anywhere by value (see
                // `EXTENT_SLOTS`). SAFETY: `open_map` makes one map-kind choice once per boot, so
                // this is the sole write to the union and no set-slot reference can exist. It must
                // never be overwritten after the `'static` reference escapes.
                let table: &'static ExtentTable = unsafe {
                    let table_slot =
                        core::ptr::addr_of_mut!(EXTENT_SLOTS.map).cast::<core::mem::MaybeUninit<ExtentTable>>();
                    crate::init_static(table_slot, table)
                };
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
    fn verify_extents<const N: usize>(&self, table: &ExtentTableWithCapacity<N>, file: RawFile, len: u32) -> bool {
        let slow = Source::new(&self.vmgr, file, len);
        let fast = ExtentSourceWithCapacity::new(self.card, table);
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

    /// A [`ByteSource`](obc_formats::io::ByteSource) over the open map file, for reading the header
    /// ([`obc_reader::MapTables::parse`]) or building a per-frame [`Reader`](obc_reader::Reader). `None` if
    /// no map was opened ([`open_map`](Self::open_map) returned `None`). Cheap — the source just wraps
    /// the already-open handle, so it's rebuilt every redraw, keeping no borrow across the `&mut self`
    /// route/track operations. Extent-mapped direct block reads when the table built (#500), the
    /// manager's seek path otherwise.
    fn map_source(&self) -> Option<MapSource<'_>> {
        if let Some(set) = &self.open_set {
            let core = set.shards.get(set.core_index as usize)?;
            return Some(MapSource::Set(core.source));
        }
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

    /// Release retained route geometry. The borrowed repository resolves/open a selected row.
    fn close_route(&mut self) {
        if let Some((_, f, _)) = self.open_route.take() {
            let _ = self.vmgr.close_file(f);
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

    /// A [`ByteSink`](obc_formats::io::ByteSink) over the open nav-route file — what
    /// [`plan_route`](obc_route::plan_route) streams the emitted OBCR through.
    pub fn nav_sink(&self, file: RawFile) -> Sink<'_> {
        SdByteSink::new(&self.vmgr, file)
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

    /// Read a `/routes` object through a matching retained detail handle, or a scoped fresh handle.
    /// The callback never outlives this call and a fresh handle is closed on success or refusal.
    fn with_routes_object<T>(
        &self,
        name: &ShortFileName,
        read: impl FnOnce(&Source<'_>, u32) -> Option<T>,
    ) -> Option<T> {
        if let Some(open) = &self.open_object {
            if name == &open.name {
                return read(&SdByteSource::new(&self.vmgr, open.file, open.len), open.len);
            }
        }
        let file = self.vmgr.open_file_in_dir(self.routes_dir?, name, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0);
        let out = read(&SdByteSource::new(&self.vmgr, file, len), len);
        let _ = self.vmgr.close_file(file);
        out
    }

    /// Read route-list facts through retained geometry/detail handles or a scoped fresh handle.
    fn route_object_info(&self, catalog: &StoredRouteCatalog, name: &ShortFileName) -> Option<(u32, RouteObjectInfo)> {
        if let Some((id, file, len)) = self.open_route {
            if catalog.get(id).is_some_and(|(_, open_name, _)| open_name == name) {
                let info = RouteObjectInfo::read(&SdByteSource::new(&self.vmgr, file, len)).ok()?;
                return Some((len, info));
            }
        }
        self.with_routes_object(name, |source, len| Some((len, RouteObjectInfo::read(source).ok()?)))
    }

    /// Whether a `/routes` object carries the exact zero-magic interrupted-commit signature.
    fn is_aborted_commit(&self, name: &ShortFileName) -> bool {
        self.with_routes_object(name, |source, _| {
            let mut magic = [0u8; 4];
            ByteSource::read_at(source, 0, &mut magic).ok()?;
            Some(magic == [0; 4])
        })
        .unwrap_or(false)
    }

    /// Open the ordinary upload temp for one exact wire and repository. A live slot is busy rather
    /// than being stolen; the returned raw-handle capability keys every later operation.
    pub fn upload_begin(&mut self, owner: GateOwner, ty: ObjectType) -> Option<UploadSession> {
        if self.open_upload.is_some() {
            return None;
        }
        let destination = UploadDestination::from_object_type(ty)?;
        let dir = self.routes_dir_or_create()?;
        match self.vmgr.open_file_in_dir(dir, UPLOAD_TMP, Mode::ReadWriteCreateOrTruncate) {
            Ok(file) => {
                self.open_upload = Some((file, UploadOwner::Temp(owner, destination)));
                Some(SinkSession::new(file))
            }
            Err(e) => {
                defmt::warn!("SD: cannot open upload temp: {}", defmt::Debug2Format(&e));
                None
            }
        }
    }

    fn upload_file(&self, session: Option<UploadSession>) -> Option<RawFile> {
        match (self.open_upload, session) {
            (Some((file, UploadOwner::Temp(..))), Some(session)) if session.matches_key(file) => Some(file),
            (Some((file, owner)), None) if !matches!(owner, UploadOwner::Temp(..)) => Some(file),
            _ => None,
        }
    }

    /// Append only when the ordinary session matches, or when the typed map/set path supplies no
    /// ordinary token.
    pub fn upload_append(&mut self, session: Option<UploadSession>, bytes: &[u8]) -> bool {
        let Some(file) = self.upload_file(session) else { return false };
        if upload_pipe_enabled() && upload_pipe_finish_active().is_err() {
            let _ = upload_pipe_end();
            return false;
        }
        if self.vmgr.write(file, bytes).is_err() {
            let _ = upload_pipe_end();
            return false;
        }
        if upload_pipe_enabled() && upload_pipe_start_queued().is_err() {
            let _ = upload_pipe_end();
            return false;
        }
        true
    }

    /// Enable the deferred/coalesced writer for a staged USB map. This is deliberately separate
    /// from reservation: BLE and unstaged fallbacks use the same pre-allocation but cannot promise
    /// the double-buffer lifetime the FLPR DMA requires after `BlockDevice::write` returns.
    pub fn upload_fast_begin(&mut self) -> bool {
        self.open_upload.is_some() && upload_pipe_begin()
    }

    /// Join the last deferred write before format validation or magic commit.
    pub fn upload_sync(&mut self) -> bool {
        self.open_upload.is_some() && (!upload_pipe_enabled() || upload_pipe_flush().is_ok())
    }

    /// Preallocate the announced chain before streaming. A refusal only costs throughput:
    /// `VolumeManager::write` still extends it cluster by cluster. `total_len` includes a map's
    /// already-written magic, so a successful reservation has no unused tail.
    pub fn upload_reserve(&mut self, session: Option<UploadSession>, total_len: u32) -> bool {
        let Some(file) = self.upload_file(session) else { return false };
        match self.vmgr.preallocate(file, total_len) {
            Ok(clusters) => {
                defmt::info!("SD: reserved {=u32} cluster(s) for a {=u32} B upload", clusters, total_len);
                true
            }
            Err(e) => {
                // Not a failure of the upload. The chain simply grows the old way from here, which
                // is slower and just as correct — so this is a warn, not a rejection.
                defmt::warn!("SD: upload pre-allocation refused ({}) — streaming unreserved", defmt::Debug2Format(&e));
                false
            }
        }
    }

    /// Flush + close the streaming handle, keeping the bytes on the card before its repository or
    /// map/set commit re-opens it to validate and publish.
    fn upload_close(&mut self) {
        if upload_pipe_enabled() && !upload_pipe_end() {
            defmt::warn!("SD: deferred upload write failed while closing — target remains inert");
        }
        if let Some((file, _)) = self.open_upload.take() {
            let _ = self.vmgr.flush_file(file);
            let _ = self.vmgr.close_file(file);
        }
    }

    /// Close exactly this ordinary session for repository validation/promotion.
    fn upload_take(&mut self, session: UploadSession, expected: UploadDestination) -> bool {
        let Some((file, UploadOwner::Temp(_, actual))) = self.open_upload else { return false };
        if !session.matches(file, actual, expected) {
            return false;
        }
        self.upload_close();
        true
    }

    /// Abort exactly this ordinary session; stale, wrong-wire and map/set tokens are no-ops.
    pub fn upload_abort(&mut self, session: UploadSession) {
        if self.upload_file(Some(session)).is_none() {
            return;
        }
        self.upload_close();
        if let Some(dir) = self.routes_dir {
            let _ = self.vmgr.delete_file_in_dir(dir, UPLOAD_TMP);
        }
    }

    /// Tear down only the ordinary session owned by this wire; an idle or stale other-wire reset is
    /// a no-op.
    pub fn upload_abort_owner(&mut self, owner: GateOwner) {
        let Some((file, UploadOwner::Temp(actual, _))) = self.open_upload else { return };
        if actual == owner {
            self.upload_abort(SinkSession::new(file));
        }
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
    pub fn commit_fwimage(&mut self, session: UploadSession) -> Option<u32> {
        if !self.upload_take(session, UploadDestination::Firmware) {
            return None;
        }
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

    // ==================== the map upload (issue #927) ====================
    //
    // **Why this path is not the route path.** Every other upload streams into `/routes/UPLOAD.TMP`
    // and is *copied* to its final name at commit with the magic held back — an invisible temp, an
    // atomic promote, no half-object ever visible. A map cannot afford that copy: at hundreds of
    // megabytes it would double both the write time and the free space the card
    // must have, to buy atomicity for a file that is, uniquely, re-derivable from the builder that
    // made it.
    //
    // So a map streams **straight into its final `MP{id}.OBM`** and gets the same commit point
    // another way: the file opens with four zero bytes standing in for the magic, the stream's own
    // first four bytes are withheld by the caller's `obc_ble::HeldMagic`, and `map_upload_commit`
    // patches them in after the upload's transport policy and the header have both validated. The torn state
    // is byte-identical to the copy path's — a zero-magic file `is_map_entry` may list but
    // `map_identity` refuses, so it never reaches a catalog, is never chosen by `open_map`, and is
    // reclaimed by the boot sweep.
    //
    // The consequence is the one policy this path enforces: a map upload is **new-only**. Writing
    // into an existing map's file would destroy it as the new bytes arrive, and §4.2's "a failed upload
    // never touches the old copy" is not a guarantee to give up on the one object the device cannot
    // rebuild. `TransferStatus::map_announce_reject` turns every named id away before a byte moves;
    // replacing a map is "upload the new one, then delete the old one".

    /// One past the highest map object id on the card — the scan half of the `max(scan_max + 1,
    /// RRAM floor)` allocation every durable id namespace uses (spec §4.1). A zero-magic torn upload
    /// is invisible to the scan, so a retried transfer re-derives the *same* id and truncates the
    /// file it abandoned, rather than leaking one id per interruption.
    ///
    /// Volume sets are skipped: `MP{id}.OBM` and `MS{id}.OBS` (`OBCA_Spec.md` §5.2) are separate
    /// namespaces with separate allocators, so a set on the card must not push this counter — the
    /// two conventions coexist precisely because neither constrains the other.
    fn next_map_id_from_scan(&self) -> u16 {
        let mut next: u32 = 0;
        let mut maps: Vec<MapSummary, MAX_MAPS> = Vec::new();
        // The scan's unlistable count is the fault card's business, not the allocator's: a torn
        // upload has to stay invisible *here* so a retry re-derives the same id and truncates the
        // file it abandoned (see the paragraph above).
        self.scan_maps_into(&mut maps);
        for m in maps.iter().filter(|m| m.shards.is_none()) {
            if let Some(id) = m.id {
                next = next.max(id as u32 + 1);
            }
        }
        next.min(u16::MAX as u32) as u16
    }

    /// Open `MP{id}.OBM` (truncating) for a map upload and write the four zero bytes that stand in
    /// for the held-back magic, so the file is length-correct from the first appended byte and inert
    /// from the moment it exists. `false` = the card refused; the caller answers `error` and no file
    /// is left behind that a scan would show.
    fn map_upload_begin(&mut self, id: u16) -> bool {
        self.upload_close();
        let Some(name) = map_file_name_for(id) else { return false };
        // `name_is_free` is the same discipline as `fresh_upload_name`: only a confirmed `NotFound`
        // green-lights a truncating create, so a transient bus error can never overwrite a stored map.
        // A *zero-magic* file under this name is the exception — it is our own abandoned transfer,
        // invisible to every catalog, and truncating it is the whole point of re-deriving the id.
        let mut text: String<12> = String::new();
        let _ = core::fmt::Write::write_fmt(&mut text, format_args!("MP{id}.OBM"));
        if !self.name_is_free(self.root, text.as_str()) && self.map_identity(&name).is_some() {
            defmt::warn!("SD: map name MP{=u16}.OBM is taken by a stored map — refusing to overwrite", id);
            return false;
        }
        match self.vmgr.open_file_in_dir(self.root, text.as_str(), Mode::ReadWriteCreateOrTruncate) {
            Ok(file) => {
                if self.vmgr.write(file, &[0u8; 4]).is_err() {
                    defmt::warn!("SD: cannot start MP{=u16}.OBM — the card refused the first write", id);
                    let _ = self.vmgr.close_file(file);
                    return false;
                }
                self.open_upload = Some((file, UploadOwner::Map));
                defmt::info!("SD: map upload streaming into /MP{=u16}.OBM (magic held back)", id);
                true
            }
            Err(e) => {
                defmt::warn!("SD: cannot create /MP{=u16}.OBM: {}", id, defmt::Debug2Format(&e));
                false
            }
        }
    }

    /// Commit a streamed map: flush + close the handle, validate the 40-byte header with the caller's
    /// held-back `magic` patched over the placeholder, then write that magic to bytes `0..4` — the
    /// commit point. Returns the stored byte length, or `None` with the file **deleted** (an invalid
    /// payload is not a map and must not linger as a zero-magic decoy the sweep would have to reason
    /// about).
    ///
    /// The header check is what stops a non-map after the USB/sEMMC link and media checks. It is the
    /// direct analogue of the route path's OBCR parse and the fwImage path's OBCU decode, and it
    /// rejects a map built for another OBCM version too: the device would only reach the *MAP
    /// UNREADABLE* fault screen on the next boot, which is a much worse way to learn it.
    fn map_upload_commit(&mut self, id: u16, magic: [u8; 4]) -> Option<u32> {
        self.upload_close();
        let name = map_file_name_for(id)?;
        let file = self.vmgr.open_file_in_dir(self.root, &name, Mode::ReadWriteAppend).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0);
        let mut header = [0u8; obc_formats::obcm::HEADER_LEN];
        let read = self.vmgr.file_seek_from_start(file, 0).is_ok()
            && matches!(self.vmgr.read(file, &mut header), Ok(n) if n == header.len());
        header[0..4].copy_from_slice(&magic);
        if !read || obc_formats::obcm::validate_header_prefix(&header).is_err() {
            let _ = self.vmgr.close_file(file);
            let _ = self.vmgr.delete_file_in_dir(self.root, &name);
            defmt::warn!(
                "SD: map upload is not a readable OBCM v{=u8} — rejected, file deleted",
                obc_formats::obcm::VERSION
            );
            return None;
        }
        // The commit point: the real magic replaces the placeholder, then a flush makes it durable.
        // Until this lands the file is inert to every reader on the device.
        let patched = self.vmgr.file_seek_from_start(file, 0).is_ok()
            && self.vmgr.write(file, &magic).is_ok()
            && self.vmgr.flush_file(file).is_ok();
        let _ = self.vmgr.close_file(file);
        if !patched {
            defmt::warn!(
                "SD: map magic patch failed — /MP{=u16}.OBM left zero-magic (inert; the boot sweep reclaims it)",
                id
            );
            return None;
        }
        defmt::info!("SD: map committed → /MP{=u16}.OBM ({=u32} B)", id, len);
        Some(len)
    }

    /// Abandon an in-flight map upload: close the handle and **delete** the partial.
    ///
    /// The file is already inert — its magic was never patched in, so no catalog lists it and no
    /// loader picks it — and the boot sweep would reclaim it eventually. Waiting for that boot is
    /// still wrong: the id is spent at `begin` (the durable floor advances there, so an id is never
    /// re-issued), which means a *retry* streams into a **new** filename and the abandoned one keeps
    /// its clusters. Three retries of a 300 MB map would strand nearly a gigabyte until the next
    /// power cycle, on the device least able to spare it. Deleting is cheap by comparison — a FAT
    /// chain walk, not a rewrite — so the sweep is left to do only what nothing running can: clean up
    /// after a power cut.
    fn map_upload_abort(&mut self, id: u16) {
        self.upload_close();
        let Some(name) = map_file_name_for(id) else { return };
        // Never delete a *committed* map: `map_upload_commit` may have already patched the magic in
        // and this call be a late cleanup, and the sweep's rule applies here too — only the exact
        // zero-magic torn signature is ours to remove.
        if self.map_identity(&name).is_some() {
            return;
        }
        match self.vmgr.delete_file_in_dir(self.root, &name) {
            Ok(()) => defmt::info!("SD: abandoned map upload /MP{=u16}.OBM deleted", id),
            Err(e) => defmt::warn!(
                "SD: could not delete the abandoned /MP{=u16}.OBM ({}) — the boot sweep will reclaim it",
                id,
                defmt::Debug2Format(&e)
            ),
        }
    }

    /// Sweep abandoned map uploads from the card root (issue #927): delete every `MP*.OBM` whose
    /// held-back magic was never patched in. Run once at boot, the map twin of the route/trip
    /// `is_aborted_commit` sweep — without it an interrupted transfer's hundreds of megabytes would
    /// sit on the card forever, invisible to every catalog that could explain them. Returns how many
    /// were reclaimed.
    fn sweep_aborted_maps(&mut self) -> usize {
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
    /// an unreadable file is **not** claimed (a bus glitch must never green-light a delete).
    fn is_zero_magic_root(&self, name: &ShortFileName) -> bool {
        matches!(self.root_magic(name), obc_app::RootMagic::Bytes([0, 0, 0, 0]))
    }

    /// What reading a card-root file's first four bytes produced. The raw read behind
    /// [`is_zero_magic_root`], kept separate because the set sweep feeds it to a pure verdict
    /// (`obc_app::sweep_verdict`) that needs all three answers apart.
    ///
    /// The distinction that matters is **short vs unopenable** (issue #1039): a file that opens and
    /// holds fewer than four bytes is one this device created and did not get to write, and folding
    /// it in with "the card refused" left that state on the card forever.
    fn root_magic(&self, name: &ShortFileName) -> obc_app::RootMagic {
        let Ok(file) = self.vmgr.open_file_in_dir(self.root, name, Mode::ReadOnly) else {
            return obc_app::RootMagic::Unreadable;
        };
        let mut magic = [0u8; 4];
        let read = self.vmgr.read(file, &mut magic);
        let _ = self.vmgr.close_file(file);
        match read {
            Ok(4) => obc_app::RootMagic::Bytes(magic),
            // A short read is the file's whole length: `read` fills what there is and returns it.
            Ok(_) => obc_app::RootMagic::Short,
            Err(_) => obc_app::RootMagic::Unreadable,
        }
    }

    // ==================== the volume-set upload (issue #1039) ====================
    //
    // A set is `1..=32` shard files plus one manifest (`OBCA_Spec.md` §5), so where a single map is
    // one transfer this is a *sequence* of them — and §5.4's atomicity rule is a rule about that
    // sequence: **the manifest is written last**, so a half-uploaded set has no manifest and is
    // invisible as a map.
    //
    // Every individual file rides the map path's shape unchanged: stream straight into the final
    // 8.3 name with the format magic held back, patch it in after the bytes validate. What is new
    // is one level of the same trick applied to the *set*:
    //
    //   `set_upload_begin` creates `MS{id}.OBS` holding four zero bytes **before the first shard
    //   streams**, and `set_manifest_commit` patches `OBCS` in as the very last write of the whole
    //   set.
    //
    // That placeholder is the set's commit point *and* its abandoned-upload signature. Until the
    // magic lands, `scan_maps_into` refuses the manifest (`obcs::parse` rejects the magic) so the
    // set is not a map; and a zero-magic `.OBS` is something only this device produces, because a
    // set arriving over a card reader is copied from a host that already holds a whole manifest.
    // `sweep_aborted_sets` therefore reclaims a torn set with no age rule and no risk to a rider
    // who is mid-copy — see `obc_app::set_upload::sweep_verdict`.
    //
    // **Set ids are card-derived only, with no RRAM floor** — the one place this deliberately
    // diverges from `MP{id}`. A map's id is a durable *protocol* object id (spec §4.1: never
    // re-issued within a store epoch, persisted by the phone, echoed back on reconnect). A set id
    // is none of those things: nothing enumerates it, no command takes it, and the only place it
    // appears is the filename. Burning one of the 1,000 ids §5.2's 8.3 names can express on every
    // interrupted upload would cost the namespace for no invariant. Reuse is made safe instead by
    // `set_upload_begin` running the whole `delete_plan` first, which is also §5.4's own rule for a
    // writer replacing a set: remove the old manifest before overwriting any of its shards.

    /// One past the highest volume-set id the card carries — the set twin of
    /// [`next_map_id_from_scan`](Self::next_map_id_from_scan), over the `MS{id}` namespace.
    ///
    /// Counts **every `MS`-named entry the card root holds** — manifests and shards alike, listed
    /// or not (issue #1039). The obvious cheaper rule, "one past the highest set the catalog
    /// listed", is wrong in the one case it has to be right about: a set arriving over a card
    /// reader, shards first, has no manifest yet, so it is listed nowhere — and it is the exact
    /// shape [`sweep_aborted_sets`](Self::sweep_aborted_sets) goes out of its way to spare. Minting
    /// its id and then clearing it (§5.4's replace rule) would delete, at the next upload, the map
    /// the sweep deliberately protected an hour earlier.
    ///
    /// So the allocator's question is not "which ids are maps" but "which ids are **spoken for**",
    /// and a filename is the whole answer. That also makes it cheap: names come off the directory
    /// entries, where the old rule opened and header-read every shard of every set.
    ///
    /// The cost, stated: an id occupied by debris stays occupied until something reclaims it — but
    /// only until the *boot*, because the sweep runs before any upload can ask, and a torn upload
    /// inside a session is deleted the moment the cable drops. No id is leaked across a restart,
    /// which is the property the no-RRAM-floor decision rests on.
    ///
    /// Saturates at `MAX_SET_ID + 1`, which is deliberately **one past** the largest derivable name
    /// rather than at it: the caller's exhaustion test is `id > MAX_SET_ID`, so clamping to 999
    /// would hand back an id a card holding `MS999` is *already using* — and the caller clears an id
    /// before it writes to it, which would delete a good map. `1000` has no 8.3 name, so it can only
    /// ever be refused.
    fn next_set_id_from_scan(&self) -> u16 {
        let mut next: u32 = 0;
        self.iter_dir_lfn(self.root, |e, _| {
            let short = short_name_bytes(&e.name);
            let id = obc_formats::obcs::parse_manifest_name(&short)
                .or_else(|| obc_formats::obcs::parse_shard_name(&short).map(|(id, _)| id));
            if let Some(id) = id {
                next = next.max(id as u32 + 1);
            }
        });
        next.min(obc_formats::obcs::MAX_SET_ID as u32 + 1) as u16
    }

    /// Open a set-upload session on the card: clear anything already stored under `id` (§5.4's
    /// "delete the old manifest **before** overwriting any of its shards"), then write the
    /// zero-magic `MS{id}.OBS` token that marks the set as in-flight.
    ///
    /// `false` = the card refused; the caller answers `error` and the session never opens.
    fn set_upload_begin(&mut self, id: u16) -> bool {
        self.upload_close();
        self.delete_set(id);
        let Some(name) = obc_formats::obcs::manifest_name(id) else { return false };
        match self.vmgr.open_file_in_dir(self.root, name.as_str(), Mode::ReadWriteCreateOrTruncate) {
            Ok(file) => {
                let wrote = self.vmgr.write(file, &[0u8; 4]).is_ok() && self.vmgr.flush_file(file).is_ok();
                let _ = self.vmgr.close_file(file);
                if !wrote {
                    defmt::warn!("SD: cannot start volume set MS{=u16} — the card refused the token write", id);
                    let _ = self.vmgr.delete_file_in_dir(self.root, name.as_str());
                    return false;
                }
                defmt::info!("SD: volume set MS{=u16} opened (manifest token written, magic held back)", id);
                true
            }
            Err(e) => {
                defmt::warn!("SD: cannot create /MS{=u16}.OBS: {}", id, defmt::Debug2Format(&e));
                false
            }
        }
    }

    /// Open shard `index` of set `id` for streaming — `MS{id}S{kk}.OBM`, truncating, with the four
    /// zero bytes that stand in for the held-back `OBCM` magic. The shard twin of
    /// [`map_upload_begin`](Self::map_upload_begin).
    ///
    /// There is no name-is-free guard here and there deliberately is one there: `MP{id}.OBM` names
    /// a map the device must never overwrite, whereas every `MS{id}S*` under an **open session's**
    /// id belongs to that session — [`set_upload_begin`] cleared the id before the first byte, so
    /// the only file this can truncate is one this same upload wrote (a re-sent shard, which §5.4's
    /// independent files make the cheapest possible recovery).
    fn set_shard_begin(&mut self, id: u16, index: usize) -> bool {
        self.upload_close();
        let Some(name) = obc_formats::obcs::shard_name(id, index) else { return false };
        match self.vmgr.open_file_in_dir(self.root, name.as_str(), Mode::ReadWriteCreateOrTruncate) {
            Ok(file) => {
                if self.vmgr.write(file, &[0u8; 4]).is_err() {
                    defmt::warn!("SD: cannot start shard {=usize} of MS{=u16} — the card refused", index, id);
                    let _ = self.vmgr.close_file(file);
                    return false;
                }
                self.open_upload = Some((file, UploadOwner::Set));
                defmt::info!("SD: shard {=usize} of MS{=u16} streaming (magic held back)", index, id);
                true
            }
            Err(e) => {
                defmt::warn!("SD: cannot create shard {=usize} of MS{=u16}: {}", index, id, defmt::Debug2Format(&e));
                false
            }
        }
    }

    /// Commit one streamed shard: patch the held-back `OBCM` magic into `MS{id}S{kk}.OBM` after the
    /// 40-byte header validates. Returns the stored length, or `None` with **that shard deleted**.
    ///
    /// A failed shard does not tear the set down: shards are independent files, so the host may
    /// simply re-send this one. What it must not leave is a zero-magic decoy the manifest commit
    /// would then have to reason about — the set's own token is the only in-flight marker.
    fn set_shard_commit(&mut self, id: u16, index: usize, magic: [u8; 4]) -> Option<u32> {
        self.upload_close();
        let derived = obc_formats::obcs::shard_name(id, index)?;
        let name = ShortFileName::create_from_str(derived.as_str()).ok()?;
        let file = self.vmgr.open_file_in_dir(self.root, &name, Mode::ReadWriteAppend).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0);
        let mut header = [0u8; obc_formats::obcm::HEADER_LEN];
        let read = self.vmgr.file_seek_from_start(file, 0).is_ok()
            && matches!(self.vmgr.read(file, &mut header), Ok(n) if n == header.len());
        header[0..4].copy_from_slice(&magic);
        if !read || obc_formats::obcm::validate_header_prefix(&header).is_err() {
            let _ = self.vmgr.close_file(file);
            let _ = self.vmgr.delete_file_in_dir(self.root, &name);
            defmt::warn!("SD: shard {=usize} of MS{=u16} is not a readable OBCM — rejected, file deleted", index, id);
            return None;
        }
        let patched = self.vmgr.file_seek_from_start(file, 0).is_ok()
            && self.vmgr.write(file, &magic).is_ok()
            && self.vmgr.flush_file(file).is_ok();
        let _ = self.vmgr.close_file(file);
        if !patched {
            defmt::warn!("SD: shard {=usize} of MS{=u16} magic patch failed — left inert", index, id);
            return None;
        }
        defmt::info!("SD: shard {=usize} of MS{=u16} committed ({=u32} B)", index, id, len);
        Some(len)
    }

    /// Open the set's **terrain shard** `MS{id}.OBD` for streaming (#1044) — the raster twin of
    /// [`set_shard_begin`](Self::set_shard_begin), with the four zero bytes that stand in for the
    /// held-back `OBCT` magic.
    ///
    /// The name is derived, not indexed: there is at most one terrain shard per set, and
    /// `MS{id}.OBD` is exactly the `OBCT_Spec.md` §4.6 sidecar of `MS{id}.OBS`, which is what lets
    /// the read side resolve it by the sidecar convention without consulting the manifest at all.
    fn set_terrain_begin(&mut self, id: u16) -> bool {
        self.upload_close();
        let Some(name) = obc_formats::obcs::terrain_name(id) else { return false };
        match self.vmgr.open_file_in_dir(self.root, name.as_str(), Mode::ReadWriteCreateOrTruncate) {
            Ok(file) => {
                if self.vmgr.write(file, &[0u8; 4]).is_err() {
                    defmt::warn!("SD: cannot start the MS{=u16} terrain shard — the card refused", id);
                    let _ = self.vmgr.close_file(file);
                    return false;
                }
                self.open_upload = Some((file, UploadOwner::Set));
                defmt::info!("SD: terrain shard of MS{=u16} streaming (magic held back)", id);
                true
            }
            Err(e) => {
                defmt::warn!("SD: cannot create /MS{=u16}.OBD: {}", id, defmt::Debug2Format(&e));
                false
            }
        }
    }

    /// Commit the streamed terrain shard: patch the held-back `OBCT` magic into `MS{id}.OBD` after
    /// the header prefix validates. Returns the stored length, or `None` with **that file deleted**.
    ///
    /// Same shape as [`set_shard_commit`](Self::set_shard_commit) and same reasoning: a failed
    /// raster is one independent file the host may re-send, and what it must not leave behind is a
    /// zero-magic decoy the manifest commit would then have to reason about. What differs is the
    /// format it is checked against — an OBCT container, not an OBCM file — and that is the whole
    /// reason terrain needed its own object type rather than a shard index (#1044).
    fn set_terrain_commit(&mut self, id: u16, magic: [u8; 4]) -> Option<u32> {
        self.upload_close();
        let derived = obc_formats::obcs::terrain_name(id)?;
        let name = ShortFileName::create_from_str(derived.as_str()).ok()?;
        let file = self.vmgr.open_file_in_dir(self.root, &name, Mode::ReadWriteAppend).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0);
        let mut header = [0u8; obc_formats::obct::HEADER_LEN];
        let read = self.vmgr.file_seek_from_start(file, 0).is_ok()
            && matches!(self.vmgr.read(file, &mut header), Ok(n) if n == header.len());
        header[0..4].copy_from_slice(&magic);
        if !read || obc_formats::obct::validate_header_prefix(&header).is_err() {
            let _ = self.vmgr.close_file(file);
            let _ = self.vmgr.delete_file_in_dir(self.root, &name);
            defmt::warn!("SD: the MS{=u16} terrain shard is not a readable OBCT — rejected, file deleted", id);
            return None;
        }
        let patched = self.vmgr.file_seek_from_start(file, 0).is_ok()
            && self.vmgr.write(file, &magic).is_ok()
            && self.vmgr.flush_file(file).is_ok();
        let _ = self.vmgr.close_file(file);
        if !patched {
            defmt::warn!("SD: MS{=u16} terrain magic patch failed — left inert", id);
            return None;
        }
        defmt::info!("SD: terrain shard of MS{=u16} committed ({=u32} B)", id, len);
        Some(len)
    }

    /// Drop the in-flight terrain shard: close the streaming handle and delete just that
    /// `MS{id}.OBD`. The set's session survives, exactly as it does for a failed OBCM shard.
    fn set_terrain_discard(&mut self, id: u16) {
        self.upload_close();
        let Some(derived) = obc_formats::obcs::terrain_name(id) else { return };
        let Ok(name) = ShortFileName::create_from_str(derived.as_str()) else { return };
        match self.vmgr.delete_file_in_dir(self.root, &name) {
            Ok(()) => defmt::info!("SD: dropped the partial terrain shard of MS{=u16}", id),
            Err(e) => defmt::warn!(
                "SD: could not drop the partial terrain shard of MS{=u16} ({}) — a re-send truncates it",
                id,
                defmt::Debug2Format(&e)
            ),
        }
    }

    /// Re-open the set's `MS{id}.OBS` token for the manifest stream, truncating it back to the four
    /// zero bytes. The manifest is the **last** file of the set (§5.4), and it is written into the
    /// same name the token already occupies, so the set is never without its in-flight marker —
    /// not even for the width of one create.
    fn set_manifest_begin(&mut self, id: u16) -> bool {
        self.upload_close();
        let Some(name) = obc_formats::obcs::manifest_name(id) else { return false };
        match self.vmgr.open_file_in_dir(self.root, name.as_str(), Mode::ReadWriteCreateOrTruncate) {
            Ok(file) => {
                if self.vmgr.write(file, &[0u8; 4]).is_err() {
                    defmt::warn!("SD: cannot start the MS{=u16} manifest — the card refused", id);
                    let _ = self.vmgr.close_file(file);
                    return false;
                }
                self.open_upload = Some((file, UploadOwner::Set));
                true
            }
            Err(e) => {
                defmt::warn!("SD: cannot reopen /MS{=u16}.OBS: {}", id, defmt::Debug2Format(&e));
                false
            }
        }
    }

    /// **The set's commit point.** Read the streamed manifest back with the held-back `OBCS` magic
    /// spliced in, validate it against `OBCA_Spec.md` §5.3 *and* against the shards actually on the
    /// card, and only then write the magic — the one write that turns `1..=32` files plus a
    /// placeholder into a map.
    ///
    /// The re-read is what makes "the manifest is written last" a *checked* property rather than a
    /// hoped-for one. A manifest is at most 1,864 B, so validating it costs a stack buffer and one
    /// pass over the shard headers — the same pass the boot scan runs — against a transfer measured
    /// in gigabytes. Returns the set's total bytes, or `None` with the **whole set deleted**: a
    /// manifest that does not describe the files beside it is not a map, and leaving the shards
    /// would leave gigabytes no surface can explain.
    #[inline(never)]
    fn set_manifest_commit(&mut self, id: u16, magic: [u8; 4]) -> Option<u64> {
        self.upload_close();
        let outcome = self.validate_committed_manifest(id, magic);
        let Some(total) = outcome else {
            defmt::warn!("SD: the MS{=u16} manifest does not describe the shards on the card — set discarded", id);
            self.delete_set(id);
            return None;
        };
        let derived = obc_formats::obcs::manifest_name(id)?;
        let name = ShortFileName::create_from_str(derived.as_str()).ok()?;
        let file = self.vmgr.open_file_in_dir(self.root, &name, Mode::ReadWriteAppend).ok()?;
        let patched = self.vmgr.file_seek_from_start(file, 0).is_ok()
            && self.vmgr.write(file, &magic).is_ok()
            && self.vmgr.flush_file(file).is_ok();
        let _ = self.vmgr.close_file(file);
        if !patched {
            defmt::warn!("SD: MS{=u16} manifest magic patch failed — the set stays inert for the boot sweep", id);
            return None;
        }
        defmt::info!("SD: volume set MS{=u16} committed ({=u64} B of shards)", id, total);
        Some(total)
    }

    /// Parse the streamed `MS{id}.OBS` with `magic` spliced over its placeholder and check it
    /// against the card, returning the set's total bytes. Split out of
    /// [`set_manifest_commit`](Self::set_manifest_commit) so the 1,864 B manifest buffer leaves the
    /// frame before the commit write (the ~36 KB stack rule).
    ///
    /// Everything [`set_file_totals`](Self::set_file_totals) checks, **plus** the one check that is
    /// the commit's alone: the `terrain` record against the raster actually on the card (#1044).
    /// The boot scan must not make that judgement — §5.3 makes an unreadable raster a mount-time
    /// non-failure and the scan's `None` means *this is not a map* — but here the host built both
    /// files seconds ago and was told the exact manifest length it had to announce, so a
    /// disagreement is the two ends contradicting each other about this very transfer. The rule and
    /// the reason it differs by call site live in `obc_app::terrain_record_agrees`, where they are
    /// tested.
    #[inline(never)]
    fn validate_committed_manifest(&self, id: u16, magic: [u8; 4]) -> Option<u64> {
        let derived = obc_formats::obcs::manifest_name(id)?;
        let name = ShortFileName::create_from_str(derived.as_str()).ok()?;
        let mut buf = [0u8; obc_formats::obcs::MAX_MANIFEST_LEN];
        let read = self.read_root_file(&name, &mut buf)?;
        let bytes = buf.get_mut(..read)?;
        bytes.get_mut(..4)?.copy_from_slice(&magic);
        let parsed = obc_formats::obcs::parse(bytes).ok()?;
        if parsed.encoded_len() != read {
            return None;
        }
        let recorded = parsed.terrain().map(|terrain| terrain.bytes);
        let on_card = derived_short_name(obc_formats::obcs::terrain_name(id))
            .and_then(|terrain_name| self.terrain_shard_len(&terrain_name));
        if !obc_app::terrain_record_agrees(recorded, on_card) {
            defmt::warn!(
                "SD: the MS{=u16} manifest's terrain record does not describe the raster beside it — set discarded",
                id
            );
            return None;
        }
        self.set_file_totals(&parsed, id)
    }

    /// Read a whole card-root file into `buf`, returning how many bytes landed, or `None` when it
    /// cannot be opened or is longer than the buffer.
    fn read_root_file(&self, name: &ShortFileName, buf: &mut [u8]) -> Option<usize> {
        let file = self.vmgr.open_file_in_dir(self.root, name, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0) as usize;
        let read = if len >= obc_formats::obcs::HEADER_LEN && len <= buf.len() {
            let mut done = 0usize;
            while done < len {
                match self.vmgr.read(file, &mut buf[done..len]) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => done += n,
                }
            }
            done
        } else {
            0
        };
        let _ = self.vmgr.close_file(file);
        (read == len && read > 0).then_some(read)
    }

    /// Drop one in-flight shard: close the streaming handle and delete just that
    /// `MS{id}S{kk}.OBM`. The set's session survives — a shard that failed validation is one file the
    /// host can re-send, and the gigabytes already committed beside it are still good.
    fn set_shard_discard(&mut self, id: u16, index: usize) {
        self.upload_close();
        let Some(derived) = obc_formats::obcs::shard_name(id, index) else { return };
        let Ok(name) = ShortFileName::create_from_str(derived.as_str()) else { return };
        match self.vmgr.delete_file_in_dir(self.root, &name) {
            Ok(()) => defmt::info!("SD: dropped the partial shard {=usize} of MS{=u16}", index, id),
            Err(e) => defmt::warn!(
                "SD: could not drop the partial shard {=usize} of MS{=u16} ({}) — a re-send truncates it",
                index,
                id,
                defmt::Debug2Format(&e)
            ),
        }
    }

    /// Abandon an in-flight set upload: close the streaming handle and remove the whole set —
    /// token first, then every shard name to the cap ([`delete_set`](Self::delete_set)).
    ///
    /// The set is already invisible (its manifest has no magic), so the boot sweep would reclaim it
    /// eventually. Waiting is as wrong here as it is for a single map, and worse by two orders of
    /// magnitude: a set is *gigabytes*, and a retry mints a fresh id only if this one is still
    /// occupied, so leaving the corpse would strand the card's whole free space in a few attempts.
    fn set_upload_abort(&mut self, id: u16) {
        self.upload_close();
        let removed = self.delete_set(id);
        defmt::info!("SD: abandoned volume set MS{=u16} reclaimed ({=usize} files)", id, removed);
    }

    /// Sweep abandoned **volume-set** uploads from the card root (issue #1039) — the set twin of
    /// [`sweep_aborted_maps`](Self::sweep_aborted_maps), run beside it at boot. Returns how many
    /// files were reclaimed.
    ///
    /// Two passes, both keyed on the same held-back-magic proof the map sweep uses, because a set
    /// leaves two distinguishable kinds of debris:
    ///
    /// 1. **An uncommitted `MS{id}.OBS`** — the in-flight token [`set_upload_begin`](Self::set_upload_begin)
    ///    writes before the first shard: four zero bytes, a torn magic patch, or (its own create
    ///    without its write) nothing at all. It says "this device was writing set `id` and did not
    ///    finish", so the whole set goes through `delete_plan`, shards included, magic-intact or
    ///    not. This is the case that reclaims the gigabytes.
    /// 2. **An uncommitted orphan shard** — an `MS{id}S{kk}.OBM` with no `MS{id}.OBS` beside it at
    ///    all, whose own magic never landed. The residue of a torn transfer whose token delete
    ///    landed and whose shard delete did not.
    ///
    /// **The terrain shard needs no third pass**, and that is a decision rather than an omission
    /// (#1044). Pass 1 reclaims it with the rest of the set — `delete_plan` names `MS{id}.OBD` —
    /// which covers every way this device abandons an upload, because a raster is only ever
    /// accepted while a set session is open and that session's token is on the card. What is left
    /// is an `MS{id}.OBD` whose manifest *and* every shard have already been reclaimed, and
    /// probing for that would mean claiming a bare `.OBD` — which is exactly what a rider's own
    /// card-reader copy looks like mid-copy, and what the orphan rule below refuses to touch for
    /// precisely that reason. It costs one file's bytes until the id is reused, and
    /// [`set_upload_begin`](Self::set_upload_begin) clears the whole id before it writes.
    ///
    /// What it will **not** touch is the case §5.4 leaves to a MAY: a *complete* orphan shard with
    /// no manifest. That is precisely the shape a rider copying a set over a card reader leaves
    /// mid-copy, and deleting it would destroy a map that was minutes from working, unrecoverably.
    /// The rule lives in `obc_app::orphan_shard_verdict` where it is tested; the cost of erring
    /// this way is some dead bytes the next upload's supersede pass reclaims.
    ///
    /// **The scan keeps ids, not entries.** A bounded list of directory entries is the wrong
    /// structure here and was a real bug: it filled with the *valid* sets' names, dropped whatever
    /// came after — the same entries every boot, since the directory order is stable — and the torn
    /// set behind them was never examined at all. Ids fit a bitmap over the whole namespace (§5.2
    /// caps it at 1,000, so 125 bytes covers every set that can exist), and every name is derivable
    /// from an id, so nothing has to be remembered to be reclaimed.
    #[inline(never)]
    fn sweep_aborted_sets(&mut self) -> usize {
        let mut manifests = SetIdBits::new();
        let mut shard_ids = SetIdBits::new();
        self.iter_dir_lfn(self.root, |e, long| {
            if is_set_manifest_entry(e, long) {
                if let Some(id) = set_manifest_id(&e.name) {
                    manifests.set(id);
                }
            } else if let Some((id, _)) = obc_formats::obcs::parse_shard_name(&short_name_bytes(&e.name)) {
                shard_ids.set(id);
            }
        });

        let mut swept = 0usize;
        for id in manifests.iter() {
            let Some(name) = derived_short_name(obc_formats::obcs::manifest_name(id)) else { continue };
            if obc_app::sweep_verdict(self.root_magic(&name)) == obc_app::SweepVerdict::Reclaim {
                defmt::info!("SD: sweeping the abandoned volume set MS{=u16} (its manifest never committed)", id);
                swept += self.delete_set(id);
            }
        }
        // Orphans: an id carrying shards and no manifest at all. Their names are derived from the
        // id (§5.2), so the 32 possible ones are probed rather than remembered — the same thing
        // `delete_set` does, and the reason neither needs a list that can overflow. Only ids the
        // scan actually saw a shard for are probed, and a card in that state was hand-made.
        for id in shard_ids.iter() {
            if manifests.has(id) {
                continue; // not an orphan: its set's manifest is (or was) there
            }
            for index in 0..obc_formats::obcs::MAX_SHARDS {
                let Some(name) = derived_short_name(obc_formats::obcs::shard_name(id, index)) else { continue };
                if obc_app::orphan_shard_verdict(self.root_magic(&name)) != obc_app::SweepVerdict::Reclaim {
                    continue;
                }
                if self.vmgr.delete_file_in_dir(self.root, &name).is_ok() {
                    defmt::info!("SD: swept the orphan shard {=usize} of MS{=u16}", index, id);
                    swept += 1;
                }
            }
        }
        swept
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

    /// Whether a stored ride file is an **interrupted save** — the held-back version byte still
    /// zeroed because [`track_to_ride`]'s final patch never ran. Only that exact signature is
    /// sweepable (the ride-scan's analogue of [`is_aborted_commit`](Self::is_aborted_commit));
    /// a merely unreadable file must be kept.
    fn is_aborted_ride_object(&self, name: &ShortFileName) -> bool {
        let Some(dir) = self.tracks_dir else { return false };
        let Ok(file) = self.vmgr.open_file_in_dir(dir, name, Mode::ReadOnly) else {
            return false;
        };
        let mut version = [0xFFu8; 1];
        let zeroed = matches!(self.vmgr.read(file, &mut version), Ok(1)) && version[0] == 0;
        let _ = self.vmgr.close_file(file);
        zeroed
    }

    /// Open a stored route for a detail download (the OBCR bytes verbatim), returning its byte
    /// length. Held in a slot separate from the ride's `open_route`.
    pub fn open_object(&mut self, owner: GateOwner, name: &ShortFileName) -> Option<(u32, DownloadSession)> {
        self.open_object_in(owner, self.routes_dir, name)
    }

    /// Open a stored ride object for a download (the stored bytes *are* the wire object) — the
    /// `/tracks` twin of [`open_object`](Self::open_object), sharing the same handle slot (one
    /// transfer at a time).
    pub fn open_ride_object(&mut self, owner: GateOwner, name: &ShortFileName) -> Option<(u32, DownloadSession)> {
        self.open_object_in(owner, self.tracks_dir, name)
    }

    fn open_object_in(
        &mut self,
        owner: GateOwner,
        dir: Option<RawDirectory>,
        name: &ShortFileName,
    ) -> Option<(u32, DownloadSession)> {
        if let Some(open) = &self.open_object {
            if open.owner != owner {
                return None;
            }
            self.close_object_owner(owner);
        }
        let file = self.vmgr.open_file_in_dir(dir?, name, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0);
        self.open_object = Some(OpenObject { name: name.clone(), owner, file, len });
        Some((len, SinkSession::new(file)))
    }

    /// A [`ByteSource`](obc_formats::io::ByteSource) over the open object — the CRC pre-pass and the
    /// chunked sends both read through it.
    pub fn object_source(&self, session: DownloadSession) -> Option<Source<'_>> {
        let open = self.open_object.as_ref()?;
        session.matches_key(open.file).then(|| SdByteSource::new(&self.vmgr, open.file, open.len))
    }

    /// Whole-object CRC over the retained detail handle.
    pub(crate) fn object_crc(&self, session: DownloadSession, len: u32) -> Option<u32> {
        let src = self.object_source(session)?;
        Self::source_crc(&src, len)
    }

    #[inline(never)]
    fn source_crc(src: &Source<'_>, len: u32) -> Option<u32> {
        let mut crc = Crc32::new();
        let mut buf = [0u8; 512];
        let mut offset = 0u32;
        while offset < len {
            let n = ((len - offset) as usize).min(buf.len());
            ByteSource::read_at(src, offset, &mut buf[..n]).ok()?;
            crc.update(&buf[..n]);
            offset += n as u32;
        }
        Some(crc.finalize())
    }

    /// Open, checksum, and always close one `/routes` object.
    pub(crate) fn file_crc(&self, name: &ShortFileName) -> Option<u32> {
        self.with_routes_object(name, Self::source_crc)
    }

    /// Close exactly this detail-download handle; stale tokens are no-ops.
    pub fn close_object(&mut self, session: DownloadSession) {
        if self.open_object.as_ref().is_some_and(|open| session.matches_key(open.file)) {
            self.close_open_object();
        }
    }

    /// Close only the retained detail owned by this wire.
    pub fn close_object_owner(&mut self, owner: GateOwner) {
        if self.open_object.as_ref().is_some_and(|open| open.owner == owner) {
            self.close_open_object();
        }
    }

    #[inline(never)]
    fn close_open_object(&mut self) {
        if let Some(open) = self.open_object.take() {
            let _ = self.vmgr.close_file(open.file);
        }
    }
}

// ==================== WEATHER.A / WEATHER.B (WX7, #1192) ====================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherIoError {
    Busy,
    Open,
    Write,
    Flush,
    Close,
}

/// Policy-only facts from the validated active OBCW header. Kept beside the board's session-open
/// reader rather than in `obc_weather::Candidate`: slot selection crosses storage error paths where
/// enlarging every candidate would materially increase the error enum and callers' stack frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeatherPolicyFacts {
    pub south_lat_udeg: i32,
    pub west_lon_udeg: i32,
    pub north_lat_udeg: i32,
    pub east_lon_udeg: i32,
    pub frame_count: u16,
}

impl Storage {
    /// Revalidate both fixed roots and retain the deterministic active generation. This runs once
    /// at mount; later weather upload composition refreshes it after a successful commit.
    pub fn refresh_weather_selection(&mut self) -> SlotSelection {
        // embedded-sdmmc refuses a second open of the same file. Drop the old selected slot before
        // the validation pass opens both roots, then reopen exactly the winning one for streaming.
        if let Some((file, _)) = self.open_weather.take() {
            let _ = self.vmgr.close_file(file);
        }
        self.weather_mount = None;
        self.weather_active = None;
        self.weather_policy = None;
        let mut selection = weather_store::inspect_slots(self);
        self.weather_active = selection.active;
        if let Some(active) = selection.active {
            let name = weather_store::slot_file_name(active.slot);
            match self.vmgr.open_file_in_dir(self.root, name, Mode::ReadOnly) {
                Ok(file) => {
                    let len = self.vmgr.file_length(file).unwrap_or(0);
                    let source = SdByteSource::new(&self.vmgr, file, len);
                    match obc_weather::WeatherReader::open(&source) {
                        Ok(reader) => {
                            let header = reader.header();
                            let reopened = WeatherCandidate {
                                slot: active.slot,
                                generation: header.generation,
                                generated_at: header.generated_at,
                                total_len: header.total_len,
                                bundle_crc32: header.crc32,
                            };
                            // The root may have changed between the two-slot inspection and this
                            // session-open. Never retain selection metadata from one object beside
                            // a validation token/source for another.
                            if reopened == active {
                                self.weather_policy = Some(WeatherPolicyFacts {
                                    south_lat_udeg: header.south_lat_udeg,
                                    west_lon_udeg: header.west_lon_udeg,
                                    north_lat_udeg: header.north_lat_udeg,
                                    east_lon_udeg: header.east_lon_udeg,
                                    frame_count: header.frame_count,
                                });
                                self.weather_mount = Some(reader.validated());
                                self.open_weather = Some((file, len));
                            } else {
                                let _ = self.vmgr.close_file(file);
                                self.weather_active = None;
                                self.weather_policy = None;
                                selection.active = None;
                            }
                        }
                        Err(_) => {
                            let _ = self.vmgr.close_file(file);
                            self.weather_active = None;
                            self.weather_policy = None;
                            selection.active = None;
                        }
                    }
                }
                Err(_) => {
                    // The file was readable during validation and disappeared before the reopen.
                    // Fail closed: metadata without a readable source is not an active bundle.
                    self.weather_active = None;
                    self.weather_policy = None;
                    selection.active = None;
                }
            }
        }
        selection
    }

    pub const fn weather_active(&self) -> Option<WeatherCandidate> {
        self.weather_active
    }

    pub const fn weather_policy(&self) -> Option<WeatherPolicyFacts> {
        self.weather_policy
    }

    /// A cheap [`ByteSource`] view over the session-open active weather bundle.
    pub fn weather_source(&self) -> Option<Source<'_>> {
        self.open_weather.map(|(file, len)| SdByteSource::new(&self.vmgr, file, len))
    }

    /// The validation proof paired with [`weather_source`](Storage::weather_source).
    pub const fn weather_mount(&self) -> Option<obc_weather::ValidatedBundle> {
        self.weather_mount
    }

    /// Run after `Storage` has moved into its final `.bss` home. Keeping this out of
    /// [`Storage::mount`] is load-bearing: naming a local `Storage` there materialized a second
    /// ~13.5 KiB construction/copy slot in `main`'s permanent async poll frame (#677/#1108).
    #[inline(never)]
    pub fn select_weather_at_boot(&mut self) {
        self.refresh_weather_selection();
        match self.weather_active() {
            Some(active) => defmt::info!(
                "SD: weather slot {=str} generation {=u32} selected ({=u32} B)",
                match active.slot {
                    WeatherSlot::A => "A",
                    WeatherSlot::B => "B",
                },
                active.generation,
                active.total_len
            ),
            None => defmt::info!("SD: no valid WEATHER.A/WEATHER.B generation"),
        }
    }
}

impl WeatherSlotIo for Storage {
    type Error = WeatherIoError;

    fn inspect_slot(&mut self, slot: WeatherSlot, magic: Option<[u8; 4]>) -> SlotValidation {
        // The active root is held open for the render/sampling lifetime, and embedded-sdmmc
        // refuses a second open. Ordinary A/B selection may reuse its cached fully-validated
        // identity; a caller asking for alternate held-magic validation reads through that same
        // handle instead of reopening it.
        if self.weather_active.is_some_and(|active| active.slot == slot) {
            if magic.is_none() {
                return SlotValidation::Valid(self.weather_active.expect("checked Some above"));
            }
            if let Some(source) = self.weather_source() {
                return obc_weather::validate_slot_with_magic(slot, &source, magic.expect("checked Some above"));
            }
            return SlotValidation::Unreadable;
        }
        let Ok(name) = ShortFileName::create_from_str(weather_store::slot_file_name(slot)) else {
            return SlotValidation::Unreadable;
        };
        let file = match self.vmgr.open_file_in_dir(self.root, &name, Mode::ReadOnly) {
            Ok(file) => file,
            Err(embedded_sdmmc::Error::NotFound) => return SlotValidation::Missing,
            Err(_) => return SlotValidation::Unreadable,
        };
        let len = match self.vmgr.file_length(file) {
            Ok(len) => len,
            _ => {
                let _ = self.vmgr.close_file(file);
                return SlotValidation::Unreadable;
            }
        };
        let validation = {
            let source = SdByteSource::new(&self.vmgr, file, len);
            match magic {
                Some(magic) => obc_weather::validate_slot_with_magic(slot, &source, magic),
                None => obc_weather::validate_slot(slot, &source),
            }
        };
        if self.vmgr.close_file(file).is_err() {
            return SlotValidation::Unreadable;
        }
        validation
    }

    fn begin_slot(&mut self, slot: WeatherSlot) -> Result<(), Self::Error> {
        if self.open_upload.is_some() || upload_pipe_enabled() {
            return Err(WeatherIoError::Busy);
        }
        let file = self
            .vmgr
            .open_file_in_dir(self.root, weather_store::slot_file_name(slot), Mode::ReadWriteCreateOrTruncate)
            .map_err(|_| WeatherIoError::Open)?;
        self.open_upload = Some((file, UploadOwner::Weather(slot)));
        Ok(())
    }

    fn append_slot(&mut self, slot: WeatherSlot, bytes: &[u8]) -> Result<(), Self::Error> {
        let Some((file, UploadOwner::Weather(owner))) = self.open_upload else {
            return Err(WeatherIoError::Busy);
        };
        if owner != slot {
            return Err(WeatherIoError::Busy);
        }
        self.vmgr.write(file, bytes).map_err(|_| WeatherIoError::Write)
    }

    fn close_slot(&mut self, slot: WeatherSlot) -> Result<(), Self::Error> {
        let Some((file, owner)) = self.open_upload.take() else {
            return Err(WeatherIoError::Busy);
        };
        if owner != UploadOwner::Weather(slot) {
            self.open_upload = Some((file, owner));
            return Err(WeatherIoError::Busy);
        }
        let flushed = self.vmgr.flush_file(file).is_ok();
        let closed = self.vmgr.close_file(file).is_ok();
        if !flushed {
            Err(WeatherIoError::Flush)
        } else if !closed {
            Err(WeatherIoError::Close)
        } else {
            Ok(())
        }
    }

    fn abandon_slot(&mut self, slot: WeatherSlot) {
        let Some((file, owner)) = self.open_upload.take() else { return };
        if owner == UploadOwner::Weather(slot) {
            let _ = self.vmgr.close_file(file);
        } else {
            self.open_upload = Some((file, owner));
        }
    }

    fn commit_magic(&mut self, slot: WeatherSlot, magic: [u8; 4]) -> Result<(), Self::Error> {
        if self.open_upload.is_some() {
            return Err(WeatherIoError::Busy);
        }
        let name =
            ShortFileName::create_from_str(weather_store::slot_file_name(slot)).map_err(|_| WeatherIoError::Open)?;
        let file =
            self.vmgr.open_file_in_dir(self.root, &name, Mode::ReadWriteAppend).map_err(|_| WeatherIoError::Open)?;
        let wrote = self.vmgr.file_seek_from_start(file, 0).is_ok() && self.vmgr.write(file, &magic).is_ok();
        let flushed = wrote && self.vmgr.flush_file(file).is_ok();
        let closed = self.vmgr.close_file(file).is_ok();
        if !wrote {
            Err(WeatherIoError::Write)
        } else if !flushed {
            // Magic may or may not have reached the card. Both recovery branches are tested by the
            // pure store: old active stays valid; boot either ignores zero magic or selects new.
            Err(WeatherIoError::Flush)
        } else if !closed {
            Err(WeatherIoError::Close)
        } else {
            Ok(())
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
    !e.attributes.is_directory() && route_name::is_admitted(e.name.extension(), long)
}

/// The **durable ride object id** in a stored ride's filename — `RD{id}.ORD` → `id`. The same
/// durability contract as the routes': the app's synced-set and tombstones key on these ids across
/// device reboots.
pub fn stored_ride_id(name: &ShortFileName) -> Option<u16> {
    id_in_name(name, b"RD", b"ORD")
}

/// Bind a FAT entry to the host-tested trip filename classifier. The ride log's `TRACK.OBT` lives
/// in `/tracks`, so it never reaches this `/routes` binding.
fn is_trip_entry(e: &embedded_sdmmc::DirEntry, long: Option<&str>) -> bool {
    if e.attributes.is_directory() {
        return false;
    }
    trip_name::is_admitted(e.name.extension(), long)
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
/// **A volume set's shards are not maps.** `MS{id}S{kk}.OBM` shares the `.OBM` extension with a
/// received single map — deliberately, so the transfer path needs no new file type — and each
/// shard *is* a valid OBCM file, which is exactly why the exclusion has to be in the *name* test
/// rather than in the header read. `OBCA_Spec.md` §5.4: a reader "MUST NOT mount a shard
/// individually as a standalone map", because a geometry shard is a map with no roads and no POIs
/// and the core is a map that draws nothing at all. The manifest ([`is_set_manifest_entry`]) is the
/// only thing that says those files are one map, and it is what the catalog lists.
///
/// The rule itself is `obc_app::classify_map_entry` — pure, and therefore tested where tests run;
/// this is the binding from a FAT directory entry to its three inputs. The board crate has no CI
/// test harness (bare metal), so nothing decidable may be decided here.
fn is_map_entry(e: &embedded_sdmmc::DirEntry, long: Option<&str>) -> bool {
    classify_entry(e, long) == obc_app::MapEntry::Map
}

/// Whether a card-root entry is a volume-set **manifest** (`MS{id}.OBS`, `OBCA_Spec.md` §5.2).
/// The classification is `obc_app::classify_map_entry`'s — see [`is_map_entry`].
fn is_set_manifest_entry(e: &embedded_sdmmc::DirEntry, long: Option<&str>) -> bool {
    classify_entry(e, long) == obc_app::MapEntry::SetManifest
}

/// Bind one FAT directory entry to the pure classifier.
fn classify_entry(e: &embedded_sdmmc::DirEntry, long: Option<&str>) -> obc_app::MapEntry {
    obc_app::classify_map_entry(&short_name_bytes(&e.name), long, e.attributes.is_directory())
}

/// A derived `obcs` filename as a [`ShortFileName`], or `None` if it has neither (an id past the
/// namespace, or a name FAT would refuse). The one-line bridge every set path needs, since §5.2
/// derives all of them from `(id, index)` rather than storing them.
fn derived_short_name(derived: Option<obc_formats::obcs::FileName>) -> Option<ShortFileName> {
    ShortFileName::create_from_str(derived?.as_str()).ok()
}

/// One bit per volume-set id — the whole `0..=MAX_SET_ID` namespace in 125 bytes (issue #1039).
///
/// The card-root scan needs to remember *which set ids it saw* while it holds the directory
/// iterator's borrow, and cannot open a file to decide anything until the iteration is over. A
/// bounded list of entries was the first answer and the wrong one: it fills with whatever the
/// directory yields first — stably, so the same entries every boot — and silently drops the rest.
/// A bitmap over an id space this small cannot drop anything, and every filename is derivable from
/// its id, so nothing else needs remembering.
struct SetIdBits([u8; (obc_formats::obcs::MAX_SET_ID as usize + 8) / 8]);

impl SetIdBits {
    const fn new() -> SetIdBits {
        SetIdBits([0; (obc_formats::obcs::MAX_SET_ID as usize + 8) / 8])
    }

    fn set(&mut self, id: u16) {
        if let Some(byte) = self.0.get_mut(id as usize / 8) {
            *byte |= 1 << (id % 8);
        }
    }

    fn has(&self, id: u16) -> bool {
        self.0.get(id as usize / 8).is_some_and(|byte| byte & (1 << (id % 8)) != 0)
    }

    /// The ids that are set, ascending.
    fn iter(&self) -> impl Iterator<Item = u16> + '_ {
        (0..=obc_formats::obcs::MAX_SET_ID).filter(|&id| self.has(id))
    }
}

/// A short name as the `BASE.EXT` bytes `obc_formats::obcs`' filename parsers take. Both halves
/// come back space-trimmed from embedded-sdmmc, so this is a straight join.
fn short_name_bytes(name: &ShortFileName) -> heapless::Vec<u8, 12> {
    let mut out: heapless::Vec<u8, 12> = heapless::Vec::new();
    let _ = out.extend_from_slice(name.base_name());
    let _ = out.push(b'.');
    let _ = out.extend_from_slice(name.extension());
    out
}

/// The set id in a `MS{id}.OBS` manifest name, or `None` for anything else. The strict parse
/// (uppercase, no leading zeros, id ≤ 999) lives in the format authority.
pub fn set_manifest_id(name: &ShortFileName) -> Option<u16> {
    obc_formats::obcs::parse_manifest_name(&short_name_bytes(name))
}

/// The terrain sidecar's 8.3 name for a map file: the map's base name with [`TERRAIN_EXT`]
/// (`GRIMSEL.OBM` → `GRIMSEL.OBD`, `MS7.OBS` → `MS7.OBD`). Derived, never stored — the same rule
/// as a set's shard names, and the reason a rider can rename a map without breaking its terrain.
#[cfg(has_nav)]
fn sidecar_name(map: &ShortFileName) -> Option<ShortFileName> {
    let mut text: String<16> = String::new();
    for &b in map.base_name().iter().take_while(|&&b| b != b' ') {
        text.push(b as char).ok()?;
    }
    text.push('.').ok()?;
    text.push_str(TERRAIN_EXT).ok()?;
    ShortFileName::create_from_str(text.as_str()).ok()
}

/// The 8.3 name of shard `index` of set `id`. Filenames are **derived, not stored** (§5.2): a
/// stored name is a second source of truth that can disagree with the directory.
fn set_shard_name_for(id: u16, index: usize) -> Option<ShortFileName> {
    ShortFileName::create_from_str(obc_formats::obcs::shard_name(id, index)?.as_str()).ok()
}

/// The **durable map object id** in a received map's filename — `MP{id}.OBM` → `id`, the same
/// filenames-guard-stored-ids rule (spec §4.1) as routes/rides/trips. `None` for a side-loaded
/// `.obcm`, which carries no id at all.
pub fn uploaded_map_id(name: &ShortFileName) -> Option<u16> {
    id_in_name(name, b"MP", b"OBM")
}

/// The 8.3 filename a map with object id `id` is stored under — the exact inverse of
/// [`uploaded_map_id`]. `None` only if the id somehow doesn't fit an 8.3 name, which `u16` can't.
pub fn map_file_name_for(id: u16) -> Option<ShortFileName> {
    let mut name: String<12> = String::new();
    core::fmt::Write::write_fmt(&mut name, format_args!("MP{id}.OBM")).ok()?;
    ShortFileName::create_from_str(name.as_str()).ok()
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

fn set_identity_from_manifest(parsed: &obc_formats::obcs::SetManifest) -> SetIdentity {
    let mut name: String<24> = String::new();
    for ch in parsed.name().unwrap_or("").chars() {
        let _ = name.push(ch);
    }
    SetIdentity {
        shard_count: parsed.shard_count() as u8,
        obcm_version: parsed.obcm_version,
        bbox: BBox {
            min_lat: parsed.bbox.min_lat,
            min_lon: parsed.bbox.min_lon,
            max_lat: parsed.bbox.max_lat,
            max_lon: parsed.bbox.max_lon,
        },
        total_bytes: parsed.total_bytes(),
        name,
    }
}

/// The scanned catalog as the host-tested classifiers want it — one [`obc_app::MapChoice`] per map,
/// in scan order, so an index into this is an index into `maps`.
///
/// A set is readable when its OBCM version matches and its shard count fits this board's retained
/// handle/store ceiling. The scan has already validated manifest presence, sizes, headers and
/// bboxes; the boot mount adds ladder/style/extent checks before rendering a pixel.
fn map_choices(maps: &[MapSummary]) -> Vec<obc_app::MapChoice, MAX_MAPS> {
    let mut choices: Vec<obc_app::MapChoice, MAX_MAPS> = Vec::new();
    for m in maps.iter().take(MAX_MAPS) {
        let _ = choices.push(obc_app::MapChoice {
            selected: m.selected,
            uploaded_id: m.id,
            readable: m.obcm_version == obc_formats::obcm::VERSION
                && m.shards.is_none_or(|count| count as usize <= SD_SET_MAX_SHARDS),
            set: m.shards.is_some(),
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
