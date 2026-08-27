//! OBC desktop simulator — host shell around the shared renderer.
//!
//! All map drawing lives in `obc_render`, the same code the nRF54L firmware runs
//! against the LS021B7DD02. This binary owns only the host concerns: argument
//! parsing, the eframe window + pan/zoom event loop, PNG output, and the device's
//! 64-color display policy.
//!
//! Host logic shared with the landing page's wasm host (`obc-web-demo`) — replay stepping, the
//! frame-interleaved `NavPlan`, the in-memory byte sink — lives in `obc-host-core`, not here.

use std::time::Instant;

use embedded_graphics::pixelcolor::Rgb888;
use obc_app::{App, AppState};
use obc_ports::{Button, ButtonEvent, Fix, InputClock, InputEvent, InputSource, LocationSource};
use obc_reader::{rgb565_to_device64, Reader};

mod calib;
mod device_input;
mod dfu;
mod framebuffer;
mod gui;
mod map_file;
mod palette;
mod panel_power;
mod present;
mod rides;
mod routes;
mod settings_store;
mod sim_compass;
mod sim_location;
mod sim_sensors;
mod track;
mod trips;
mod weather_companion;
mod weather_live;
mod weather_store;
use framebuffer::Framebuffer;
use obc_host_core::{
    initial_camera, replay_advance, ActiveRouteSession, HostLoop, HostPlatform, PlanHold, ReplaySensors,
};
use obc_replay::{gpx::Track, BaroSensor, GpxPlayer};
use obc_route::RouteReader;
use rides::RideStore;
use routes::RouteStore;
use track::TrackStore;
use trips::TripStore;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct BleSeed {
    connected: bool,
    paired: bool,
    passkey: Option<u32>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SensorSeed {
    Demo,
    Screen,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hold {
    Nav,
    Detour,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NavFailure {
    Exhausted,
    NoPath,
}

#[derive(Clone, Copy)]
enum Injection {
    NavFail(NavFailure),
    DetourFail(NavFailure),
    Upload { id: obc_app::CatalogObjectId, replaced: bool },
    TripUpload { id: obc_app::CatalogObjectId },
    MapTransfer(obc_app::screen::MapTransfer),
    Warning(obc_app::WarningFlags),
}

#[derive(Clone)]
enum DfuSeed {
    Scan(dfu::DfuScanKind),
    Progress(dfu::DfuScanKind),
    Installing(dfu::DfuScanKind),
    Error(obc_app::DfuScanError),
    Confirmed(String),
    /// The boot-outcome failure verdict + the version that was staged (`None` when the board could
    /// not name one) — the "UPDATE FAILED" card's two inputs.
    Failed(obc_app::DfuFailure, Option<String>),
}

enum WeatherFault {
    CorruptRequest(u32),
    TruncateRequest(u32),
    FailFrom(u32, u16),
    Latency(u64),
}

struct Args {
    map: String,
    width: u32,
    height: u32,
    scale: u32,
    png: Option<String>,
    /// Start in heading-up orientation with this course (degrees CW from north).
    heading: Option<f32>,
    /// Preload this GPX track for replay.
    gpx: Option<String>,
    /// With `--gpx --png`, the playback time (seconds) to render the fix at; defaults
    /// to the track midpoint.
    at: Option<f64>,
    /// Headless camera center "lon,lat" (microdegrees); defaults to the bbox center.
    center: Option<(i32, i32)>,
    /// Headless zoom multiplier applied to the bbox-fit zoom (picks a finer LOD).
    zoom_mul: f32,
    /// A gesture script applied before a headless `--png` render, to snapshot a specific
    /// screen. Tokens (one char, spaces ignored) mirror the four buttons: `d`/`u` = one Down /
    /// Up step, `p` = press (Select), `h` = Select hold, `b` = back, `B` = back-hold, `H`/`M` =
    /// leave Select / Back held partway (snapshots the in-flight long-press hint), `w` = wait
    /// ~800 ms so an in-flight animation (the Menu needle sweep) settles before the snapshot,
    /// `f` = draw one throwaway frame so draw-time lazy state (the POI-list snapshot) is filled
    /// before the next gesture, `T` = one route-aware tick (sync + open the active route and run
    /// the once-per-load state builds), `Q` = the Up+Select squeeze that opens the universal quick
    /// drawer, with its slide-down settled, `I` = elapse 5 min with no input so the idle-return
    /// timeout fires.
    script: Option<String>,
    /// `--no-backlight`: model a platform whose panel has **no controllable light**
    /// ([`Backlight::available`](obc_ports::Backlight) `== false`). The board is not that platform
    /// any more — it drives a PWM backlight since #1558 — so this is the arrangement a *future*
    /// lightless host would get. The quick drawer then draws three controls instead of four;
    /// nothing else changes.
    no_backlight: bool,
    /// `--expect-screen NAME`: headless `--png` only — refuse to render unless the script landed
    /// on that screen ([`Screen::name`](obc_app::Screen::name), the `screens!` table's own
    /// variant string). A recipe that walks a menu is a hostage to that menu's station order:
    /// insert one row and `p d d d d w p d p` silently snapshots a different screen under the old
    /// filename. Stating the destination turns that into a failed sweep instead of a quietly
    /// wrong PNG.
    expect_screen: Option<String>,
    /// Headless `--png` only: render from the device's real power-on state (Home / Idle,
    /// no route) instead of straight from the map.
    boot: bool,
    /// Folder of `.obcr` routes — the stand-in for the device SD card; defaults to `routes/`.
    routes_dir: Option<String>,
    /// Folder for saved `.gpx` tracks + the in-progress `.obct` log; defaults to `tracks/`.
    tracks_dir: Option<String>,
    /// Convert this GPX into the routes folder and exit. Needs no map.
    import: Option<String>,
    /// Render the device window at the panel's true physical size (needs a saved
    /// calibration). Falls back to the scaled view if uncalibrated.
    physical: bool,
    /// Show the device's 64-color gamut and nothing else. Needs no map.
    palette: bool,
    /// Initial battery charge (0–100 %) shown on the Home gauge; stands in for the not-yet-
    /// wired fuel gauge. Defaults to full.
    battery: Option<u8>,
    /// Headless `--png` only: seed the device's UTC wall-clock anchor to `YYYY-MM-DDTHH:MM`. With
    /// the default `+00:00` offset `local_clock()` returns it verbatim, pinning the POI-detail
    /// "today's hours" weekday + the OPEN/CLOSED-now badge for a reproducible render. Defaults to the
    /// device default (2025-01-01 12:00, a Wednesday noon).
    clock: Option<obc_ports::DateTime>,
    /// `--weather FILE.obcw|demo[:SCENARIO]`: offer the production rain-overlay lease on every frame —
    /// GUI and headless `--png` alike (WX10). Offering is not drawing: the app hands the lease on
    /// only to a screen that declared it wants rain (`Caps::rain_overlay`), so the raster appears
    /// on the WX11 **rain map** and the ordinary Map stays rain-free with a store mounted. A
    /// file is one validated OBCW bundle (`specs/vectors/*.obcw` work directly). `demo`
    /// synthesizes a deterministic bundle over the loaded map's own bbox —
    /// scenarios `scattered` (default) | `drizzle` | `frontal` | `storm` — the WX10 look-tuning
    /// scenes.
    weather: Option<String>,
    /// `--weather-now UNIX`: the UTC instant the rain freshness gate evaluates at. Defaults to the
    /// loaded bundle's **first frame timestamp**, so fixture stores render deterministically; pass
    /// the real time to exercise staleness (an expired instant renders a rain-free map).
    weather_now: Option<i64>,
    /// Headless `--png` only (WX11): draw the dashboard's non-blocking refresh cue (the title
    /// bar's UPDATING slot) — the cached content stays fully visible, which is the point.
    weather_refreshing: bool,
    /// Headless `--png` only (WX11): push the weather alert card through the production
    /// `App::show_weather_alert` seam — `rain[:MIN]`, `storm[:MIN]` or `gust[:MIN]` (default 28
    /// minutes). Drives only the presentation; the *engine* is [`Args::weather_decide`].
    weather_alert: Option<(obc_app::WeatherAlertKind, u16)>,
    /// Headless `--png` only (WX12): run the production ride-decision path for the final frame —
    /// sample the bundle **route-projected** (`App::ride_projection` → `sample_along`) and run
    /// the real alert engine (`App::weather_alert_tick`: thresholds, dedup, cooldown), exactly
    /// as the GUI does every frame. Source-agnostic: it decides over whichever bundle is loaded,
    /// `demo:` or `live`. Opt-in so the WX10/WX11/WX14 fixture renders stay byte-identical (their
    /// scenarios would otherwise grow alert cards).
    weather_decide: bool,
    /// `--weather live` knobs (WX14): the service origin, the corridor radius, and the failure
    /// controls that make an outage, a corrupt tile or a cut connection reproducible on demand.
    live: weather_live::LiveConfig,
    /// `--no-card`: the device has no storage the companion could write a bundle to. §11.7's rule
    /// is that such a device raises **no** weather request at all — urgent included — because
    /// every upload would be answered `error` and the phone would burn on the retry loop.
    no_card: bool,
    /// Headless `--png` only: the UI language `en` | `de` | `fr` | `es` (epic #602). Seeded into
    /// `Settings.language` before the render, so a scripted screen draws its de/fr/es copy from the
    /// i18n catalog — the per-language snapshot mechanism. Defaults to `en` (the device default), so
    /// omitting it leaves the English output byte-identical.
    lang: Option<obc_app::settings::Language>,
    /// Headless `--png` only: replace the Statistics grid's field selection with this comma-separated
    /// list (epic #946, U5 — the `Next: <category>` tiles). Names are the catalogue's kebab-case ids
    /// (`speed`, `next-waypoint`, `next-water`, …; `obc-sim --stat-fields ?` isn't a thing, but an
    /// unknown name fails with the full list). Stands in for walking the Fields editor with a
    /// twenty-token script just to place a tile.
    stat_fields: Option<std::vec::Vec<obc_app::StatField>>,
    /// One typed BLE fixture state for headless snapshots; `+` composes independent link facts.
    ble: Option<BleSeed>,
    /// One mutually-exclusive sensor fixture for headless snapshots.
    sensors: Option<SensorSeed>,
    /// Consume one host request without starting it so its planning spinner stays visible.
    hold: Option<Hold>,
    /// One mutually-exclusive host event injection for headless snapshots.
    inject: Option<Injection>,
    /// Headless `--png` only: engage the **Recalculating freeze** (#1146 P2) after the script, via
    /// `App::debug_set_plan_live`, so the overlay-plane banner renders over whatever map base the
    /// script left showing. The freeze's visible state is otherwise unreachable headlessly: the
    /// flows that start a plan leave the opaque planning spinner as the base (no map to freeze),
    /// and the one gesture that puts a map base back under a live search also cancels the plan.
    freeze: bool,
    /// One mutually-exclusive DFU fixture state for headless snapshots.
    dfu: Option<DfuSeed>,
    /// Headless `--png` only: stamp every loaded route's retention meta (epic #638 S5), so the
    /// Route overview's expiry row renders for a snapshot. `LEVEL:AGE` — `LEVEL` is the retention
    /// `u8` (0 Never · 1 1d · 2 1wk · 3 2wk · 4 1mo · 5 2mo), `AGE` the route's `last_used` as a
    /// duration *ago* from the (`--clock`-pinned) wall clock (`2d` / `19h` / `3600s` / bare seconds),
    /// or `unknown` for "clock never started" (→ the row's `--`). Stands in for the SD retention
    /// sidecar the board reads; without it every route stays `Never` and the row is absent.
    route_retention: Option<(u8, Option<u32>)>,
}

impl Default for Args {
    /// Device resolution + all knobs off — the CLI parser's base. The resolution is the single
    /// [`obc_display`] frame authority, not a re-declared literal (`--size` overrides it for
    /// off-device experiments).
    fn default() -> Self {
        Args {
            map: String::new(),
            width: obc_display::ls021::FRAME_W as u32,
            height: obc_display::ls021::FRAME_H as u32,
            scale: 1,
            png: None,
            heading: None,
            gpx: None,
            at: None,
            center: None,
            zoom_mul: 1.0,
            script: None,
            no_backlight: false,
            expect_screen: None,
            boot: false,
            routes_dir: None,
            tracks_dir: None,
            import: None,
            physical: false,
            palette: false,
            battery: None,
            clock: None,
            weather: None,
            weather_now: None,
            weather_refreshing: false,
            weather_alert: None,
            weather_decide: false,
            live: weather_live::LiveConfig::default(),
            no_card: false,
            lang: None,
            stat_fields: None,
            ble: None,
            sensors: None,
            hold: None,
            inject: None,
            freeze: false,
            dfu: None,
            route_retention: None,
        }
    }
}

impl Args {
    pub(crate) fn routes_dir(&self) -> String {
        self.routes_dir.clone().unwrap_or_else(|| "routes".to_string())
    }

    pub(crate) fn tracks_dir(&self) -> String {
        self.tracks_dir.clone().unwrap_or_else(|| "tracks".to_string())
    }

    /// The persisted-settings file (the device's RRAM stand-in). Holds the shared
    /// [`obc_app::settings`] blob, so relaunching restores units / clock / intervals.
    pub(crate) fn settings_path(&self) -> String {
        "obc-settings.bin".to_string()
    }
}

/// Parse a `--clock` value `YYYY-MM-DDTHH:MM` into an [`obc_ports::DateTime`].
/// Rejects a malformed stamp with a message (out-of-range fields are clamped by `Settings::decode`'s
/// sanitiser when seeded, but the format itself must be well-formed).
fn parse_clock(s: &str) -> Result<obc_ports::DateTime, String> {
    let (date, time) = s.split_once('T').ok_or("--clock format is YYYY-MM-DDTHH:MM")?;
    let mut d = date.split('-');
    let mut t = time.split(':');
    let year = d.next().and_then(|v| v.parse().ok()).ok_or("bad --clock year")?;
    let month = d.next().and_then(|v| v.parse().ok()).ok_or("bad --clock month")?;
    let day = d.next().and_then(|v| v.parse().ok()).ok_or("bad --clock day")?;
    let hour = t.next().and_then(|v| v.parse().ok()).ok_or("bad --clock hour")?;
    let minute = t.next().and_then(|v| v.parse().ok()).ok_or("bad --clock minute")?;
    Ok(obc_ports::DateTime { year, month, day, hour, minute })
}

/// Parse a `--route-retention LEVEL:AGE` value (epic #638 S5). `LEVEL` is the retention `u8`; `AGE`
/// is a `last_used` age *before now* (`2d` / `19h` / `3600s` / bare seconds), or `unknown` for
/// "clock never started" (`last_used == 0`). Returns `(level, Some(secs_ago))` / `(level, None)`.
fn parse_route_retention(s: &str) -> Result<(u8, Option<u32>), String> {
    let (level, age) = s.split_once(':').ok_or("--route-retention format is LEVEL:AGE (e.g. 3:2d or 2:unknown)")?;
    let level: u8 = level.parse().map_err(|_| "bad --route-retention LEVEL (0..5)")?;
    let age = if age == "unknown" { None } else { Some(parse_duration_secs(age)?) };
    Ok((level, age))
}

/// Parse a duration like `2d` / `19h` / `3600s` (bare = seconds) into whole seconds.
fn parse_duration_secs(s: &str) -> Result<u32, String> {
    let (num, mult) = match s.strip_suffix('d') {
        Some(n) => (n, 86_400),
        None => match s.strip_suffix('h') {
            Some(n) => (n, 3_600),
            None => (s.strip_suffix('s').unwrap_or(s), 1),
        },
    };
    let v: u32 = num.parse().map_err(|_| "bad --route-retention AGE duration")?;
    Ok(v.saturating_mul(mult))
}

/// Parse a `--lang` value into a [`Language`](obc_app::settings::Language). Accepts the four
/// ISO-639-1 codes the catalog ships (`en`/`de`/`fr`/`es`); anything else is a located error rather
/// than a silent fall back to English, so a typo in a snapshot script fails loudly.
fn parse_lang(s: &str) -> Result<obc_app::settings::Language, String> {
    use obc_app::settings::Language;
    match s {
        "en" => Ok(Language::En),
        "de" => Ok(Language::De),
        "fr" => Ok(Language::Fr),
        "es" => Ok(Language::Es),
        other => Err(format!("--lang needs en|de|fr|es, got `{other}`")),
    }
}

/// The catalogue's kebab-case field ids, in catalogue order — the `--stat-fields` vocabulary. Kept
/// beside [`parse_stat_fields`] so an added [`StatField`](obc_app::StatField) shows up as a missing
/// arm here rather than as a silently unnameable field.
fn stat_field_id(f: obc_app::StatField) -> &'static str {
    use obc_app::StatField as F;
    match f {
        F::Speed => "speed",
        F::AvgSpeed => "avg-speed",
        F::DistDone => "dist-done",
        F::DistToGo => "dist-to-go",
        F::Climbed => "climbed",
        F::ToClimb => "to-climb",
        F::Grade => "grade",
        F::Elevation => "elevation",
        F::RideTime => "ride-time",
        F::TimeToGo => "time-to-go",
        F::Eta => "eta",
        F::Clock => "clock",
        F::NextWaypoint => "next-waypoint",
        F::WaypointList => "waypoint-list",
        F::HeartRate => "heart-rate",
        F::Power => "power",
        F::Cadence => "cadence",
        F::NextWater => "next-water",
        F::NextCampsite => "next-campsite",
        F::NextLodging => "next-lodging",
        F::NextResupply => "next-resupply",
        F::NextPharmacy => "next-pharmacy",
        F::NextBikeShop => "next-bike-shop",
    }
}

/// An empty [`StatFieldList`](obc_app::StatFieldList) — the grid selection every `--stat-fields`
/// (and `--sensors demo`) list is built onto, since the type starts at the device's default six and
/// only shrinks by index.
fn empty_stat_field_list() -> obc_app::StatFieldList {
    let mut sf = obc_app::StatFieldList::default();
    while !sf.is_empty() {
        sf.remove(0);
    }
    sf
}

/// Parse a `--stat-fields` list into the grid selection. Unknown names fail with the whole
/// vocabulary listed, so a typo in a snapshot script is a loud, self-explaining error.
///
/// Every name is also pushed onto a real [`StatFieldList`](obc_app::StatFieldList) as it is parsed,
/// so a list the grid *cannot hold* fails here just as loudly instead of silently truncating into a
/// snapshot: `push` refuses both past the grid's cap and on a repeat, and either way the frame the
/// script asked for is not the frame it would get.
fn parse_stat_fields(s: &str) -> Result<std::vec::Vec<obc_app::StatField>, String> {
    let mut fields = std::vec::Vec::new();
    let mut grid = empty_stat_field_list();
    for n in s.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let f = obc_app::StatField::ALL.into_iter().find(|f| stat_field_id(*f) == n).ok_or_else(|| {
            let all: std::vec::Vec<&str> = obc_app::StatField::ALL.into_iter().map(stat_field_id).collect();
            format!("--stat-fields: unknown field `{n}`; known: {}", all.join(", "))
        })?;
        if !grid.push(f) {
            return Err(format!(
                "--stat-fields: `{n}` does not fit the grid — it is either a repeat, or past the \
                 grid's cap ({} field(s) accepted before it)",
                grid.len()
            ));
        }
        fields.push(f);
    }
    Ok(fields)
}

fn parse_nav_failure(s: &str, flag: &str) -> Result<NavFailure, String> {
    match s {
        "exhausted" => Ok(NavFailure::Exhausted),
        "nopath" => Ok(NavFailure::NoPath),
        _ => Err(format!("{flag} needs exhausted|nopath")),
    }
}

fn parse_dfu_error(s: &str) -> Result<obc_app::DfuScanError, String> {
    match s {
        "notfound" => Ok(obc_app::DfuScanError::NotFound),
        "unreadable" => Ok(obc_app::DfuScanError::Unreadable),
        "damaged" => Ok(obc_app::DfuScanError::Damaged),
        "toolarge" => Ok(obc_app::DfuScanError::TooLarge),
        "fragmented" => Ok(obc_app::DfuScanError::TooFragmented),
        "untrusted" => Ok(obc_app::DfuScanError::Untrusted),
        other => Err(format!("--dfu error: unknown variant `{other}`")),
    }
}

fn parse_warning(s: &str) -> Result<obc_app::WarningFlags, String> {
    let mut warnings = obc_app::WarningFlags::NONE;
    for token in s.split(',') {
        warnings |= match token.trim() {
            "gps" => obc_app::WarningFlags::NO_GPS,
            "altimeter" | "baro" => obc_app::WarningFlags::NO_ALTIMETER,
            "compass" | "imu" => obc_app::WarningFlags::NO_COMPASS,
            "map" => obc_app::WarningFlags::MAP_SLOW,
            "rec" | "record" => obc_app::WarningFlags::REC_ERROR,
            _ => return Err("--inject warning tokens: gps|altimeter|compass|map|rec".into()),
        };
    }
    Ok(warnings)
}

fn parse_ble(s: &str) -> Result<BleSeed, String> {
    let mut seed = BleSeed::default();
    for part in s.split('+') {
        match part {
            "connected" => seed.connected = true,
            "paired" => seed.paired = true,
            _ if part.starts_with("passkey=") && seed.passkey.is_none() => {
                seed.passkey = Some(
                    part.strip_prefix("passkey=")
                        .and_then(|n| n.parse().ok())
                        .filter(|&n| n <= 999_999)
                        .ok_or("--ble passkey needs 0..=999999")?,
                );
            }
            _ => return Err("--ble needs connected, paired, and/or passkey=N joined by + (N is 0..=999999)".into()),
        }
    }
    Ok(seed)
}

/// The `--inject` vocabulary, stated once — the parser's error text and the `--help` line both read
/// it, so an added form cannot advertise itself in only one of them.
const INJECT_FORMS: &str = "--inject needs nav-fail=KIND|detour-fail=KIND|upload=ID|upload-replace=ID|\
     trip-upload=N|map-transfer=receiving:RECEIVED/TOTAL|map-transfer=installed|map-transfer=failed:KIND|\
     warning=LIST";

/// The `--inject map-transfer` forms, stated once (see [`INJECT_FORMS`]).
const MAP_TRANSFER_FORMS: &str =
    "--inject map-transfer needs receiving:RECEIVED/TOTAL|installed|failed:storage|damaged|notamap|refused";

/// Parse a `--inject map-transfer` value into the board's live transfer state (issue #927): every
/// state the seam can carry — `receiving:RECEIVED/TOTAL` (kibibytes, the unit the seam itself
/// carries), the terminal `installed`, and each `failed:KIND` face.
///
/// There is deliberately no form for an **abort or unplug**, and that is the one state this is
/// short of: those clear the card rather than raising one (the rider caused them, and a red card
/// explaining what they just did is noise), so `None` at the seam is what they look like and there
/// is no frame to shoot.
fn parse_map_transfer(s: &str) -> Result<obc_app::screen::MapTransfer, String> {
    use obc_app::screen::{MapTransfer, MapTransferError};
    if s == "installed" {
        return Ok(MapTransfer::Installed);
    }
    if let Some(kind) = s.strip_prefix("failed:") {
        return Ok(MapTransfer::Failed(match kind {
            "storage" => MapTransferError::Storage,
            "damaged" => MapTransferError::Damaged,
            "notamap" => MapTransferError::NotAMap,
            "refused" => MapTransferError::Refused,
            other => return Err(format!("--inject map-transfer failed: unknown kind `{other}`")),
        }));
    }
    let progress = s.strip_prefix("receiving:").ok_or(MAP_TRANSFER_FORMS)?;
    let (received, total) =
        progress.split_once('/').ok_or("--inject map-transfer receiving needs RECEIVED/TOTAL in KiB")?;
    let received_kib: u32 = received.parse().map_err(|_| "--inject map-transfer: bad RECEIVED (KiB)")?;
    let total_kib: u32 = total.parse().map_err(|_| "--inject map-transfer: bad TOTAL (KiB)")?;
    if total_kib == 0 || received_kib > total_kib {
        return Err("--inject map-transfer: RECEIVED must be ≤ TOTAL and TOTAL non-zero".into());
    }
    Ok(MapTransfer::Receiving { received_kib, total_kib })
}

fn parse_injection(s: &str) -> Result<Injection, String> {
    let (kind, value) = s.split_once('=').ok_or(INJECT_FORMS)?;
    match kind {
        "nav-fail" => Ok(Injection::NavFail(parse_nav_failure(value, "--inject nav-fail")?)),
        "detour-fail" => Ok(Injection::DetourFail(parse_nav_failure(value, "--inject detour-fail")?)),
        "upload" | "upload-replace" => Ok(Injection::Upload {
            id: value.parse().map_err(|_| "--inject upload needs a u64 object id")?,
            replaced: kind == "upload-replace",
        }),
        "trip-upload" => {
            let n: obc_app::CatalogObjectId =
                value.parse().map_err(|_| "--inject trip-upload needs the N of TP{N}.OBT")?;
            // The band here, not at the use site: a `TP{N}.OBT` the trip store cannot carry has no
            // catalog identity to announce, and saturating into one would name a trip that no scan
            // can ever list.
            let id =
                n.checked_add(obc_host_core::TRIP_ID_BASE).ok_or("--inject trip-upload N is past the trip id band")?;
            Ok(Injection::TripUpload { id })
        }
        "map-transfer" => Ok(Injection::MapTransfer(parse_map_transfer(value)?)),
        "warning" => Ok(Injection::Warning(parse_warning(value)?)),
        _ => Err(INJECT_FORMS.into()),
    }
}

/// The `--dfu` vocabulary, stated once (see [`INJECT_FORMS`]).
const DFU_FORMS: &str =
    "--dfu needs scan=KIND|progress=KIND|installing=KIND|error=ERR|confirmed=VERSION|failed=WHY[:VERSION]";

/// Parse a `--dfu failed=WHY[:VERSION]` value into the boot-outcome verdict the "UPDATE FAILED"
/// card carries: why the armed update is not what is running, and the version that was staged (the
/// board leaves it out when the arm marker could not name one).
fn parse_dfu_failed(s: &str) -> Result<DfuSeed, String> {
    let (why, staged) = match s.split_once(':') {
        Some((why, version)) => (why, Some(version.to_string())),
        None => (s, None),
    };
    let why = match why {
        "notstarted" => obc_app::DfuFailure::NotStarted,
        "reverted" => obc_app::DfuFailure::Reverted,
        other => return Err(format!("--dfu failed: unknown reason `{other}` (notstarted|reverted)")),
    };
    Ok(DfuSeed::Failed(why, staged))
}

fn parse_dfu(s: &str) -> Result<DfuSeed, String> {
    let (state, value) = s.split_once('=').ok_or(DFU_FORMS)?;
    match state {
        "scan" => Ok(DfuSeed::Scan(dfu::DfuScanKind::parse(value)?)),
        "progress" => Ok(DfuSeed::Progress(dfu::DfuScanKind::parse(value)?)),
        "installing" => Ok(DfuSeed::Installing(dfu::DfuScanKind::parse(value)?)),
        "error" => Ok(DfuSeed::Error(parse_dfu_error(value)?)),
        "confirmed" => Ok(DfuSeed::Confirmed(value.to_string())),
        "failed" => parse_dfu_failed(value),
        _ => Err(DFU_FORMS.into()),
    }
}

fn parse_weather_fault(s: &str) -> Result<WeatherFault, String> {
    let (kind, value) = s
        .split_once('=')
        .ok_or("--weather-fault needs corrupt-request=N|truncate-request=N|fail-from=N:CODE|latency=MS")?;
    match kind {
        "corrupt-request" => Ok(WeatherFault::CorruptRequest(
            value.parse().map_err(|_| "--weather-fault corrupt-request needs an index")?,
        )),
        "truncate-request" => Ok(WeatherFault::TruncateRequest(
            value.parse().map_err(|_| "--weather-fault truncate-request needs an index")?,
        )),
        "fail-from" => {
            let (n, code) = value.split_once(':').ok_or("--weather-fault fail-from needs N:CODE")?;
            Ok(WeatherFault::FailFrom(
                n.parse().map_err(|_| "--weather-fault fail-from: bad request index")?,
                code.parse().map_err(|_| "--weather-fault fail-from: bad status code")?,
            ))
        }
        "latency" => {
            Ok(WeatherFault::Latency(value.parse().map_err(|_| "--weather-fault latency needs milliseconds")?))
        }
        _ => Err("--weather-fault needs corrupt-request=N|truncate-request=N|fail-from=N:CODE|latency=MS".into()),
    }
}

fn parse_args_from(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--size" => {
                let s = it.next().ok_or("--size needs WxH")?;
                let (w, h) = s.split_once('x').ok_or("--size format is WxH")?;
                a.width = w.parse().map_err(|_| "bad width")?;
                a.height = h.parse().map_err(|_| "bad height")?;
            }
            "--scale" => a.scale = it.next().and_then(|s| s.parse().ok()).ok_or("bad --scale")?,
            "--png" => a.png = Some(it.next().ok_or("--png needs a path")?),
            "--heading" => a.heading = Some(it.next().and_then(|s| s.parse().ok()).ok_or("bad --heading")?),
            "--gpx" => a.gpx = Some(it.next().ok_or("--gpx needs a path")?),
            "--at" => a.at = Some(it.next().and_then(|s| s.parse().ok()).ok_or("bad --at")?),
            "--center" => {
                let s = it.next().ok_or("--center needs lon,lat")?;
                let (lon, lat) = s.split_once(',').ok_or("--center format is lon,lat")?;
                a.center = Some((
                    lon.trim().parse().map_err(|_| "bad --center lon")?,
                    lat.trim().parse().map_err(|_| "bad --center lat")?,
                ));
            }
            "--zoom" => a.zoom_mul = it.next().and_then(|s| s.parse().ok()).ok_or("bad --zoom")?,
            "--no-backlight" => a.no_backlight = true,
            "--script" => a.script = Some(it.next().ok_or("--script needs a token string")?),
            "--expect-screen" => a.expect_screen = Some(it.next().ok_or("--expect-screen needs a screen name")?),
            "--boot" => a.boot = true,
            "--routes-dir" => a.routes_dir = Some(it.next().ok_or("--routes-dir needs a path")?),
            "--tracks-dir" => a.tracks_dir = Some(it.next().ok_or("--tracks-dir needs a path")?),
            "--import" => a.import = Some(it.next().ok_or("--import needs a GPX path")?),
            "--physical" => a.physical = true,
            "--palette" => a.palette = true,
            "--battery" => {
                a.battery = Some(
                    it.next().and_then(|s| s.parse().ok()).filter(|&b| b <= 100).ok_or("--battery needs 0..=100")?,
                )
            }
            "--clock" => {
                a.clock = Some(parse_clock(&it.next().ok_or("--clock needs YYYY-MM-DDTHH:MM")?)?);
            }
            "--weather" => a.weather = Some(it.next().ok_or("--weather needs a .obcw file or demo source")?),
            "--weather-now" => {
                a.weather_now = Some(it.next().and_then(|s| s.parse().ok()).ok_or("--weather-now needs unix seconds")?);
            }
            "--weather-refreshing" => a.weather_refreshing = true,
            "--weather-service" => {
                a.live.service = it.next().ok_or("--weather-service needs an origin URL")?;
            }
            "--weather-radius-km" => {
                a.live.radius_km =
                    Some(it.next().and_then(|s| s.parse().ok()).ok_or("--weather-radius-km needs kilometres")?);
            }
            "--no-card" => a.no_card = true,
            "--weather-offline" => a.live.controls.offline = true,
            "--weather-fault" => match parse_weather_fault(&it.next().ok_or("--weather-fault needs a value")?)? {
                WeatherFault::CorruptRequest(n) => a.live.controls.corrupt_request = Some(n),
                WeatherFault::TruncateRequest(n) => a.live.controls.truncate_request = Some(n),
                WeatherFault::FailFrom(n, code) => a.live.controls.fail_from = Some((n, code)),
                WeatherFault::Latency(ms) => a.live.controls.latency = std::time::Duration::from_millis(ms),
            },
            "--weather-alert" => {
                let spec = it.next().ok_or("--weather-alert needs rain[:MIN], storm[:MIN] or gust[:MIN]")?;
                let (kind, min) = spec.split_once(':').unwrap_or((spec.as_str(), "28"));
                let kind = match kind {
                    "rain" => obc_app::WeatherAlertKind::Rain,
                    "storm" => obc_app::WeatherAlertKind::Storm,
                    "gust" => obc_app::WeatherAlertKind::Gust,
                    _ => return Err("--weather-alert: kind must be rain, storm or gust".into()),
                };
                let minutes: u16 = min.parse().map_err(|_| "--weather-alert: bad minutes")?;
                a.weather_alert = Some((kind, minutes));
            }
            "--weather-decide" => a.weather_decide = true,
            "--route-retention" => {
                a.route_retention =
                    Some(parse_route_retention(&it.next().ok_or("--route-retention needs LEVEL:AGE")?)?);
            }
            "--lang" => {
                a.lang = Some(parse_lang(&it.next().ok_or("--lang needs en|de|fr|es")?)?);
            }
            "--stat-fields" => {
                a.stat_fields = Some(parse_stat_fields(&it.next().ok_or("--stat-fields needs a comma list")?)?);
            }
            "--ble" => a.ble = Some(parse_ble(&it.next().ok_or("--ble needs a value")?)?),
            "--hold" => {
                a.hold = Some(match it.next().ok_or("--hold needs nav|detour")?.as_str() {
                    "nav" => Hold::Nav,
                    "detour" => Hold::Detour,
                    _ => return Err("--hold needs nav|detour".into()),
                });
            }
            "--freeze" => a.freeze = true,
            "--sensors" => {
                a.sensors = Some(match it.next().ok_or("--sensors needs demo|screen")?.as_str() {
                    "demo" => SensorSeed::Demo,
                    "screen" => SensorSeed::Screen,
                    _ => return Err("--sensors needs demo|screen".into()),
                });
            }
            "--dfu" => a.dfu = Some(parse_dfu(&it.next().ok_or("--dfu needs a value")?)?),
            "--inject" => a.inject = Some(parse_injection(&it.next().ok_or("--inject needs a value")?)?),
            other if other.starts_with('-') => return Err(format!("unexpected option: {other}")),
            other => {
                if a.map.is_empty() {
                    a.map = other.to_string();
                } else {
                    return Err(format!("unexpected arg: {other}"));
                }
            }
        }
    }
    // `--palette` and `--import` need no map file.
    if a.map.is_empty() && !a.palette && a.import.is_none() {
        return Err("missing map path (one .obcm file)".into());
    }
    Ok(a)
}

fn parse_args() -> Result<Args, String> {
    parse_args_from(std::env::args().skip(1))
}

fn color_of(c: u16) -> Rgb888 {
    let (r, g, b) = rgb565_to_device64(c);
    Rgb888::new(r, g, b)
}

/// The fixed card-free stand-in the sim answers a card-free scan with (the sim has no FAT to scan):
/// ~1.2 GiB → the System screen reads "1.2 GB".
const SIM_CARD_FREE: u64 = 1_288_490_188;

/// A canned scan-hit set for the sim's fake sensor manager (SE7, epic #707): two HR straps, one
/// power meter, one unnamed cadence sensor — so any kind's scan list shows something (the unnamed
/// one exercises the address fallback, the second HR hit a multi-row list). The scan-list screen
/// filters to the row's quantity by `slot`. Shared with the interactive GUI ([`gui`]) so the two
/// sensor paths cannot drift.
fn fake_scan_hits() -> [obc_app::SensorScanHit; 4] {
    [
        obc_app::SensorScanHit::new(0, 1, [0x66, 0x55, 0x44, 0x33, 0x22, 0x11], "HRM-Dual", -58),
        obc_app::SensorScanHit::new(0, 1, [0x21, 0x43, 0x65, 0x87, 0xA9, 0xCB], "Forerunner", -74),
        obc_app::SensorScanHit::new(1, 0, [0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F], "Stages LR", -67),
        obc_app::SensorScanHit::new(2, 0, [0x01, 0x02, 0x03, 0x04, 0x05, 0x06], "", -80),
    ]
}

/// The headless driver's repositories, threaded as one value: three folder stores plus the open
/// ride log. The tuple keeps them visibly one repository family and keeps [`settle`]'s signature
/// from growing four more parameters.
struct Stores<'a> {
    routes: &'a mut RouteStore,
    rides: &'a mut RideStore,
    trips: &'a mut TripStore,
    tracks: &'a mut TrackStore,
}

/// What only the headless driver can do: a fixed card-free figure (the sim has no FAT to scan) and
/// the update answers `--dfu` stages. Persistence is deliberately absent for **both** durable
/// records — a `--png` run must not write the developer's settings file, nor their alert anchors —
/// and the defaults acknowledge the writes so the domains settle instead of parking.
#[derive(Default)]
struct HeadlessPlatform {
    /// The `--dfu` scan answer, taken by the first scan the flow asks for.
    scan: Option<Result<obc_app::dfu::DfuScanReport, obc_app::dfu::DfuScanError>>,
    /// The `--dfu` install answer. `None` leaves the arm in flight — the progress spinner.
    install: Option<Result<(), obc_app::dfu::DfuInstallError>>,
}

impl HostPlatform for HeadlessPlatform {
    fn measure_free_space(&mut self) -> Result<u64, obc_app::device_core::StorageInfoError> {
        Ok(SIM_CARD_FREE)
    }

    fn scan_update(&mut self) -> Option<Result<obc_app::dfu::DfuScanReport, obc_app::dfu::DfuScanError>> {
        self.scan.take()
    }

    fn arm_install(&mut self) -> Option<Result<(), obc_app::dfu::DfuInstallError>> {
        self.install.take()
    }
}

/// The most passes one settle runs. A route plan is stepped once per pass here exactly as it is on
/// the board, so the ceiling has to clear a whole A* search; it is a runaway guard, not a budget.
const MAX_SETTLE_PASSES: usize = 100_000;

/// Passes with nothing owed before the device counts as settled. Two, because an outcome the
/// executor produced is consumed by the *next* pass — one quiet pass alone would stop with an
/// answer still in the inbox.
const QUIET_PASSES: usize = 2;

/// Run DeviceCore passes until the device stops asking for anything.
///
/// A scripted host has no display frame to yield between bounded steps, so it settles here instead
/// of once per frame: the same `App::run_pass` + typed executor the GUI runs, looped until no
/// effect is owed, no deferred value is in flight and no planner step is left.
///
/// The **ride clock stands still** at zero and the UI clock is the script's own, which is exactly
/// what the headless path has always done: it drives the UI with synthesized button events and only
/// ever ticked the ride on an explicit `T`. A settling pass must not age a ride nobody is riding.
#[allow(clippy::too_many_arguments)]
fn settle(
    host: &mut HostLoop,
    session: &mut ActiveRouteSession,
    app: &mut App,
    stores: &mut Stores<'_>,
    reader: &Reader,
    elev: &mut dyn obc_route::ElevationSource,
    platform: &mut HeadlessPlatform,
    weather: Option<&obc_app::WeatherSnapshot>,
    now: u32,
) {
    let mut quiet = 0usize;
    for _ in 0..MAX_SETTLE_PASSES {
        session.sync(app, stores.routes);
        let (mut plan, owed) = {
            let src = stores.routes.active_source();
            let route = match (session.index(), src.as_ref()) {
                (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
                _ => None,
            };
            let mut loc = NoFix;
            let sensors = obc_ports::Sensors { track: stores.tracks.sink(), ..obc_ports::Sensors::new(&mut loc) };
            let plan = host.pass(
                app,
                obc_app::device_core::PassClock { ride: obc_ports::RideClock(0), ui: InputClock(now) },
                &[],
                sensors,
                route.as_ref(),
                weather,
                gui::SIM_SUPPORT,
            );
            let owed = plan.effects.has_pending() || plan.immediate || !plan.derived_needs.is_empty();
            (plan, owed)
        };
        host.execute(
            app,
            &mut plan,
            session,
            stores.routes,
            stores.rides,
            stores.tracks,
            stores.trips,
            reader,
            elev,
            platform,
        );
        if owed || host.is_planning() {
            quiet = 0;
        } else {
            quiet += 1;
            if quiet >= QUIET_PASSES {
                return;
            }
        }
    }
    eprintln!("warning: the headless device did not settle in {MAX_SETTLE_PASSES} passes");
}

/// The planner error a `--inject nav-fail=` / `detour-fail=` seed stands for.
fn nav_error(kind: NavFailure) -> obc_route::NavError {
    match kind {
        NavFailure::Exhausted => obc_route::NavError::Exhausted,
        NavFailure::NoPath => obc_route::NavError::NoPath,
    }
}

/// A location port that never has a fix — the settling pass's sensor input. The headless driver
/// drives position through the GPX replay below, never through a settle.
struct NoFix;
impl obc_ports::LocationSource for NoFix {
    fn poll(&mut self) -> Option<obc_ports::Fix> {
        None
    }
}

/// Encode a framebuffer to a PNG, upscaling by `scale` with nearest-neighbor so the
/// device's hard pixel edges stay crisp.
fn write_png(fb: &Framebuffer, scale: u32, path: &str) -> Result<(), String> {
    let (w, h) = (fb.width(), fb.height());
    let base = image::RgbImage::from_raw(w, h, fb.as_rgb888().to_vec()).ok_or("framebuffer size mismatch")?;
    let out = if scale > 1 {
        image::imageops::resize(&base, w * scale, h * scale, image::imageops::FilterType::Nearest)
    } else {
        base
    };
    out.save(path).map_err(|e| format!("save_png failed: {e}"))
}

/// A scripted [`InputSource`] that replays a fixed queue of raw events — the
/// headless counterpart to the control panel's [`device_input::DeviceInput`].
struct ScriptInput(std::collections::VecDeque<InputEvent>);
impl InputSource for ScriptInput {
    fn poll(&mut self) -> Option<InputEvent> {
        self.0.pop_front()
    }
}

/// A host-side effect a script token requests from `apply_script`'s hook closure.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScriptHook {
    /// Draw one throwaway frame (the `f` token).
    Render,
    /// Run one route-aware pass (the `T` token).
    Tick,
}

/// Feed one batch of raw events to the app at time `now` (ms).
fn feed(app: &mut App, now: u32, events: Vec<InputEvent>) {
    app.handle_input(InputClock(now), &mut ScriptInput(events.into()));
}

/// Apply a gesture script (see `Args::script`) to `app`. Synthesizes the raw four-button
/// events with a rising clock — including the threshold crossing that turns a held button
/// into a `Hold`/`BackHold` — exactly as the real recognizer would see them.
///
/// `hook` runs the host-side effects a token asks for: [`ScriptHook::Render`] draws one throwaway
/// headless frame against the current app state — the `f` token uses it to **flush lazy draw-time
/// state** that only fills at draw (the POI-list snapshot, then the detail's hours read), so a
/// script can `p` into a POI *and then* `f p` to open its detail. Without an `f` the whole script
/// runs before the single final render, so lazy state never fills mid-script.
/// [`ScriptHook::Tick`] runs one **route-aware tick** (the `T` token): the GUI ticks every frame,
/// but the headless script path never does — so route-derived `Activity` state (`route_total_m`,
/// the climbs/waypoints caches) stays unbuilt without it. A mid-ride flow that *reads* that state
/// (the Detour chooser, #882) scripts a `T` after starting the ride.
fn apply_script(app: &mut App, script: &str, start_ms: u32, hook: &mut dyn FnMut(&mut App, ScriptHook, u32)) -> u32 {
    let down = |b| InputEvent::Button(ButtonEvent::Down(b));
    let up = |b| InputEvent::Button(ButtonEvent::Up(b));
    let hold = obc_app::DEFAULT_HOLD_MS;
    let mut now: u32 = start_ms;

    // One selection step: feed it, then nudge the clock.
    let step = |app: &mut App, now: &mut u32, dir: i32| {
        feed(app, *now, vec![InputEvent::Step(dir)]);
        *now += 30;
    };
    // A tap: down, then up 80 ms later (well under the long-press threshold).
    let tap = |app: &mut App, now: &mut u32, b| {
        feed(app, *now, vec![down(b)]);
        *now += 80;
        feed(app, *now, vec![up(b)]);
        *now += 30;
    };
    // A long-press: hold past the threshold (one empty tick fires `Hold`/`BackHold`), then release.
    let press_hold = |app: &mut App, now: &mut u32, b| {
        feed(app, *now, vec![down(b)]);
        *now += hold + 80;
        feed(app, *now, vec![]);
        *now += 30;
        feed(app, *now, vec![up(b)]);
        *now += 30;
    };
    // Held partway (no release, no threshold crossing): snapshots the in-flight long-press hint.
    let partial_hold = |app: &mut App, now: &mut u32, b| {
        feed(app, *now, vec![down(b)]);
        *now += hold * 55 / 100; // ~55% toward the threshold
        feed(app, *now, vec![]); // samples the in-flight progress for the render
    };
    // A **chord** (#1515 D2): two buttons squeezed inside the 100 ms window, released together.
    // The recognizer swallows both, so the script gets the drawer and no constituent gesture.
    let chord = |app: &mut App, now: &mut u32, a, b| {
        feed(app, *now, vec![down(a)]);
        *now += 30;
        feed(app, *now, vec![down(b)]);
        *now += 60;
        feed(app, *now, vec![up(b), up(a)]);
        *now += 30;
    };

    for ch in script.chars() {
        match ch {
            ' ' => {}
            'd' => step(app, &mut now, 1),
            'u' => step(app, &mut now, -1),
            'p' => tap(app, &mut now, Button::Select),
            'b' => tap(app, &mut now, Button::Back),
            'h' => press_hold(app, &mut now, Button::Select),
            'B' => press_hold(app, &mut now, Button::Back),
            'H' => partial_hold(app, &mut now, Button::Select),
            'M' => partial_hold(app, &mut now, Button::Back),
            // Settle: step the clock ~800 ms in animation-sized ticks (a sweep integrates a
            // dt-capped step per poll, so one big jump would leave it mid-flight) until any
            // time-driven animation (the Menu needle) has finished. Not for use after `H`/`M` —
            // the empty feeds would cross the hold threshold and fire the `Hold`/`BackHold`
            // those tokens deliberately leave armed.
            'w' => {
                for _ in 0..8 {
                    now += 100;
                    feed(app, now, vec![]);
                }
            }
            // Draw one throwaway frame to flush lazy draw-time state (the POI-list snapshot / the
            // detail's hours read) so the next gesture sees it — e.g. `p f p` opens a POI list, fills
            // its snapshot, then presses a POI into its detail.
            'f' => hook(app, ScriptHook::Render, now),
            // One route-aware tick (see the fn doc): sync + open the active route and run the
            // once-per-load state builds the GUI's per-frame tick would have run.
            'T' => hook(app, ScriptHook::Tick, now),
            // Idle-elapse: jump the clock 5 min forward with no input and run one animation pass, so
            // the app-level idle-return timeout (Part B) fires deterministically for a snapshot —
            // e.g. `B u p I` sits in Settings, elapses, and lands back on Home. Longer than every
            // configurable timeout (max 5 min), so it fires for any `Idle return` setting but Never.
            // The universal quick drawer's Up+Select squeeze, then its slide-down settled — one
            // token, because every drawer frame starts with it.
            'Q' => {
                chord(app, &mut now, Button::Up, Button::Select);
                for _ in 0..8 {
                    now += 40;
                    feed(app, now, vec![]);
                }
            }
            // The contextual drawer's Down+Back squeeze, then its slide-up settled — the bottom
            // sheet's counterpart to `Q`.
            'C' => {
                chord(app, &mut now, Button::Down, Button::Back);
                for _ in 0..8 {
                    now += 40;
                    feed(app, now, vec![]);
                }
            }
            'I' => {
                now += 5 * 60_000 + 1_000;
                feed(app, now, vec![]);
            }
            other => eprintln!("warning: ignoring unknown --script token '{other}'"),
        }
    }
    now
}

const HELP: &str = r#"OpenBikeComputer desktop simulator

Usage: obc-sim <MAP.obcm> [OPTIONS]
       obc-sim --palette [--png OUT]
       obc-sim --import TRACK.gpx [--routes-dir DIR]

Map and output:
  --size WxH              Frame size (default: device 240x320)
  --scale N               Integer PNG/window scale (default: 1)
  --png PATH              Render one device frame to PNG and exit
  --palette               Show or save the device 64-colour palette
  --center LON,LAT        Headless camera centre in microdegrees
  --zoom MULT             Headless bbox-fit zoom multiplier
  --heading DEG           Start in heading-up mode at this course

Ride and storage fixtures:
  --gpx PATH              Replay a GPX track
  --at SECONDS            GPX playback time for a headless render
  --routes-dir DIR        Route-store directory (default: routes/)
  --tracks-dir DIR        Ride/track-store directory (default: tracks/)
  --import PATH           Convert a GPX into the route store and exit
  --route-retention L:A   Set route retention LEVEL and AGE (for example 3:2d)

Device state:
  --boot                  Start headless rendering at the power-on Home screen
  --battery PCT           Initial battery charge, 0..=100
  --clock DATE            UTC anchor, YYYY-MM-DDTHH:MM
  --lang LANG             UI language: en|de|fr|es
  --stat-fields LIST      Comma-separated Statistics field ids
  --physical              Use saved physical-size calibration in the GUI
  --ble STATE             connected|paired|passkey=N (join independent facts with +)
  --sensors MODE          demo|screen

Scripted snapshots:
  --script TOKENS         Apply device-button script tokens before rendering
                          (d/u step, p press, b back, h/B hold, H/M partial hold,
                           Q quick-drawer squeeze, C context-drawer squeeze,
                           w wait, f frame, T tick, I idle)
  --no-backlight          Model a panel with no controllable light (three quick-drawer controls)
  --expect-screen NAME    Refuse unless the script lands on this screen
  --hold PLAN             Consume without starting one request: nav|detour
  --inject EVENT          nav-fail=KIND|detour-fail=KIND|upload=ID|
                          upload-replace=ID|trip-upload=N (TP{N}.OBT)|warning=LIST|
                          map-transfer=receiving:RECEIVED/TOTAL|
                          map-transfer=installed|map-transfer=failed:KIND
  --dfu STATE             scan=KIND|progress=KIND|installing=KIND|
                          error=ERR|confirmed=VERSION|failed=WHY[:VERSION]
  --freeze                Show the live recalculation freeze over the map

Weather (independent product controls):
  --weather SOURCE        FILE.obcw|demo[:SCENARIO]|live (see README for scenarios)
  --weather-now UNIX      Freshness instant
  --weather-refreshing    Show the non-blocking refresh cue
  --weather-alert ALERT   rain[:MIN]|storm[:MIN]|gust[:MIN]
  --weather-decide        Run the production route-projected alert decision
  --weather-service URL   Live weather-service origin
  --weather-radius-km KM  Live corridor radius
  --weather-offline       Force the live service offline
  --weather-fault FAULT   corrupt-request=N|truncate-request=N|
                          fail-from=N:CODE|latency=MS (repeat to compose)
  --no-card               Simulate no writable companion storage

Other:
  -h, --help              Print this help

`--png` always renders the device's RGB222/64-colour output. Housing colorways and
display calibration remain available from the GUI control panel.
"#;

fn main() {
    if std::env::args().skip(1).any(|arg| arg == "--help" || arg == "-h") {
        print!("{HELP}");
        return;
    }
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{HELP}");
            std::process::exit(2);
        }
    };

    // `--palette`: the device's 64-color gamut on a standalone color-test screen. Needs no
    // map. With `--png` it writes the frame headlessly (diffable in CI); else a minimal window.
    if args.palette {
        if let Some(path) = &args.png {
            let mut fb = Framebuffer::new(args.width, args.height);
            palette::draw_palette(&mut fb);
            if let Err(e) = write_png(&fb, args.scale, path) {
                eprintln!("{e}");
                std::process::exit(1);
            }
            eprintln!("wrote {path}");
        } else if let Err(e) = palette::run(args.width, args.height, args.scale) {
            eprintln!("palette error: {e}");
            std::process::exit(1);
        }
        return;
    }

    // `--import` converts a GPX into the routes folder (the device's USB-drop path). Needs no map.
    if let Some(gpx) = &args.import {
        let dir = args.routes_dir();
        let mut store = RouteStore::open(&dir);
        match store.import_gpx(std::path::Path::new(gpx)) {
            Ok(s) => eprintln!(
                "imported {gpx} → {dir}/ | {} km, +{} m / -{} m | {} pts, {} chunks, ele {}..{} m",
                (s.total_distance_m + 500) / 1000,
                s.total_ascent_m,
                s.total_descent_m,
                s.point_count,
                s.chunk_count,
                s.min_ele_m,
                s.max_ele_m
            ),
            Err(e) => {
                eprintln!("import failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // One map is one `.obcm` file, read whole.
    let map = map_file::MapSource::load_single(&args.map).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });

    // Parse the tables **once**, here, for the process lifetime — the shape the device uses (parse
    // at boot, hold for the session), not a per-frame rebuild. A file that does not parse is
    // refused before a single frame renders and the sim exits non-zero saying why.
    let map = map_file::LoadedMap::open(map).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    {
        let reader = map.reader();
        eprintln!(
            "OBCM v{} | bbox {:?} | {} LODs | {} styles",
            reader.version,
            reader.bbox,
            reader.lods().len(),
            (0..=255).filter(|&i| reader.style(i).is_some()).count()
        );
        for (i, l) in reader.lods().iter().enumerate() {
            eprintln!(
                "  LOD {i}: max_mpp {} | {} nodes | chunk_size {} | {} chunks",
                l.max_mpp, l.node_count, l.chunk_size, l.chunk_count
            );
        }
    }

    // Headless mode: render one frame through the shared app, save PNG, exit.
    if let Some(path) = &args.png {
        let tables = map.tables();
        // One reader over the one map file — the map plane, nav, POI, hours and routing all read
        // it, exactly as they do on the device.
        let reader = map.reader();
        let (mut cx, mut cy, mut zoom) = initial_camera(&reader, args.width);
        if let Some((lon, lat)) = args.center {
            cx = lon;
            cy = lat;
        }
        zoom *= args.zoom_mul;
        let mut state = AppState::new(cx, cy, zoom);
        if let Some(b) = args.battery {
            state.device.battery_pct = b;
        }
        // `--heading` renders a rotated (heading-up) frame; the rotation derives from the
        // fix's course, so seed one at the map center.
        if let Some(deg) = args.heading {
            state.heading_up = true;
            state.user_fix = Some(Fix { lat: cy, lon: cx, course: Some(deg), speed_mps: None });
        }
        // `--gpx` renders the replayed fix at `--at` (default: track midpoint). Seed the
        // camera/heading from that fix now; the replay up to `--at` runs below (after the
        // route opens) so the snapshot shows live riding state, not just a static marker.
        let mut player: Option<GpxPlayer> = None;
        let mut replay_to = 0.0_f64;
        if let Some(path) = &args.gpx {
            match Track::load(std::path::Path::new(path)) {
                Ok(track) => {
                    let mut p = GpxPlayer::new(track);
                    let at = args.at.unwrap_or(p.duration() / 2.0);
                    replay_to = at;
                    p.seek(at);
                    if let Some(fix) = p.poll() {
                        state.heading_up = fix.course.is_some();
                        state.user_fix = Some(fix);
                        state.cam_lon = fix.lon;
                        state.cam_lat = fix.lat;
                    }
                    player = Some(p);
                }
                Err(e) => {
                    eprintln!("cannot load GPX: {e}");
                    std::process::exit(1);
                }
            }
        }
        let mut app = if args.boot { App::new_idle(state) } else { App::new(state) };
        // `--weather` (WX10/WX11): built *before* the settings seed so the wall clock can anchor
        // on the store's effective instant (screens' `now_utc` then agrees with the rain lease)
        // and the script's rain-map time-steps clamp against the real frame count.
        let map_bbox = (reader.bbox.min_lon, reader.bbox.min_lat, reader.bbox.max_lon, reader.bbox.max_lat);
        // The corridor a live fetch centres on: the rider's fix when there is one (`--gpx` /
        // `--center` have already seeded it), else the camera. The *service* never receives it —
        // it only shapes which immutable objects get Range-read.
        let wx_seed_pos = app.state.user_fix.map(|f| (f.lat, f.lon)).unwrap_or((app.state.cam_lat, app.state.cam_lon));
        let wx_source = args
            .weather
            .as_ref()
            .map(|arg| weather_live::build(arg, args.weather_now, map_bbox, &args.live, wx_seed_pos, !args.no_card))
            .unwrap_or(weather_live::WeatherSource { store: None, live: None, clock_anchor: None });
        let mut weather = wx_source.store;
        let weather_anchor = wx_source.clock_anchor;
        // Headless is one fetch, by design: a `--png` render must be a single, reportable
        // transaction. The report goes to stdout beside the other render diagnostics — never
        // into the emulated device pixels, which carry no provenance badge.
        if let Some(live) = wx_source.live.as_ref() {
            let report = &live.report;
            if args.no_card {
                println!("weather live: no card — §11.7: no storage, no requests (nothing was fetched)");
            } else {
                match (&report.generation, &report.error) {
                    (_, Some(error)) => println!("weather live: FAILED — {error}"),
                    // The dataset's honest summary, now that there is no product to name: which
                    // generation answered, what it cost, and how much of the corridor was measured
                    // dry rather than fetched.
                    (Some(generation), None) => println!(
                        "weather live: generation {generation} | {} B bundle | service {} req, {} B | MET {} req, {} B | {} dry shard(s) | {}",
                        report.bundle_bytes,
                        report.service_requests,
                        report.service_bytes,
                        report.met_requests,
                        report.met_bytes,
                        report.dry_shards,
                        report.no_rain_map.as_deref().unwrap_or("rain map available")
                    ),
                    (None, None) => println!(
                        "weather live: hourly only — {}",
                        report.no_rain_map.as_deref().unwrap_or("the manifest could not be read")
                    ),
                }
            }
        }
        // WX11: with a weather store and no explicit `--clock`, pin the app clock to the weather
        // instant. An explicit `--clock` always wins — the WX10 map sweeps pass one, so their
        // output stays byte-identical. In live mode that instant is the *real* clock (see
        // `weather_live`): anchoring on the newest frame would hide a stalled baker.
        let weather_clock = if args.clock.is_none() {
            weather_anchor.map(|now| obc_ports::DateTime::from_unix(now.max(0) as u64 as u32))
        } else {
            None
        };

        // `--clock` / `--lang` seed the headless Settings. `--clock` pins the UTC wall-clock anchor;
        // with the default `+00:00` offset `local_clock()` returns it verbatim for the POI-detail
        // weekday + OPEN/CLOSED-now badge. `--lang` selects the UI language (epic #602) so a scripted
        // screen draws its de/fr/es copy. Both stay at the device default otherwise — with neither
        // flag `set_settings` isn't called, and `--clock` alone still leaves `language` English, so
        // the existing snapshots' output is byte-unchanged. `set_settings` restamps the WallClock
        // from this local set-point (see `App::set_settings`).
        if args.clock.is_some()
            || weather_clock.is_some()
            || args.lang.is_some()
            || args.stat_fields.is_some()
            || args.sensors.is_some()
        {
            let mut settings = obc_app::settings::Settings::default();
            if let Some(clock) = args.clock {
                settings.clock = clock;
            } else if let Some(clock) = weather_clock {
                settings.clock = clock;
            }
            if let Some(lang) = args.lang {
                settings.language = lang;
            }
            // `--sensors demo` (epic #707, SE5): pin the three new sensor tiles onto the visible
            // Statistics page so the snapshot shows them. A dedicated demo selection — HR / PWR /
            // RPM first, then a few live neighbours — replacing the default six.
            if args.sensors == Some(SensorSeed::Demo) {
                use obc_app::StatField;
                let mut sf = settings.stat_fields;
                while !sf.is_empty() {
                    sf.remove(0);
                }
                for f in [
                    StatField::HeartRate,
                    StatField::Power,
                    StatField::Cadence,
                    StatField::Speed,
                    StatField::RideTime,
                    StatField::Climbed,
                ] {
                    sf.push(f);
                }
                settings.stat_fields = sf;
            }
            // `--stat-fields` (epic #946, U5): replace the grid selection wholesale, so a snapshot can
            // put a `Next: <category>` tile (or any other field) on the visible page without walking
            // the Fields editor. Applied after `--sensors demo` so an explicit list always wins.
            if let Some(fields) = &args.stat_fields {
                let mut sf = empty_stat_field_list();
                for f in fields {
                    // Can't fail: `parse_stat_fields` pushed the identical list onto an identical
                    // grid and rejected the argument outright if any of it didn't fit.
                    let added = sf.push(*f);
                    debug_assert!(added, "`--stat-fields` is validated at parse time");
                }
                settings.stat_fields = sf;
            }
            // `--sensors screen` (SE7): two saved slots so the row screen reads Connected / Searching
            // (the Not-set third stays empty). A settings write, so it survives into the row status
            // gate; the live phase + battery come from the status snapshot pushed after the script.
            if args.sensors == Some(SensorSeed::Screen) {
                settings.saved_sensors[0] = obc_app::SavedSensor::saved(1, [0x66, 0x55, 0x44, 0x33, 0x22, 0x11]);
                settings.saved_sensors[1] = obc_app::SavedSensor::saved(0, [0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F]);
            }
            app.set_settings(settings);
        }
        // Mirror the map's §8.6 routing-profile names for the Bike-type screen + overview label (N5),
        // and whether it carries a nav graph at all (#882: gates the ride menu's Detour station).
        app.set_nav_profiles(tables.nav_profiles());
        app.set_map_nav_graph(tables.has_nav_graph());
        // Device-info built-ins for the System settings screen (T8 item 6): the running firmware
        // version (the sim's own crate version stands in for the board's git-describe tag) and the
        // loaded map's name (filename stem) + OBCM version from the parsed header. The card-free scan
        // is answered after the script (below), mirroring the on-entry FAT scan seam.
        // The panel-light capability the drawer's root row is built from (#1515 D2). The headless
        // host models a lit panel, like the window does, so a snapshot shows the four-icon
        // arrangement a rider gets on a device with a light — the board included, since #1558 —
        // while `--no-backlight` renders the three-control one a lightless host would show.
        app.set_backlight_available(!args.no_backlight);
        app.set_fw_version(env!("CARGO_PKG_VERSION"));
        let map_name = map.source.display_name();
        app.set_map_info(&map_name, tables.version);
        // Load the routes folder so the Route menu has real entries and a picked route
        // can be drawn.
        let mut store = RouteStore::open(args.routes_dir());
        app.set_routes_with_ids(store.catalog(), store.ids());
        // The same host-protocol owner the interactive simulator drives. Headless runs each plan
        // to completion inside a pass; its planned detour stays resident here until commit/cancel.
        let mut host = HostLoop::new();
        // `--route-retention` (epic #638 S5): overlay every route's retention meta so the Route
        // overview's expiry row renders (the board reads this from the SD retention sidecar). The
        // `last_used` stamp is anchored to the wall clock — `AGE` seconds before now — so the
        // countdown is deterministic regardless of the absolute `--clock`; `unknown` leaves it 0.
        if let Some((level, age)) = args.route_retention {
            let now = app.wall_unix_now();
            let last_used = age.map_or(0, |secs| now.saturating_sub(secs));
            // Into the **sidecar**, not straight onto the app: that is where the board reads it
            // from, and it is what makes the injection survive the catalog re-read the device's
            // first pass orders. Overlaying the app's copy alone reverted on that read.
            let ids: Vec<_> = store.ids().to_vec();
            for id in ids {
                store.set_retention(id, obc_app::Retention::from_u8(level));
                store.stamp_route_used(id, last_used);
            }
            let metas = store.retention_metas();
            app.set_route_meta(&metas);
        }
        // Scan the `.obt` trips beside the routes (epic #526, TR2) — grouped-route folders. Fed
        // **after** the routes so the stage ids resolve against the catalog. The TR3 menu draws the
        // folder rows; until then the grouping is resolved but unrendered (the flat menu is intact).
        let mut trip_store = TripStore::open(args.routes_dir());
        app.set_trips(&trip_store.inputs());
        // Load the simulator tracks folder so the Rides screen (#454) lists its v3 fixtures and
        // process-local synced flags.
        let mut ride_store = RideStore::open(args.tracks_dir());
        app.set_rides(ride_store.catalog(), ride_store.ids());
        // Inject BLE before the script; `+` preserves independent link, bond and passkey facts.
        let ble = args.ble.unwrap_or_default();
        app.set_ble_status(obc_app::BleStatus {
            link: if ble.connected { obc_app::BleLink::Connected } else { obc_app::BleLink::Advertising },
            passkey: ble.passkey,
            paired: ble.paired,
        });
        // The map's terrain (EL7), mounted **once** for the whole headless run like the map itself:
        // the `.obcd` sidecar beside the `.obcm`, or the null source when there is none. Two
        // consumers share it — a scripted route plan fills its elevation from it (EL7), and the
        // replay below feeds the map-referenced altimeter from it (EL8) — so it is mounted here,
        // above both, rather than once per user (mounting borrows the whole file for the session).
        let mut elev = map.elevation();
        // The open ride log. Opened here, above the script, because every settling pass reconciles
        // it against the app's tracking session exactly as a frame loop does.
        let mut tracks = TrackStore::open(args.tracks_dir());
        // The resident active-route parse, shared by every settle and by the final render.
        let mut session = ActiveRouteSession::new();
        // What only this host can do: the fixed card-free figure, and the `--dfu` answers.
        //
        // Staged **here**, above the script, because `DfuState` asks exactly once: the "Checking
        // card…" wait the script leaves on top emits one `DfuEffect::Scan`, and an executor with no
        // answer ready would consume that operation and leave the flow parked. `Progress`
        // deliberately stages no install answer — that unanswered arm *is* the spinner.
        let mut platform = HeadlessPlatform::default();
        if let Some(dfu) = &args.dfu {
            platform.scan = match dfu {
                DfuSeed::Scan(kind) | DfuSeed::Progress(kind) | DfuSeed::Installing(kind) => Some(kind.report()),
                DfuSeed::Error(e) => Some(Err(*e)),
                _ => None,
            };
            platform.install = matches!(dfu, DfuSeed::Installing(_)).then_some(Ok(()));
        }
        // `--hold nav` / `--inject nav-fail=...` **acquire** the search without starting it, so the
        // planning screen stays up (for its own snapshot) and the injected answer below is a real
        // answer to the operation the rider actually started.
        let hold = PlanHold::new(
            args.hold == Some(Hold::Nav) || matches!(args.inject, Some(Injection::NavFail(_))),
            args.hold == Some(Hold::Detour) || matches!(args.inject, Some(Injection::DetourFail(_))),
        );
        host.set_plan_hold(hold);
        // The script's synthesized button events reach the app through its own input plane, exactly
        // as the board's high-priority plane feeds gestures between passes; a *frame* — the `f`
        // token, the `T` token and the settle after the script — is what runs `App::run_pass`.
        // WX11: a scripted rain-map time-step must clamp against the real frame count, and the rain
        // map's entry must know the product's zoom floor — both derived by the domain at stage 10
        // from the snapshot the host samples. Sampled once here (position: rider fix, else the
        // camera centre — the demo bundles span the map bbox either way) and lent to **every**
        // settle below: a host that stops offering its bundle is a host with no bundle, and the
        // domain collapses the view state to match.
        //
        // Unprojected on purpose: the ride has not been driven yet. The final frame re-samples,
        // projected under `--weather-decide`.
        let wx_script = weather.as_mut().and_then(|w| {
            let pos = app.state.user_fix.map(|f| (f.lat, f.lon)).unwrap_or((app.state.cam_lat, app.state.cam_lon));
            w.sync_clock(app.wall_unix_now() as i64, false);
            w.snapshot(Some(pos), None)
        });
        let mut script_now = 100u32;
        if let Some(script) = &args.script {
            // The `f` token flushes lazy draw-time state (the POI snapshot / detail hours / the
            // Up-ahead corridor snapshot) by settling the device and then drawing one throwaway
            // frame against the map reader and the active route. Settling first is what lets a
            // script walk the whole POI→route flow: the plan's answer swaps the confirm for the
            // overview / failure card, and the frame after it draws what the next token acts on.
            let (rw, rh) = (args.width, args.height);
            // One render scratch for the whole script run, lent to each throwaway frame — the
            // host owns it since #1146, and ~90 KB is not something to re-allocate per token.
            let mut scratch = Box::new(obc_render::RenderScratch::new());
            let mut stores =
                Stores { routes: &mut store, rides: &mut ride_store, trips: &mut trip_store, tracks: &mut tracks };
            let mut hook = |app: &mut App, what: ScriptHook, now: u32| {
                // Both tokens are the same device frame; only `f` also draws. The keyed ride-track
                // fill and the route overview's shape preview are answered inside the executor from
                // the plan's `derived_needs`, so nothing here reaches for them by hand.
                settle(
                    &mut host,
                    &mut session,
                    app,
                    &mut stores,
                    &reader,
                    &mut *elev,
                    &mut platform,
                    wx_script.as_ref(),
                    now,
                );
                if matches!(what, ScriptHook::Render) {
                    // The frame carries the **streamed route** when one is active, exactly as the
                    // GUI's per-frame render does: the Up-ahead timeline's corridor snapshot (epic
                    // #946) is taken in the pre-draw `prepare` pass off that route, so a routeless
                    // throwaway frame would leave it pending and the next gesture would step an
                    // empty list.
                    let src = stores.routes.active_source();
                    let route = match (session.index(), src.as_ref()) {
                        (Some(i), Some(s)) => Some(RouteReader::new(i, s)),
                        _ => None,
                    };
                    let mut fb = Framebuffer::new(rw, rh);
                    let _ = app.render_frame(
                        Some(&mut scratch),
                        &mut fb,
                        &reader,
                        route.as_ref(),
                        rw as f32,
                        rh as f32,
                        color_of,
                    );
                }
            };
            // One settle before the first token. A scripted host runs a pass only at an `f`/`T`
            // token, so without this the device would take its first gestures never having been told
            // anything about itself: no store level (and so no ride can start — a ride needs
            // somewhere to go), and no weather step range for the gestures to clamp against. A real
            // device mounts its card long before a rider touches a button; this is that.
            hook(&mut app, ScriptHook::Tick, script_now);
            script_now = apply_script(&mut app, script, script_now, &mut hook);
        }
        // Everything the script's last press asked for, with no trailing `f`: settle it now so the
        // final render reflects the answer (the create-route commit, the detour plan/commit, the
        // hold-to-delete re-feed, the trip cascade, the open Ride detail's track — all of it).
        let mut stores =
            Stores { routes: &mut store, rides: &mut ride_store, trips: &mut trip_store, tracks: &mut tracks };
        let mut settle_now =
            |app: &mut App, stores: &mut Stores<'_>, host: &mut HostLoop, platform: &mut HeadlessPlatform| {
                settle(host, &mut session, app, stores, &reader, &mut *elev, platform, wx_script.as_ref(), script_now);
            };
        settle_now(&mut app, &mut stores, &mut host, &mut platform);

        // ── The scripted injections ──────────────────────────────────────────────────────────
        // Each of these is what the device actually sees: an **outcome** answering the operation
        // the rider started (carrying the token the executor is holding for it), or an external
        // **fact** nobody asked for. The pass consumes them at its first two stages, so the snapshot
        // pins the same seam the board runs.
        //
        // A routing failure lands in the CREATE ROUTE confirm's own planning screen: `--hold nav`
        // acquired the operation without starting the search, so this is a genuine answer to it.
        if let Some(Injection::NavFail(kind)) = args.inject {
            let error = obc_app::navigator::NavigatorError::Plan(nav_error(kind));
            if let Some(token) = host.plan_token() {
                let _ =
                    host.outcomes().navigator.try_put(obc_app::navigator::NavigatorOutcome::Failed { token, error });
            } else {
                eprintln!("warning: --inject nav-fail= has no planning operation to answer");
            }
            settle(
                &mut host,
                &mut session,
                &mut app,
                &mut stores,
                &reader,
                &mut *elev,
                &mut platform,
                wx_script.as_ref(),
                script_now,
            );
        }

        // The detour twin: the same failure, against the detour search the script left running.
        if let Some(Injection::DetourFail(kind)) = args.inject {
            let error = obc_app::navigator::NavigatorError::Plan(nav_error(kind));
            if let Some(token) = host.plan_token() {
                let _ =
                    host.outcomes().navigator.try_put(obc_app::navigator::NavigatorOutcome::Failed { token, error });
            } else {
                eprintln!("warning: --inject detour-fail= has no detour operation to answer");
            }
            settle(
                &mut host,
                &mut session,
                &mut app,
                &mut stores,
                &reader,
                &mut *elev,
                &mut platform,
                wx_script.as_ref(),
                script_now,
            );
        }

        // A committed route upload (epic #447, P4): the catalog above is the "already rescanned"
        // store, and this is the fact that names the committed id — the device's exact order. The
        // route's mini elevation band is built from the committed OBCR at "commit time", exactly the
        // seam the board fills (#682); the idle card draws it.
        if let Some(Injection::Upload { id, replaced }) = args.inject {
            let elevation = stores.routes.elevation_sparkline(id);
            host.facts().note_route_upload(obc_app::device_core::RouteUpload { id, replaced, elevation });
            settle(
                &mut host,
                &mut session,
                &mut app,
                &mut stores,
                &reader,
                &mut *elev,
                &mut platform,
                wx_script.as_ref(),
                script_now,
            );
        }
        // The **trip** twin (epic #526): a trip always lands after its member routes, so the one
        // "TRIP RECEIVED" card replaces the burst's last per-route popup.
        //
        // `N` names the file `TP{N}.OBT`, which the trip store lists under `TRIP_ID_BASE + N`:
        // routes and trips are numbered from unrelated counters in these folders, so the store
        // carves the trips into their own band and the fact must name the identity the catalog
        // holds.
        if let Some(Injection::TripUpload { id }) = args.inject {
            host.facts().note_trip_upload(obc_app::device_core::TripUpload { id, replaced: false });
            settle(
                &mut host,
                &mut session,
                &mut app,
                &mut stores,
                &reader,
                &mut *elev,
                &mut platform,
                wx_script.as_ref(),
                script_now,
            );
        }
        // The board's live map-transfer state (issue #927) — a level the ride loop polls each pass,
        // and a feeder rather than a fact until the flat engine's `busy` is wired (S6b).
        if let Some(Injection::MapTransfer(state)) = args.inject {
            app.set_map_transfer(Some(state));
        }
        // Device warnings (issue #504): the sim has no I²C probe / fragmented card to trip them for
        // real, so they arrive as the fact the board raises.
        if let Some(Injection::Warning(w)) = args.inject {
            host.facts().raise_warnings(w);
            settle(
                &mut host,
                &mut session,
                &mut app,
                &mut stores,
                &reader,
                &mut *elev,
                &mut platform,
                wx_script.as_ref(),
                script_now,
            );
        }

        // DFU sideload snapshots (epic #615 S5, #620). The scan ran board-side above, answered by
        // the staged platform when the "Checking card…" wait asked for it — so the confirm /
        // progress / error cards render off the same app state the device reaches, behind the same
        // operation token. What is left is the rider's own Confirm.
        if let Some(dfu) = &args.dfu {
            if matches!(dfu, DfuSeed::Progress(_) | DfuSeed::Installing(_)) {
                // Confirm (Install is the default selection) → the arm request. A tap: down, then up
                // 80 ms later, well under the long-press threshold.
                //
                // Long after any script (~110 ms a token), but taken as a *floor* on the script's
                // own clock rather than as an absolute: the UI clock must never move backwards, and
                // a script long enough to pass this mark would otherwise re-open every bounded
                // window it had already closed.
                let now = script_now.max(500_000);
                feed(&mut app, now, vec![InputEvent::Button(ButtonEvent::Down(Button::Select))]);
                feed(&mut app, now + 80, vec![InputEvent::Button(ButtonEvent::Up(Button::Select))]);
                settle(
                    &mut host,
                    &mut session,
                    &mut app,
                    &mut stores,
                    &reader,
                    &mut *elev,
                    &mut platform,
                    wx_script.as_ref(),
                    now + 80,
                );
            }
        }
        // The one-time post-update toast and its failure twin: this boot's update result is a fact,
        // and the pass's card scheduler pushes the "Updated to vX" / "UPDATE FAILED" card from it.
        let boot_update = match &args.dfu {
            Some(DfuSeed::Confirmed(version)) => {
                Some(obc_app::device_core::UpdateResult::Confirmed(obc_app::dfu::clamp(version)))
            }
            Some(DfuSeed::Failed(why, staged)) => Some(obc_app::device_core::UpdateResult::Failed {
                why: *why,
                staged: staged.as_deref().map(obc_app::dfu::clamp),
            }),
            _ => None,
        };
        if let Some(result) = boot_update {
            let _ = host.facts().note_update_result(result);
            let now = script_now.max(500_000);
            settle(
                &mut host,
                &mut session,
                &mut app,
                &mut stores,
                &reader,
                &mut *elev,
                &mut platform,
                wx_script.as_ref(),
                now,
            );
        }

        // `--sensors screen` (SE7, epic #707): after the script lands on the Sensors screen (or its
        // scan list), push the per-slot status + the canned scan-hit set — the fake central manager,
        // so the three-row screen reads Connected · 78 % / Searching / Not set and the scan list shows
        // filtered hits. The row screen ignores the hits; the scan-list screen ignores the status.
        if args.sensors == Some(SensorSeed::Screen) {
            let status = [
                obc_app::SensorStatus { phase: obc_app::SensorPhase::Connected, battery: Some(78), last_value_ms: 0 },
                obc_app::SensorStatus { phase: obc_app::SensorPhase::Searching, battery: None, last_value_ms: 0 },
                obc_app::SensorStatus::default(),
            ];
            app.set_sensor_status(&status);
            app.set_sensor_scan_hits(&fake_scan_hits());
        }

        // Replay the track from the start up to `--at`, one **device frame** per step so the
        // map-matcher locks on and the ride accumulators + breadcrumb fill. A coarse-but-bounded
        // step keeps long tracks fast while staying under the dropout/teleport gates. The UI clock
        // stands still at the script's own mark: a replay drives the *ride*, and aging the UI on top
        // of it would run every card and idle timer through the whole track in one go.
        if let Some(p) = player.as_mut() {
            let mut baro = BaroSensor::new();
            p.seek(0.0);
            p.play();
            let step = (replay_to / 400.0).clamp(1.0, 8.0);
            let mut t = 0.0;
            while t < replay_to {
                session.sync(&app, stores.routes);
                let mut plan = {
                    let src = stores.routes.active_source();
                    let route = match (session.index(), src.as_ref()) {
                        (Some(i), Some(s)) => Some(RouteReader::new(i, s)),
                        _ => None,
                    };
                    let (ride, sensors) =
                        replay_advance(p, &mut baro, None, step, stores.tracks.sink(), ReplaySensors::default());
                    host.pass(
                        &mut app,
                        obc_app::device_core::PassClock { ride, ui: InputClock(script_now) },
                        &[],
                        sensors,
                        route.as_ref(),
                        None,
                        gui::SIM_SUPPORT,
                    )
                };
                host.execute(
                    &mut app,
                    &mut plan,
                    &mut session,
                    stores.routes,
                    stores.rides,
                    stores.tracks,
                    stores.trips,
                    &reader,
                    &mut *elev,
                    &mut platform,
                );
                // The map-referenced altimeter's one terrain read per fix (EL8, #1076) — the same
                // mounted `.obcd` the router emits from, drained right behind the pass exactly as
                // the board's ride loop does.
                app.sample_terrain(&mut *elev);
                t += step;
            }
        }

        // `--sensors demo` (epic #707, SE5): one final frame fed a **fixed synthetic** HR/power/
        // cadence through SE2's HAL sensor traits, so the three new stat tiles render live values in
        // the Statistics-grid snapshot (the grid was pinned to HR/PWR/RPM in the settings seed
        // above). Stamped at the replay's own `now_ms` so `Activity`'s 5 s staleness gate reads them
        // fresh. Deliberately minimal — SE8 replaces this with the sim control panel's real sliders.
        if args.sensors == Some(SensorSeed::Demo) {
            if let Some(p) = player.as_mut() {
                struct DemoHr;
                impl obc_ports::HeartRateSource for DemoHr {
                    fn poll(&mut self) -> Option<u16> {
                        Some(152)
                    }
                }
                struct DemoPower;
                impl obc_ports::PowerSource for DemoPower {
                    fn poll(&mut self) -> Option<u16> {
                        Some(210)
                    }
                }
                struct DemoCadence;
                impl obc_ports::CadenceSource for DemoCadence {
                    fn poll(&mut self) -> Option<u8> {
                        Some(88)
                    }
                }
                session.sync(&app, stores.routes);
                let ride = obc_ports::RideClock((p.time() * 1000.0) as u32);
                let mut plan = {
                    let src = stores.routes.active_source();
                    let route = match (session.index(), src.as_ref()) {
                        (Some(i), Some(s)) => Some(RouteReader::new(i, s)),
                        _ => None,
                    };
                    let (mut hr, mut power, mut cadence) = (DemoHr, DemoPower, DemoCadence);
                    let sensors = obc_ports::Sensors {
                        hr: Some(&mut hr),
                        power: Some(&mut power),
                        cadence: Some(&mut cadence),
                        ..obc_ports::Sensors::new(p)
                    };
                    host.pass(
                        &mut app,
                        obc_app::device_core::PassClock { ride, ui: InputClock(script_now) },
                        &[],
                        sensors,
                        route.as_ref(),
                        None,
                        gui::SIM_SUPPORT,
                    )
                };
                host.execute(
                    &mut app,
                    &mut plan,
                    &mut session,
                    stores.routes,
                    stores.rides,
                    stores.tracks,
                    stores.trips,
                    &reader,
                    &mut *elev,
                    &mut platform,
                );
            }
        }

        // The final frame's geometry: whatever the script, the injections and the replay left
        // active, re-opened from the resident session.
        session.sync(&app, stores.routes);
        let route_src = stores.routes.active_source();
        let route = match (session.index(), route_src.as_ref()) {
            (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
            _ => None,
        };

        // `--freeze` (#1146 P2): engage the Recalculating freeze through the same seam a drained
        // plan command takes, so the snapshot shows the real banner over the real frozen map.
        if args.freeze {
            app.debug_set_plan_live(true);
        }

        // `--weather-alert` (WX11): push the alert card through the production seam WX12's
        // decision engine will drive on the device.
        if let Some((kind, minutes)) = args.weather_alert {
            app.show_weather_alert(kind, minutes);
        }

        let mut fb = Framebuffer::new(args.width, args.height);
        // Time the whole frame draw into `render_us` (the no_std renderer has no clock, so
        // the host fills it) — same field the live panel shows.
        let t0 = Instant::now();
        let mut scratch = Box::new(obc_render::RenderScratch::new());
        // `--weather` (WX10/WX11): the final frame renders through the production rain lease and
        // the production resident snapshot — the same adapter/feed pair the device and the GUI
        // use. No store / nothing current ⇒ `None`, byte-identical to a rain-free render.
        let wx_pos = app.state.user_fix.map(|f| (f.lat, f.lon)).unwrap_or((app.state.cam_lat, app.state.cam_lon));
        // `--weather-decide` (WX12): the production ride path — frame samples route-projected
        // from the app's own matched progress + recent-pace estimate (`ride_projection` →
        // `sample_along`), then the real alert engine (thresholds/dedup/cooldown) over the very
        // snapshot the screens render. Source-agnostic: whichever bundle is loaded (`demo:`,
        // a fixture file, or a `--weather live` fetch) is what it decides over. The alert engine
        // itself is no longer opt-in: it runs at stage 10 on every host, over whatever bundle is
        // sampled.
        let wx_projection = if args.weather_decide { route.as_ref().zip(app.ride_projection()) } else { None };
        let wx_snapshot = weather.as_mut().and_then(|w| {
            w.sync_clock(app.wall_unix_now() as i64, false);
            w.snapshot(Some(wx_pos), wx_projection)
        });
        // One settle carrying that snapshot: the domain derives the rain map's step range and zoom
        // floor from it and runs the alert decision, both at stage 10 — the same stage the board
        // runs them at. The route opened above ends here (the settle re-opens it) and the frame's
        // own reader is taken again below.
        //
        // **Only for a scenario that mounted weather.** A settle is not free — it advances the
        // card and spinner timers every screen shares — so a still frame that named no bundle keeps
        // exactly the passes it always ran.
        if weather.is_some() {
            // `--weather-refreshing`: the provider plane's level, reported as the external fact the
            // domain reads. The cue is the domain's answer on every host now, never a render argument.
            if args.weather_refreshing {
                host.facts().note_weather_refreshing(true);
            }
            settle(
                &mut host,
                &mut session,
                &mut app,
                &mut stores,
                &reader,
                &mut *elev,
                &mut platform,
                wx_snapshot.as_ref(),
                script_now,
            );
        }
        session.sync(&app, stores.routes);
        let route_src = stores.routes.active_source();
        let route = match (session.index(), route_src.as_ref()) {
            (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
            _ => None,
        };
        let scene = map_file::Scene { reader: &reader, route: route.as_ref() };

        // `--expect-screen`: the recipe states where its gestures were supposed to land, and the
        // sim checks it against the `screens!` table's own name before a single pixel is written.
        // Checked here — below every seam that can still change the top screen, including WX12's
        // alert decision — so what is verified is exactly what gets saved.
        if let Some(expected) = &args.expect_screen {
            let landed = app.top_screen().name();
            if landed != expected {
                eprintln!("error: --expect-screen {expected}, but the script landed on {landed}");
                std::process::exit(1);
            }
        }

        let rain_step = app.state.rain_step;
        let render_final = |rain: Option<&mut dyn obc_render::RainOverlaySource>,
                            weather_feed: Option<&obc_app::WeatherSnapshot>,
                            app: &mut App,
                            fb: &mut Framebuffer,
                            scratch: &mut obc_render::RenderScratch| {
            map_file::render_frame(
                app,
                scratch,
                fb,
                scene,
                rain,
                weather_feed,
                (args.width as f32, args.height as f32),
                color_of,
            )
        };
        let wx_wall_now = app.wall_unix_now() as i64;
        let mut stats = match weather.as_mut() {
            Some(weather) => weather.lease(wx_wall_now, rain_step, |rain| {
                render_final(rain, wx_snapshot.as_ref(), &mut app, &mut fb, &mut scratch)
            }),
            None => render_final(None, wx_snapshot.as_ref(), &mut app, &mut fb, &mut scratch),
        };
        stats.render_us = t0.elapsed().as_micros() as u32;
        let cache_reqs = stats.map_chunk_hits + stats.map_chunk_misses;
        let hit_pct = if cache_reqs == 0 { 0.0 } else { 100.0 * stats.map_chunk_hits as f32 / cache_reqs as f32 };
        eprintln!(
            "rendered {}/{} features ({} chunks, LOD {}, {} dropped) | route {}/{} drawn, {} chunks in {:.2} ms | spans {:.0}% points {:.0}% rings {:.0}% | map-cache {:.0}% hit, {} reads, {} B",
            stats.features_drawn,
            stats.features_tried,
            stats.chunks_visited,
            stats.lod,
            stats.features_dropped,
            stats.route_points_drawn,
            stats.route_points,
            stats.route_chunks,
            stats.render_us as f64 / 1000.0,
            stats.span_utilization * 100.0,
            stats.point_utilization * 100.0,
            stats.ring_utilization * 100.0,
            hit_pct,
            stats.map_sd_reads,
            stats.map_bytes_read
        );
        // The span/point/ring scratch split by render path (lines vs polygons).
        eprintln!(
            "  scratch by kind: spans {}L+{}P/{} · points {}L+{}P/{} · rings {}L+{}P/{}",
            stats.line_spans,
            stats.poly_spans,
            obc_render::MAX_SPANS,
            stats.line_points,
            stats.poly_points,
            obc_render::MAX_FRAME_POINTS,
            stats.line_rings,
            stats.poly_rings,
            obc_render::MAX_FRAME_RINGS,
        );
        // Rain overlay accounting (WX10), for the look-tuning rounds: how many 16×16 tiles were
        // decoded, how many pixels the dither actually painted, and the overlay's own wall time.
        if stats.rain_out_of_regime {
            eprintln!(
                "  rain: OUT OF REGIME — overlay suppressed (cells would drop below ~3 px; see RAIN_MAX_CELL_STEP)"
            );
        } else if stats.rain_tiles > 0 || stats.rain_px > 0 {
            eprintln!(
                "  rain: {} tiles decoded, {} px painted, {:.2} ms overlay",
                stats.rain_tiles,
                stats.rain_px,
                stats.rain_us as f64 / 1000.0
            );
        }

        if let Err(e) = write_png(&fb, args.scale, path) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        eprintln!("wrote {path}");
        return;
    }

    // Interactive: hand the map to the eframe host window.
    if let Err(e) = gui::run(map, args) {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    fn parse(options: &[&str]) -> Result<Args, String> {
        let mut args = vec!["map.obcm".to_string()];
        args.extend(options.iter().map(|s| (*s).to_string()));
        parse_args_from(args)
    }

    #[test]
    fn grouped_flags_parse_typed_states() {
        assert_eq!(parse(&["--ble", "passkey=42"]).unwrap().ble.unwrap().passkey, Some(42));
        let linked_bond = parse(&["--ble", "connected+paired"]).unwrap().ble.unwrap();
        assert!(linked_bond.connected);
        assert!(linked_bond.paired);
        assert!(matches!(
            parse(&["--inject", "upload-replace=7"]).unwrap().inject,
            Some(Injection::Upload { id: 7, replaced: true })
        ));
        assert!(matches!(
            parse(&["--dfu", "installing=normal"]).unwrap().dfu,
            Some(DfuSeed::Installing(dfu::DfuScanKind::Normal))
        ));
        let trip = obc_host_core::TRIP_ID_BASE + 5;
        assert!(
            matches!(parse(&["--inject", "trip-upload=5"]).unwrap().inject, Some(Injection::TripUpload { id }) if id == trip)
        );
        assert!(parse(&["--inject", "trip-upload=18446744073709551615"]).is_err(), "past the trip band, refused");
        assert!(matches!(
            parse(&["--inject", "map-transfer=receiving:100000/400000"]).unwrap().inject,
            Some(Injection::MapTransfer(obc_app::screen::MapTransfer::Receiving {
                received_kib: 100_000,
                total_kib: 400_000
            }))
        ));
        assert!(matches!(
            parse(&["--inject", "map-transfer=installed"]).unwrap().inject,
            Some(Injection::MapTransfer(obc_app::screen::MapTransfer::Installed))
        ));
        assert!(matches!(
            parse(&["--inject", "map-transfer=failed:notamap"]).unwrap().inject,
            Some(Injection::MapTransfer(obc_app::screen::MapTransfer::Failed(
                obc_app::screen::MapTransferError::NotAMap
            )))
        ));
        assert!(matches!(
            parse(&["--dfu", "failed=reverted:v1.2.3"]).unwrap().dfu,
            Some(DfuSeed::Failed(obc_app::DfuFailure::Reverted, Some(v))) if v == "v1.2.3"
        ));
        assert!(matches!(
            parse(&["--dfu", "failed=notstarted"]).unwrap().dfu,
            Some(DfuSeed::Failed(obc_app::DfuFailure::NotStarted, None))
        ));
    }

    /// The new seed forms refuse the values that would silently snapshot a *different* frame:
    /// a progress bar past full, a zero-length transfer, and an unknown failure reason.
    #[test]
    fn seed_forms_reject_states_the_device_cannot_reach() {
        assert!(parse(&["--inject", "map-transfer=receiving:500/400"]).is_err());
        assert!(parse(&["--inject", "map-transfer=receiving:0/0"]).is_err());
        assert!(parse(&["--inject", "map-transfer=aborted"]).is_err());
        assert!(parse(&["--inject", "map-transfer=failed:melted"]).is_err());
        assert!(parse(&["--inject", "trip-upload=nope"]).is_err());
        assert!(parse(&["--dfu", "failed=exploded"]).is_err());
        let weather = parse(&["--weather-fault", "fail-from=3:503"]).unwrap();
        assert_eq!(weather.live.controls.fail_from, Some((3, 503)));
        let faults = parse(&["--weather-fault", "latency=10", "--weather-fault", "fail-from=2:503"]).unwrap();
        assert_eq!(faults.live.controls.latency, std::time::Duration::from_millis(10));
        assert_eq!(faults.live.controls.fail_from, Some((2, 503)));
    }

    #[test]
    fn removed_flags_are_rejected() {
        for flag in [
            "--true-color",
            "--colorway",
            "--calibrate",
            "--screenshot",
            "--boot-fault",
            "--open-climb",
            "--baro-drift",
            "--set",
        ] {
            assert!(parse(&[flag]).is_err(), "{flag} must stay removed");
        }
    }

    #[test]
    fn help_lists_every_parser_flag_and_no_removed_flag() {
        let readme = include_str!("../README.md");
        for flag in [
            "--size",
            "--scale",
            "--png",
            "--heading",
            "--gpx",
            "--at",
            "--center",
            "--zoom",
            "--script",
            "--expect-screen",
            "--boot",
            "--routes-dir",
            "--tracks-dir",
            "--import",
            "--physical",
            "--palette",
            "--battery",
            "--clock",
            "--weather",
            "--weather-now",
            "--weather-refreshing",
            "--weather-service",
            "--weather-radius-km",
            "--no-card",
            "--weather-offline",
            "--weather-fault",
            "--weather-alert",
            "--weather-decide",
            "--route-retention",
            "--lang",
            "--stat-fields",
            "--ble",
            "--hold",
            "--freeze",
            "--sensors",
            "--dfu",
            "--inject",
        ] {
            assert!(HELP.contains(flag), "help is missing {flag}");
            assert!(readme.contains(flag), "README is missing {flag}");
        }
        for removed in ["--true-color", "--colorway", "--calibrate", "--screenshot", "--boot-fault", "--set"] {
            assert!(!HELP.contains(removed), "help still advertises {removed}");
            assert!(!readme.contains(removed), "README still advertises {removed}");
        }
        // A grouped flag's *forms* are the actual vocabulary a snapshot recipe writes, so they are
        // documented in both places too — a seed nobody can find is a seed nobody uses.
        for form in [
            "trip-upload=N",
            "map-transfer=receiving:RECEIVED/TOTAL",
            "map-transfer=installed",
            "map-transfer=failed:KIND",
            "failed=WHY[:VERSION]",
        ] {
            assert!(HELP.contains(form), "help is missing {form}");
            assert!(readme.contains(form), "README is missing {form}");
        }
    }
}
