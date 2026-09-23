//! OBC desktop simulator: the host shell around the shared renderer.
//!
//! All map drawing lives in `obc_render`, the same code the firmware runs. This binary owns only
//! the host concerns: argument parsing, the eframe window and its pan/zoom event loop, PNG output,
//! and the device's 64-color display policy.
//!
//! Host logic shared with the landing page's wasm host lives in `obc-host-core`.

use obc_app::{App, AppState};
use obc_host_core::frame::device_rgb888;
use obc_ports::{Button, ButtonEvent, Fix, InputClock, InputEvent, InputSource, LocationSource};

mod calib;
mod card;
mod dfu;
mod diagnostics;
mod framebuffer;
mod gui;
mod headless_script;
mod map_file;
mod palette;
mod panel_power;
mod peak_view;
mod present;
mod rides;
mod routes;
mod sim_compass;
mod sim_location;
mod sim_sensors;
mod trips;

use framebuffer::Framebuffer;
use obc_host_core::{
    initial_camera, replay_advance, ActiveRouteSession, HostLoop, HostPlatform, PlanHold, ReplaySensors, TrackStore,
};
use obc_host_core::{FlatRideStore as RideStore, RideRepository};
use obc_host_core::{FlatRouteStore as RouteStore, FlatTripStore as TripStore, RouteRepository};
use obc_replay::{gpx::Track, BaroSensor, GpxPlayer};
use obc_route::RouteReader;

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
    /// The boot-outcome failure verdict and the version that was staged. `None` when the board
    /// could not name one.
    Failed(obc_app::DfuFailure, Option<String>),
}

struct Args {
    diagnostics: Option<String>,
    map: String,
    width: u32,
    height: u32,
    scale: u32,
    png: Option<String>,
    /// Start in heading-up orientation with this course (degrees CW from north).
    heading: Option<f32>,
    /// Explicit geographic fixture input for Peak View acceptance tests.
    peak_view: Option<peak_view::Preset>,
    /// Preload this GPX track for replay.
    gpx: Option<String>,
    /// With `--gpx --png`, the playback time in seconds to render the fix at. Defaults to the
    /// track midpoint.
    at: Option<f64>,
    /// GPX position for the button script; replay then continues from here to `at`.
    script_at: Option<f64>,
    /// Headless camera center "lon,lat" in microdegrees. Defaults to the bbox center.
    center: Option<(i32, i32)>,
    /// Headless zoom multiplier applied to the bbox-fit zoom (picks a finer LOD).
    zoom_mul: f32,
    /// A gesture script applied before a headless `--png` render, to snapshot a specific screen.
    /// One character a token, spaces ignored: `d` and `u` step Down and Up, `p` presses Select, `h`
    /// holds Select, `b` is back, `B` is back-hold, `H` and `M` leave Select and Back held partway
    /// to snapshot the long-press hint, `w` waits for an in-flight animation to settle, `f` draws
    /// one throwaway frame so draw-time lazy state fills, `T` runs one route-aware tick, `Q` opens
    /// the quick drawer with its slide settled, `A` holds Up and Select to open Assistant, and `I`
    /// elapses 5 minutes with no input so the idle-return timeout fires.
    script: Option<String>,
    /// Normal button input after GPX replay, before the final render.
    script_after: Option<String>,
    /// Model a platform whose panel has no controllable light. The quick drawer then draws three
    /// controls instead of four; nothing else changes.
    no_backlight: bool,
    /// Headless `--png` only: refuse to render unless the script landed on that screen, named by
    /// the `screens!` table's own variant string. A recipe that walks a menu depends on that menu's
    /// station order, so one inserted row would otherwise snapshot a different screen under the old
    /// filename.
    expect_screen: Option<String>,
    /// Headless `--png` only: render from the device's real power-on state (Home / Idle,
    /// no route) instead of straight from the map.
    boot: bool,
    /// One-time route/trip fixture import directory; defaults to `routes/`.
    routes_dir: Option<String>,
    /// A progress record for the first imported trip: `(day, metres, last finished day)`.
    pub(crate) trip_progress: Option<(u16, u32, Option<u16>)>,
    card: Option<String>,
    create_card: Option<String>,
    /// Folder for saved `.gpx` tracks + the in-progress `.obct` log; defaults to `tracks/`.
    tracks_dir: Option<String>,
    /// Convert this GPX into the routes folder and exit. Needs no map.
    import: Option<String>,
    /// Render the device window at the panel's true physical size (needs a saved
    /// calibration). Falls back to the scaled view if uncalibrated.
    physical: bool,
    /// Show the device's 64-color gamut and nothing else. Needs no map.
    palette: bool,
    /// Initial battery charge shown on the Home gauge, from 0 to 100. Defaults to full.
    battery: Option<u8>,
    /// Explicit UTC time and local offset, through the trusted clock entry.
    clock: Option<obc_ports::DateTime>,
    /// Apply another explicit time after the button script, before the final settle and render.
    clock_after_script: Option<obc_ports::DateTime>,
    utc_offset_min: Option<i16>,

    no_card: bool,
    route_cleanup: bool,
    /// Headless `--png` only: the UI language, one of `en`, `de`, `fr` or `es`. Seeded into
    /// `Settings.language` before the render, so a scripted screen draws its copy from the i18n
    /// catalog. Defaults to `en`.
    lang: Option<obc_app::settings::Language>,
    /// Headless `--png` only: replace the Statistics grid's field selection with this
    /// comma-separated list of the catalogue's kebab-case ids. An unknown name fails with the full
    /// list. It stands in for walking the Fields editor with a long script just to place a tile.
    stat_fields: Option<std::vec::Vec<obc_app::StatField>>,
    /// One typed BLE fixture state for headless snapshots; `+` composes independent link facts.
    ble: Option<BleSeed>,
    /// One mutually-exclusive sensor fixture for headless snapshots.
    sensors: Option<SensorSeed>,
    /// Consume one host request without starting it so its planning spinner stays visible.
    hold: Option<Hold>,
    /// One mutually-exclusive host event injection for headless snapshots.
    inject: Option<Injection>,
    /// Headless `--png` only: engage the Recalculating freeze after the script, so the
    /// overlay-plane banner renders over whatever map base the script left showing. The freeze is
    /// otherwise unreachable headlessly, because the flows that start a plan leave the opaque
    /// planning spinner as the base.
    freeze: bool,
    /// One mutually-exclusive DFU fixture state for headless snapshots.
    dfu: Option<DfuSeed>,
}

impl Default for Args {
    /// Device resolution with all knobs off: the CLI parser's base. The resolution comes from the
    /// one [`obc_display`] frame authority; `--size` overrides it for off-device experiments.
    fn default() -> Self {
        Args {
            diagnostics: None,
            map: String::new(),
            width: obc_display::ls021::FRAME_W as u32,
            height: obc_display::ls021::FRAME_H as u32,
            scale: 1,
            png: None,
            heading: None,
            peak_view: None,
            gpx: None,
            at: None,
            script_at: None,
            center: None,
            zoom_mul: 1.0,
            script: None,
            script_after: None,
            no_backlight: false,
            expect_screen: None,
            boot: false,
            routes_dir: None,
            trip_progress: None,
            card: None,
            create_card: None,
            tracks_dir: None,
            import: None,
            physical: false,
            palette: false,
            battery: None,
            clock: None,
            clock_after_script: None,
            utc_offset_min: None,

            no_card: false,
            route_cleanup: false,
            lang: None,
            stat_fields: None,
            ble: None,
            sensors: None,
            hold: None,
            inject: None,
            freeze: false,
            dfu: None,
        }
    }
}

impl Args {
    fn replay_range(&self, duration: f64) -> Result<(f64, f64, f64), String> {
        let end = self.at.unwrap_or(duration / 2.0);
        let Some(start) = self.script_at else { return Ok((end, 0.0, end)) };
        if !start.is_finite() || !end.is_finite() || start < 0.0 || end < start || end > duration {
            return Err("--script-at requires 0 <= script start <= --at <= GPX duration".into());
        }
        Ok((start, start, end))
    }

    fn stamp_initial_clock(&self, app: &mut obc_app::App) {
        if let Some(clock) = self.clock {
            app.stamp_clock(clock, 0, Some(self.utc_offset_min.unwrap_or(0)), obc_app::ClockTrust::Ble);
        }
    }

    pub(crate) fn routes_dir(&self) -> String {
        self.routes_dir.clone().unwrap_or_else(|| "routes".to_string())
    }

    pub(crate) fn tracks_dir(&self) -> String {
        self.tracks_dir.clone().unwrap_or_else(|| "tracks".to_string())
    }

    /// The persisted-settings file, standing in for the device's RRAM. It holds the shared
    /// [`obc_app::settings`] blob, so relaunching restores units, clock and intervals.
    pub(crate) fn settings_path(&self) -> String {
        "obc-settings.bin".to_string()
    }
}

/// Parse a `--clock` value `YYYY-MM-DDTHH:MM` into an [`obc_ports::DateTime`]. An out-of-range
/// field is clamped when seeded, but the format itself must be well-formed.
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
/// Parse a `--lang` value into a [`Language`](obc_app::settings::Language). Anything but the four
/// codes the catalog ships is an error, so a typo in a snapshot script fails loudly.
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

/// The catalogue's kebab-case field ids, in catalogue order: the `--stat-fields` vocabulary. An
/// added [`StatField`](obc_app::StatField) shows up as a missing arm here.
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
        F::TripToGo => "trip-to-go",
        F::TripDay => "trip-day",
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

/// An empty [`StatFieldList`](obc_app::StatFieldList), which every `--stat-fields` list is built
/// onto: the type starts at the device's default six and only shrinks by index.
fn empty_stat_field_list() -> obc_app::StatFieldList {
    let mut sf = obc_app::StatFieldList::default();
    while !sf.is_empty() {
        sf.remove(0);
    }
    sf
}

/// Parse a `--stat-fields` list into the grid selection. An unknown name fails with the whole
/// vocabulary listed.
///
/// Every name is also pushed onto a real [`StatFieldList`](obc_app::StatFieldList) as it is parsed,
/// so a list the grid cannot hold fails here instead of truncating into a snapshot.
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
            "storage" => obc_app::WarningFlags::STORAGE_ERROR,
            "rec" | "record" => obc_app::WarningFlags::REC_ERROR,
            _ => return Err("--inject warning tokens: gps|altimeter|compass|storage|rec".into()),
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

/// The `--inject` vocabulary, stated once: the parser's error text and the `--help` line both read
/// it.
const INJECT_FORMS: &str = "--inject needs nav-fail=KIND|detour-fail=KIND|upload=ID|upload-replace=ID|\
     trip-upload=N|map-transfer=receiving:RECEIVED/TOTAL|map-transfer=installed|map-transfer=failed:KIND|\
     warning=LIST";

/// The `--inject map-transfer` forms, stated once.
const MAP_TRANSFER_FORMS: &str =
    "--inject map-transfer needs receiving:RECEIVED/TOTAL|installed|failed:storage|damaged|notamap|refused";

/// Parse a `--inject map-transfer` value into the board's live transfer state:
/// `receiving:RECEIVED/TOTAL` in kibibytes, the terminal `installed`, and each `failed:KIND` face.
///
/// There is no form for an abort or an unplug. Those clear the card rather than raise one, so they
/// look like `None` at the seam and there is no frame to shoot.
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
            // The band here, not at the use site: a trip file the store cannot carry has no catalog
            // identity to announce, and saturating into one would name a trip no scan can list.
            let id =
                n.checked_add(obc_host_core::TRIP_ID_BASE).ok_or("--inject trip-upload N is past the trip id band")?;
            Ok(Injection::TripUpload { id })
        }
        "map-transfer" => Ok(Injection::MapTransfer(parse_map_transfer(value)?)),
        "warning" => Ok(Injection::Warning(parse_warning(value)?)),
        _ => Err(INJECT_FORMS.into()),
    }
}

/// The `--dfu` vocabulary, stated once.
const DFU_FORMS: &str =
    "--dfu needs scan=KIND|progress=KIND|installing=KIND|error=ERR|confirmed=VERSION|failed=WHY[:VERSION]";

/// Parse a `--dfu failed=WHY[:VERSION]` value into the boot-outcome verdict the update-failed card
/// carries: why the armed update is not what is running, and the version that was staged.
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
            "--peak-view" => {
                a.peak_view = Some(peak_view::Preset::parse(&it.next().ok_or("--peak-view needs a preset")?)?);
            }
            "--gpx" => a.gpx = Some(it.next().ok_or("--gpx needs a path")?),
            "--at" => a.at = Some(it.next().and_then(|s| s.parse().ok()).ok_or("bad --at")?),
            "--script-at" => a.script_at = Some(it.next().and_then(|s| s.parse().ok()).ok_or("bad --script-at")?),
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
            "--diagnostics" => a.diagnostics = Some(it.next().ok_or("--diagnostics needs a new JSONL path")?),
            "--script" => a.script = Some(it.next().ok_or("--script needs a token string")?),
            "--script-after" => a.script_after = Some(it.next().ok_or("--script-after needs a token string")?),
            "--expect-screen" => a.expect_screen = Some(it.next().ok_or("--expect-screen needs a screen name")?),
            "--boot" => a.boot = true,
            "--card" => a.card = Some(it.next().ok_or("--card needs a path")?),
            "--create-card" => a.create_card = Some(it.next().ok_or("--create-card needs a path")?),
            "--routes-dir" => a.routes_dir = Some(it.next().ok_or("--routes-dir needs a path")?),
            "--trip-progress" => {
                let value = it.next().ok_or("--trip-progress needs DAY:METRES:LAST")?;
                let mut parts = value.split(':');
                let mut next = || parts.next().ok_or("--trip-progress needs DAY:METRES:LAST");
                let day = next()?.parse().map_err(|_| "bad --trip-progress day")?;
                let metres = next()?.parse().map_err(|_| "bad --trip-progress metres")?;
                let last = match next()? {
                    "-" => None,
                    last => Some(last.parse().map_err(|_| "bad --trip-progress last day")?),
                };
                a.trip_progress = Some((day, metres, last));
            }
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
            "--clock-after-script" => {
                a.clock_after_script =
                    Some(parse_clock(&it.next().ok_or("--clock-after-script needs YYYY-MM-DDTHH:MM")?)?);
            }

            "--utc-offset-min" => {
                let offset = it
                    .next()
                    .ok_or("--utc-offset-min needs minutes")?
                    .parse::<i16>()
                    .map_err(|_| "--utc-offset-min needs an integer number of minutes")?;
                if !(obc_app::settings::UTC_OFFSET_MIN..=obc_app::settings::UTC_OFFSET_MAX).contains(&offset) {
                    return Err("--utc-offset-min is outside the device's supported range".into());
                }
                a.utc_offset_min = Some(offset);
            }

            "--route-cleanup" => a.route_cleanup = true,
            "--no-card" => a.no_card = true,
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
    if a.script_after.is_some() && (a.gpx.is_none() || a.png.is_none()) {
        return Err("--script-after requires --gpx and --png".into());
    }
    if let Some(start) = a.script_at {
        if a.gpx.is_none() || a.png.is_none() || a.script.is_none() {
            return Err("--script-at requires --gpx, --png and --script".into());
        }
        if !start.is_finite() || start < 0.0 || a.at.is_some_and(|end| !end.is_finite() || end < start) {
            return Err("--script-at requires a finite non-negative start no later than --at".into());
        }
    }
    if a.utc_offset_min.is_some() && a.clock.is_none() && a.clock_after_script.is_none() {
        return Err("--utc-offset-min requires --clock or --clock-after-script".into());
    }
    if a.diagnostics.is_some() && (a.png.is_none() || a.palette || a.import.is_some() || a.create_card.is_some()) {
        return Err("--diagnostics requires a normal --png session".into());
    }
    // `--palette` and `--import` need no map file.
    if a.card.is_some() && (a.create_card.is_some() || !a.map.is_empty() || a.routes_dir.is_some()) {
        return Err("--card reopens without map or route-directory imports".into());
    }

    if a.card.is_some() && matches!(a.inject, Some(Injection::TripUpload { .. })) {
        return Err("trip-upload names a TP fixture and requires a map import session".into());
    }
    if a.create_card.is_some() && (a.import.is_some() || a.palette || a.png.is_some()) {
        return Err("--create-card imports its map and routes once, then exits".into());
    }
    if a.map.is_empty() && !a.palette && a.import.is_none() && a.card.is_none() {
        return Err("missing map path (one .obcm file)".into());
    }
    Ok(a)
}

fn headless_replay_advance<'s>(
    player: &'s mut GpxPlayer,
    baro: &'s mut BaroSensor,
    dt: f64,
    from: f64,
) -> (obc_ports::RideClock, obc_ports::Sensors<'s>) {
    let (ride, sensors) = replay_advance(player, baro, None, dt, ReplaySensors::default());
    (obc_ports::RideClock(ride.0.saturating_sub((from * 1000.0) as u32)), sensors)
}

fn parse_args() -> Result<Args, String> {
    parse_args_from(std::env::args().skip(1))
}

/// The fixed free-space figure the sim answers a card scan with, because it has no FAT to scan.
const SIM_CARD_FREE: u64 = 1_288_490_188;

/// A canned scan-hit set for the sim's fake sensor manager: two HR straps, one power meter and one
/// unnamed cadence sensor, so any kind's scan list shows something. The unnamed hit exercises the
/// address fallback, and the second HR hit gives a multi-row list. Shared with the interactive GUI
/// so the two sensor paths cannot drift.
fn fake_scan_hits() -> [obc_app::SensorScanHit; 4] {
    [
        obc_app::SensorScanHit::new(0, 1, [0x66, 0x55, 0x44, 0x33, 0x22, 0x11], "HRM-Dual", -58),
        obc_app::SensorScanHit::new(0, 1, [0x21, 0x43, 0x65, 0x87, 0xA9, 0xCB], "Forerunner", -74),
        obc_app::SensorScanHit::new(1, 0, [0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F], "Stages LR", -67),
        obc_app::SensorScanHit::new(2, 0, [0x01, 0x02, 0x03, 0x04, 0x05, 0x06], "", -80),
    ]
}

/// The headless driver's repositories, threaded as one value: three folder stores plus the open
/// ride log.
struct Stores<'a> {
    routes: &'a mut RouteStore,
    rides: &'a mut RideStore,
    trips: &'a mut TripStore,
    tracks: &'a mut TrackStore,
}

/// What only the headless driver can do: a fixed free-space figure and the update answers `--dfu`
/// stages. Neither durable record persists, because a `--png` run must not write the developer's
/// settings file or their alert anchors, and the defaults acknowledge the writes so the domains
/// settle instead of parking.
#[derive(Default)]
struct HeadlessPlatform {
    /// The `--dfu` scan answer, taken by the first scan the flow asks for.
    scan: Option<Result<obc_app::dfu::DfuScanReport, obc_app::dfu::DfuScanError>>,
    /// The `--dfu` install answer. `None` leaves the arm in flight, which is the progress
    /// spinner.
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

/// The most passes one settle runs. A route plan is stepped once per pass, as on the board, so the
/// ceiling must clear a whole search. It is a runaway guard, not a budget.
const MAX_SETTLE_PASSES: usize = 100_000;

/// Passes with nothing owed before the device counts as settled. Two, because the next pass
/// consumes an outcome the executor produced, so one quiet pass would stop with an answer still in
/// the inbox.
const QUIET_PASSES: usize = 2;

/// Run DeviceCore passes until the device stops asking for anything.
///
/// A scripted host has no display frame to yield between bounded steps, so it settles here: the
/// same `App::run_pass` and typed executor the GUI runs, looped until no effect is owed, no
/// deferred value is in flight and no planner step is left.
///
/// The ride clock stands still at zero and the UI clock is the script's own. A settling pass must
/// not age a ride nobody is riding.
#[allow(clippy::too_many_arguments)]
fn settle(
    host: &mut HostLoop,
    session: &mut ActiveRouteSession,
    app: &mut App,
    stores: &mut Stores<'_>,
    map: &obc_host_core::flat_map::FlatMap,
    elev: &mut dyn obc_route::ElevationSource,
    platform: &mut HeadlessPlatform,

    now: u32,
) {
    settle_at(
        host,
        session,
        app,
        stores,
        map,
        elev,
        platform,
        obc_app::device_core::PassClock { ride: obc_ports::RideClock(0), ui: InputClock(now) },
    );
}

#[allow(clippy::too_many_arguments)]
fn settle_at(
    host: &mut HostLoop,
    session: &mut ActiveRouteSession,
    app: &mut App,
    stores: &mut Stores<'_>,
    map: &obc_host_core::flat_map::FlatMap,
    elev: &mut dyn obc_route::ElevationSource,
    platform: &mut HeadlessPlatform,
    clock: obc_app::device_core::PassClock,
) {
    let mut quiet = 0usize;
    for _ in 0..MAX_SETTLE_PASSES {
        session.sync(app, stores.routes);
        let (mut plan, owed) = {
            let src = stores.routes.active_source();
            let route = match (session.index(), src) {
                (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
                _ => None,
            };
            let fix = None;
            let mut loc = crate::sim_location::SimLocationSource::new(fix);
            let sensors = obc_ports::Sensors::new(&mut loc);
            let plan = host.pass(app, clock, &[], sensors, route.as_ref(), gui::SIM_SUPPORT);
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
            map,
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

/// Encode a framebuffer to a PNG, upscaling by `scale` with nearest-neighbor so the device's hard
/// pixel edges stay crisp.
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

/// A scripted [`InputSource`] that replays a fixed queue of raw events.
struct ScriptInput(std::collections::VecDeque<InputEvent>);
impl InputSource for ScriptInput {
    fn poll(&mut self) -> Option<InputEvent> {
        self.0.pop_front()
    }
}

/// A host-side effect a script token requests from `apply_script`'s hook closure.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScriptHook {
    /// Draw one throwaway frame, for the `f` token.
    Render,
    /// Run one route-aware pass, for the `T` token.
    Tick,
    Before(char),
    After(char),
}

/// Feed one batch of raw events to the app at time `now` (ms).
fn feed(app: &mut App, now: u32, events: Vec<InputEvent>) {
    app.handle_input(InputClock(now), &mut ScriptInput(events.into()));
}

/// Apply a gesture script (see `Args::script`) to `app`. It synthesizes the raw four-button events
/// with a rising clock, including the threshold crossing that turns a held button into a `Hold` or
/// `BackHold`, as the real recognizer would see them.
///
/// `hook` runs the host-side effects a token asks for. [`ScriptHook::Render`] draws one throwaway
/// headless frame, which is how the `f` token flushes lazy draw-time state such as the POI-list
/// snapshot. Without an `f` the whole script runs before the single final render, so lazy state
/// never fills mid-script. [`ScriptHook::Tick`] runs one route-aware tick, which the headless path
/// otherwise never does, so Navigator's route total and derived caches stay unbuilt.
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
    // A tap: down, then up 80 ms later, well under the long-press threshold.
    let tap = |app: &mut App, now: &mut u32, b| {
        feed(app, *now, vec![down(b)]);
        *now += 80;
        feed(app, *now, vec![up(b)]);
        *now += 30;
    };
    // A long-press: hold past the threshold, where one empty tick fires the hold, then release.
    let press_hold = |app: &mut App, now: &mut u32, b| {
        feed(app, *now, vec![down(b)]);
        *now += hold + 80;
        feed(app, *now, vec![]);
        *now += 30;
        feed(app, *now, vec![up(b)]);
        *now += 30;
    };
    // Held partway, with no release and no threshold crossing: the in-flight long-press hint.
    let partial_hold = |app: &mut App, now: &mut u32, b| {
        feed(app, *now, vec![down(b)]);
        *now += hold * 55 / 100; // ~55% toward the threshold
        feed(app, *now, vec![]); // samples the in-flight progress for the render
    };
    // A chord: two buttons squeezed inside the 100 ms window, released together. The recognizer
    // swallows both, so the script gets the drawer and no constituent gesture.
    let chord = |app: &mut App, now: &mut u32, a, b| {
        feed(app, *now, vec![down(a)]);
        *now += 30;
        feed(app, *now, vec![down(b)]);
        *now += 60;
        feed(app, *now, vec![up(b), up(a)]);
        *now += 30;
    };

    for ch in script.chars() {
        if ch == ' ' {
            continue;
        }
        hook(app, ScriptHook::Before(ch), now);
        match ch {
            'd' => step(app, &mut now, 1),
            'u' => step(app, &mut now, -1),

            'p' => tap(app, &mut now, Button::Select),
            'b' => tap(app, &mut now, Button::Back),
            'h' => press_hold(app, &mut now, Button::Select),
            'B' => press_hold(app, &mut now, Button::Back),
            'H' => partial_hold(app, &mut now, Button::Select),
            'M' => partial_hold(app, &mut now, Button::Back),
            // Settle: step the clock in animation-sized ticks until any time-driven animation has
            // finished. A sweep integrates a dt-capped step per poll, so one big jump would leave
            // it mid-flight. Not for use after `H` or `M`: the empty feeds would cross the hold
            // threshold and fire the hold those tokens leave armed.
            'w' => {
                for _ in 0..8 {
                    now += 100;
                    feed(app, now, vec![]);
                }
            }
            // Draw one throwaway frame to flush lazy draw-time state, so the next gesture sees it.
            // `p f p` opens a POI list, fills its snapshot, then presses a POI into its detail.
            'f' => hook(app, ScriptHook::Render, now),
            // One route-aware tick: sync and open the active route, and run the once-per-load
            // state builds the GUI's per-frame tick would have run.
            'T' => hook(app, ScriptHook::Tick, now),
            // Idle-elapse: jump the clock forward with no input and run one animation pass, so the
            // idle-return timeout fires deterministically for a snapshot. Longer than every
            // configurable timeout, so it fires for any idle-return setting but Never.
            // The quick drawer's Up and Select squeeze, then its slide settled: one token, because
            // every drawer frame starts with it.
            'Q' => {
                chord(app, &mut now, Button::Up, Button::Select);
                for _ in 0..12 {
                    now += 40;
                    feed(app, now, vec![]);
                }
            }
            // Hold the same raw button pair past the Assistant threshold.
            'A' => {
                feed(app, now, vec![down(Button::Up)]);
                now += 30;
                feed(app, now, vec![down(Button::Select)]);
                now += hold;
                feed(app, now, vec![]);
                now += 30;
                feed(app, now, vec![up(Button::Select), up(Button::Up)]);
                now += 30;
            }
            // The contextual drawer's Down and Back squeeze, then its slide settled.
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
        hook(app, ScriptHook::After(ch), now);
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
  --center LON,LAT        Headless camera or local Assistant study centre in microdegrees
  --zoom MULT             Headless bbox-fit zoom multiplier
  --heading DEG           Start in heading-up mode at this course
  --peak-view PLACE       Peak panorama: gornergrat|scheidegg|glockner

Ride and storage fixtures:
  --gpx PATH              Replay a GPX track
  --at SECONDS            GPX playback time for a headless render
  --script-at SECONDS     GPX script position; replay continues from here to --at
  --card PATH             Reopen an existing persistent card without importing files
  --create-card PATH      Create a new card, import MAP/routes once, then exit
  --routes-dir DIR        Route and trip import directory (default: routes/)
  --tracks-dir DIR        Ride/track-store directory (default: tracks/)
  --import PATH           Import GPX to --card, or convert to --routes-dir, then exit

Device state:
  --route-cleanup        Show the storage cleanup dialog
  --no-card               Simulate an absent storage card
  --boot                  Start headless rendering at the power-on Home screen
  --battery PCT           Initial battery charge, 0..=100
  --clock DATE            Trusted UTC time, YYYY-MM-DDTHH:MM (local offset defaults to 0)
  --clock-after-script DATE  Set trusted UTC time after the script (same format and offset)
  --utc-offset-min N      Local offset for explicit clocks, minutes (-720..840; default 0)
  --lang LANG             UI language: en|de|fr|es
  --stat-fields LIST      Comma-separated Statistics field ids
  --physical              Use saved physical-size calibration in the GUI
  --ble STATE             connected|paired|passkey=N (join independent facts with +)
  --sensors MODE          demo|screen

Scripted snapshots:
  --script-after TOKENS  Apply button input after GPX replay, before rendering
  --script TOKENS         Apply device-button script tokens before rendering
                          (d/u step, p press, b back, h/B hold, H/M partial hold,
                           Q quick-drawer tap, A held Up+Select (Assistant), C context-drawer squeeze,
                           w wait, f frame, T tick, I idle)
  --trip-progress D:M:L   The first trip's progress: day D (from 0), M metres into it, and the
                          last finished day L (or -)
  --no-backlight          Model a panel with no controllable light (three quick-drawer controls)
  --diagnostics PATH      Write a new JSONL journey trace (requires --png)
  --expect-screen NAME    Refuse unless the script lands on this screen
  --hold PLAN             Consume without starting one request: nav|detour
  --inject EVENT          nav-fail=KIND|detour-fail=KIND|upload=ID|
                          upload-replace=ID|trip-upload=N (TP{N}.OBT)|warning=LIST|
                          map-transfer=receiving:RECEIVED/TOTAL|
                          map-transfer=installed|map-transfer=failed:KIND
  --dfu STATE             scan=KIND|progress=KIND|installing=KIND|
                          error=ERR|confirmed=VERSION|failed=WHY[:VERSION]
  --freeze                Show the live recalculation freeze over the map

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
    let mut args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{HELP}");
            std::process::exit(2);
        }
    };

    let diagnostics = diagnostics::Diagnostics::open(args.diagnostics.as_deref()).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(1);
    });
    if diagnostics.enabled() {
        if args.png.as_ref().and_then(|path| std::fs::canonicalize(path).ok())
            == args.diagnostics.as_ref().and_then(|path| std::fs::canonicalize(path).ok())
        {
            eprintln!("diagnostics and PNG must use different paths");
            std::process::exit(2);
        }
        diagnostics.record(
            "session",
            serde_json::json!({
                "schema": 1, "args": std::env::args().collect::<Vec<_>>(),
                "cwd": std::env::current_dir().ok(), "version": env!("CARGO_PKG_VERSION"),
            }),
        );
    }

    // `--palette`: the device's 64-color gamut on a standalone color-test screen, which needs no
    // map. With `--png` it writes the frame headlessly; otherwise it opens a minimal window.
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

    if let Some(gpx) = &args.import {
        let result = if let Some(path) = &args.card {
            card::persistent(path, false).and_then(|owner| {
                let map = match map_file::LoadedMap::reopen(&owner) {
                    Ok(map) => Some(map),
                    Err(obc_host_core::flat_map::MapError::Storage(obc_storage::flat::StoreError::NotFound)) => None,
                    Err(error) => return Err(error.to_string()),
                };
                let mut routes = RouteStore::new(owner, &[]).map_err(|error| error.to_string())?;
                routes::import_gpx(
                    &mut routes,
                    std::path::Path::new(gpx),
                    obc_route::BikeType::Road,
                    map.as_ref()
                        .map(|map| map.reader())
                        .as_ref()
                        .zip(map.as_ref())
                        .map(|(reader, map)| (reader, map.route_attribution_key())),
                )
            })
        } else {
            let map = if args.map.is_empty() {
                Ok(None)
            } else {
                map_file::MapSource::load_single(&args.map)
                    .and_then(map_file::LoadedMap::open)
                    .map(Some)
                    .map_err(|error| error.to_string())
            };
            map.and_then(|map| {
                routes::export_gpx(
                    std::path::Path::new(gpx),
                    std::path::Path::new(&args.routes_dir()),
                    obc_route::BikeType::Road,
                    map.as_ref()
                        .map(|map| map.reader())
                        .as_ref()
                        .zip(map.as_ref().map(|map| map.route_attribution_key())),
                )
            })
        };
        match result {
            Ok(stats) => eprintln!("imported {gpx} | {} m, +{} m", stats.total_distance_m, stats.total_ascent_m),
            Err(error) => {
                eprintln!("import failed: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    let card::Session { map, routes, trips, rides, tracks, .. } =
        card::Session::load(&mut args).unwrap_or_else(|error| {
            eprintln!("session failed: {error}");
            std::process::exit(1);
        });
    let source = map.map_source();
    eprintln!("card {:?} | map {} revision {}", source.store_id(), source.id().0, source.revision().0);
    if diagnostics.enabled() {
        diagnostics.record(
            "map",
            serde_json::json!({
                "store": format!("{:?}", source.store_id()), "object": source.id().0,
                "revision": source.revision().0, "fingerprint": format!("{:?}", source.fingerprint()),
            }),
        );
    }
    if args.create_card.is_some() {
        eprintln!("card created; inputs unchanged");
        return;
    }
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
        // One reader over the one map file: the map plane, nav, POI, hours and routing all read
        // it, as they do on the device.
        let reader = map.reader();
        let (mut cx, mut cy, mut zoom) = initial_camera(&reader, args.width);
        if let Some((lon, lat)) = args.center {
            cx = lon;
            cy = lat;
        }
        zoom *= args.zoom_mul;
        let mut state = AppState::new(cx, cy, zoom);
        let mut peak_runtime = peak_view::Runtime::new(&map, args.peak_view);
        state.peak_view_profile = peak_runtime.profile();
        if let Some(profile) = args.peak_view.map(peak_view::Preset::profile) {
            state.peak_view_profile = Some(profile.detached());
            state.compass_deg = Some(profile.default_heading_q4 as f32 / 4.0);
            state.user_fix =
                Some(Fix { lat: profile.observer_lat, lon: profile.observer_lon, course: None, speed_mps: Some(0.0) });
        }
        if let Some(b) = args.battery {
            state.device.battery_pct = b;
        }
        // `--heading` renders a heading-up frame, and the rotation derives from the fix's course,
        // so seed one at the map center.
        if let Some(deg) = args.heading {
            state.compass_deg = Some(deg);
            state.heading_up = true;
            let (lat, lon) = state.user_fix.map(|f| (f.lat, f.lon)).unwrap_or((cy, cx));
            state.user_fix = Some(Fix { lat, lon, course: Some(deg), speed_mps: None });
        }
        // Seed the camera and script fix at `--script-at`, or `--at`. The replay up to `--at` runs
        // below, after the route opens, so the snapshot shows live riding state.
        let mut player: Option<GpxPlayer> = None;
        let mut replay_to = 0.0_f64;
        let mut replay_from = 0.0_f64;
        let mut script_at = 0.0_f64;
        if let Some(path) = &args.gpx {
            match Track::load(std::path::Path::new(path)) {
                Ok(track) => {
                    let mut p = GpxPlayer::new(track);
                    (script_at, replay_from, replay_to) = args.replay_range(p.duration()).unwrap_or_else(|e| {
                        eprintln!("{e}");
                        std::process::exit(1);
                    });
                    p.seek(script_at);
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
        if app.state.user_fix.is_some() {
            let mut loc = crate::sim_location::SimLocationSource::new(app.state.user_fix);
            app.tick(obc_ports::RideClock(0), obc_ports::Sensors::new(&mut loc), None);
        }
        // Explicit headless settings are applied before the script. Without an explicit clock the
        // device's boot time stays untrusted.
        if args.clock.is_some() || args.lang.is_some() || args.stat_fields.is_some() || args.sensors.is_some() {
            let mut settings = obc_app::settings::Settings::default();
            if let Some(clock) = args.clock {
                settings.clock = clock;
            }
            if let Some(lang) = args.lang {
                settings.language = lang;
            }
            // `--sensors demo`: pin the three sensor tiles onto the visible Statistics page so the
            // snapshot shows them, replacing the default six.
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
            // `--stat-fields` replaces the grid selection wholesale, so a snapshot can put any
            // field on the visible page without walking the Fields editor. Applied after
            // `--sensors demo`, so an explicit list wins.
            if let Some(fields) = &args.stat_fields {
                let mut sf = empty_stat_field_list();
                for f in fields {
                    // Cannot fail: `parse_stat_fields` pushed the identical list onto an identical
                    // grid and rejected the argument if any of it did not fit.
                    let added = sf.push(*f);
                    debug_assert!(added, "`--stat-fields` is validated at parse time");
                }
                settings.stat_fields = sf;
            }
            // `--sensors screen`: two saved slots, so the row screen reads Connected and Searching
            // and the third stays unset. A settings write, so it survives into the row status gate;
            // the live phase and battery come from the status snapshot pushed after the script.
            if args.sensors == Some(SensorSeed::Screen) {
                settings.saved_sensors[0] = obc_app::SavedSensor::saved(1, [0x66, 0x55, 0x44, 0x33, 0x22, 0x11]);
                settings.saved_sensors[1] = obc_app::SavedSensor::saved(0, [0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F]);
            }
            app.set_settings(settings);
        }
        args.stamp_initial_clock(&mut app);
        // Whether the map carries a nav graph at all, which gates the ride menu's Detour station.
        app.set_map_nav_graph(tables.has_nav_graph());
        // Device-info built-ins for the System settings screen: the running firmware version, with
        // the sim's crate version standing in for the board's tag, and the loaded map's name and
        // version from the parsed header. The free-space scan is answered after the script, which
        // mirrors the on-entry FAT scan seam.
        // The panel-light capability the drawer's root row is built from. The headless host models
        // a lit panel, like the window does, so a snapshot shows the four-icon arrangement;
        // `--no-backlight` renders the three-control one instead.
        app.set_backlight_available(!args.no_backlight);
        // No `set_resident_frame` here: the headless host composes one frame into a buffer that
        // holds nothing, so every screen must be drawn, including a base a resident host would
        // leave standing under a sheet.
        app.set_fw_version(env!("CARGO_PKG_VERSION"));
        let map_name = map.display_name();
        app.set_map_info(map_name, tables.version);
        // The startup import has committed route identities before the first catalog feed.
        let mut store = routes;
        app.set_routes_with_ids(store.catalog(), store.ids());

        // The same host-protocol owner the interactive simulator drives. Headless runs each plan
        // to completion inside a pass, and its planned detour stays resident until commit or cancel.
        let mut host = HostLoop::new();
        if diagnostics.enabled() {
            host.set_trace(Box::new(diagnostics.clone()));
        }
        if let Some(scope) = store.store_scope() {
            host.facts().note_store_revision(scope);
        }
        // Trip stage references are already remapped to the card's route identities. Fed after the
        // routes, so the stage ids resolve against the catalog.
        if args.route_cleanup {
            if let Some(scope) = store.store_scope() {
                app.offer_route_cleanup(scope.store);
            }
        }
        let mut trip_store = trips;
        app.set_trips(&trip_store.inputs());
        app.set_trip_progress(trip_store.progress().iter().cloned());
        let join = obc_host_core::day_join(&app, &store, &trip_store);
        app.set_day_join(join);
        // The complete saved-ride projection comes from the same card as the map and routes.
        let mut ride_store = rides;
        app.set_rides(ride_store.catalog(), ride_store.trip_names());
        // Inject BLE before the script. `+` keeps independent link, bond and passkey facts.
        let ble = args.ble.unwrap_or_default();
        app.set_ble_status(obc_app::BleStatus {
            link: if ble.connected { obc_app::BleLink::Connected } else { obc_app::BleLink::Advertising },
            passkey: ble.passkey,
            paired: ble.paired,
        });
        // Planner emission and map-referenced altitude share terrain from this retained map.
        let mut elev = map.elevation();
        // The open ride log. Opened above the script, because every settling pass reconciles it
        // against the app's tracking session as a frame loop does.
        let mut tracks = tracks;
        tracks.offer_recovery(&mut app);
        // The resident active-route parse, shared by every settle and by the final render.
        let mut session = ActiveRouteSession::new();
        // What only this host can do: the fixed free-space figure and the `--dfu` answers.
        //
        // Staged above the script, because `DfuState` asks exactly once: the wait the script leaves
        // on top emits one scan effect, and an executor with no answer ready would consume that
        // operation and park the flow. `Progress` stages no install answer, because that unanswered
        // arm is the spinner.
        let mut platform = HeadlessPlatform::default();
        if let Some(dfu) = &args.dfu {
            platform.scan = match dfu {
                DfuSeed::Scan(kind) | DfuSeed::Progress(kind) | DfuSeed::Installing(kind) => Some(kind.report()),
                DfuSeed::Error(e) => Some(Err(*e)),
                _ => None,
            };
            platform.install = matches!(dfu, DfuSeed::Installing(_)).then_some(Ok(()));
        }
        // `--hold nav` and `--inject nav-fail=...` acquire the search without starting it, so the
        // planning screen stays up and the injected answer below answers the operation the rider
        // started.
        let hold = PlanHold::new(
            args.hold == Some(Hold::Nav) || matches!(args.inject, Some(Injection::NavFail(_))),
            args.hold == Some(Hold::Detour) || matches!(args.inject, Some(Injection::DetourFail(_))),
        );
        host.set_plan_hold(hold);

        let mut script_now = 100u32;
        if let Some(script) = &args.script {
            let mut stores =
                Stores { routes: &mut store, rides: &mut ride_store, trips: &mut trip_store, tracks: &mut tracks };
            script_now = headless_script::Session {
                diagnostics: diagnostics.clone(),
                host: &mut host,
                route: &mut session,
                stores: &mut stores,
                map: &map,
                reader: &reader,
                elevation: &mut *elev,
                platform: &mut platform,
                peak: &mut peak_runtime,
                player: &mut player,
                size: (args.width, args.height),
            }
            .run(&mut app, script, script_now, obc_ports::RideClock(0), Some(script_at));
        }
        if let Some(clock) = args.clock_after_script {
            app.stamp_clock(clock, 0, Some(args.utc_offset_min.unwrap_or(0)), obc_app::ClockTrust::Ble);
        }
        // Everything the script's last press asked for, with no trailing `f`: settle it now so the
        // final render reflects the answer.
        let mut stores =
            Stores { routes: &mut store, rides: &mut ride_store, trips: &mut trip_store, tracks: &mut tracks };
        let mut settle_now =
            |app: &mut App, stores: &mut Stores<'_>, host: &mut HostLoop, platform: &mut HeadlessPlatform| {
                settle(host, &mut session, app, stores, map.planner_map(), &mut *elev, platform, script_now);
            };
        settle_now(&mut app, &mut stores, &mut host, &mut platform);

        // The scripted injections. Each is what the device sees: an outcome answering the
        // operation the rider started, carrying the token the executor holds for it, or an external
        // fact nobody asked for. The pass consumes them at its first two stages, so the snapshot
        // pins the same seam the board runs.
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
                map.planner_map(),
                &mut *elev,
                &mut platform,
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
                map.planner_map(),
                &mut *elev,
                &mut platform,
                script_now,
            );
        }

        // A committed route upload: the catalog above is the already-rescanned store, and this is
        // the fact that names the committed id, which is the device's own order. The route's mini
        // elevation band is built from the committed OBCR at commit time, and the idle card draws
        // it.
        if let Some(Injection::Upload { id, replaced }) = args.inject {
            let elevation = routes::elevation_sparkline(stores.routes, id);
            host.facts().note_route_upload(obc_app::device_core::RouteUpload { id, replaced, elevation });
            settle(
                &mut host,
                &mut session,
                &mut app,
                &mut stores,
                map.planner_map(),
                &mut *elev,
                &mut platform,
                script_now,
            );
        }
        // Import fixture numbers were resolved to committed trip identities at startup.
        if let Some(Injection::TripUpload { id }) = args.inject {
            host.facts().note_trip_upload(obc_app::device_core::TripUpload { id, replaced: false });
            settle(
                &mut host,
                &mut session,
                &mut app,
                &mut stores,
                map.planner_map(),
                &mut *elev,
                &mut platform,
                script_now,
            );
        }
        // The board's live map-transfer state: a level the ride loop polls each pass, and a feeder
        // rather than a fact.
        if let Some(Injection::MapTransfer(state)) = args.inject {
            app.set_map_transfer(Some(state));
        }
        // Device warnings: the sim has no probe or fragmented card to trip them for real, so they
        // arrive as the fact the board raises.
        if let Some(Injection::Warning(w)) = args.inject {
            host.facts().raise_warnings(w);
            settle(
                &mut host,
                &mut session,
                &mut app,
                &mut stores,
                map.planner_map(),
                &mut *elev,
                &mut platform,
                script_now,
            );
        }

        // DFU sideload snapshots. The scan ran above, answered by the staged platform when the
        // wait asked for it, so the confirm, progress and error cards render off the same app state
        // the device reaches. What is left is the rider's own Confirm.
        if let Some(dfu) = &args.dfu {
            if matches!(dfu, DfuSeed::Progress(_) | DfuSeed::Installing(_)) {
                // Confirm, where Install is the default selection, is the arm request. A tap: down,
                // then up 80 ms later, well under the long-press threshold.
                //
                // Taken as a floor on the script's own clock and not as an absolute: the UI clock
                // must never move backwards, and a longer script would otherwise re-open every
                // bounded window it had closed.
                let now = script_now.max(500_000);
                feed(&mut app, now, vec![InputEvent::Button(ButtonEvent::Down(Button::Select))]);
                feed(&mut app, now + 80, vec![InputEvent::Button(ButtonEvent::Up(Button::Select))]);
                settle(
                    &mut host,
                    &mut session,
                    &mut app,
                    &mut stores,
                    map.planner_map(),
                    &mut *elev,
                    &mut platform,
                    now + 80,
                );
            }
        }
        // The one-time post-update toast and its failure twin: this boot's update result is a fact,
        // and the pass's card scheduler pushes the card from it.
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
            settle(&mut host, &mut session, &mut app, &mut stores, map.planner_map(), &mut *elev, &mut platform, now);
        }

        // `--sensors screen`: once the script lands on the Sensors screen or its scan list, push
        // the per-slot status and the canned scan hits, so the three-row screen reads its statuses
        // and the scan list shows filtered hits. Each screen ignores the half it does not draw.
        if args.sensors == Some(SensorSeed::Screen) {
            let status = [
                obc_app::SensorStatus { phase: obc_app::SensorPhase::Connected, battery: Some(78), last_value_ms: 0 },
                obc_app::SensorStatus { phase: obc_app::SensorPhase::Searching, battery: None, last_value_ms: 0 },
                obc_app::SensorStatus::default(),
            ];
            app.set_sensor_status(&status);
            app.set_sensor_scan_hits(&fake_scan_hits());
        }

        // Replay from `--script-at` up to `--at`, one device frame per step, so the map-matcher
        // locks on and the ride accumulators and breadcrumb fill. A coarse but bounded step keeps
        // long tracks fast while staying under the dropout and teleport gates. The UI clock stands
        // still at the script's mark: a replay drives the ride, and aging the UI on top of it would
        // run every card and idle timer through the whole track at once.
        let mut replay_clock = obc_ports::RideClock(0);
        if let Some(p) = player.as_mut() {
            let mut baro = BaroSensor::new();
            p.seek(replay_from);
            // `play` restarts at zero when already at the end. An empty range stays still.
            if replay_from < replay_to {
                p.play();
            }
            let step = ((replay_to - replay_from) / 400.0).clamp(1.0, 8.0);
            let mut t = replay_from;
            while t < replay_to {
                session.sync(&app, stores.routes);
                let mut plan = {
                    let src = stores.routes.active_source();
                    let route = match (session.index(), src) {
                        (Some(i), Some(s)) => Some(RouteReader::new(i, s)),
                        _ => None,
                    };
                    let dt = if args.script_at.is_some() { step.min(replay_to - t) } else { step };
                    let (ride, sensors) = headless_replay_advance(p, &mut baro, dt, replay_from);
                    replay_clock = ride;
                    host.pass(
                        &mut app,
                        obc_app::device_core::PassClock { ride, ui: InputClock(script_now) },
                        &[],
                        sensors,
                        route.as_ref(),
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
                    map.planner_map(),
                    &mut *elev,
                    &mut platform,
                );
                // The map-referenced altimeter's one terrain read per fix, from the same retained map
                // terrain the router emits from, drained behind the pass as the board's ride loop
                // does.
                app.sample_terrain(&mut *elev);
                t = (t + step).min(replay_to);
            }
        }

        // `--sensors demo`: one final frame fed a fixed synthetic heart rate, power and cadence
        // through the HAL sensor traits, so the three stat tiles render live values in the
        // Statistics-grid snapshot. Stamped at the replay's own `now_ms`, so the 5 s staleness gate
        // reads them fresh.
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
                let ride = obc_ports::RideClock(((p.time() - replay_from) * 1000.0) as u32);
                replay_clock = ride;
                let mut plan = {
                    let src = stores.routes.active_source();
                    let route = match (session.index(), src) {
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
                    map.planner_map(),
                    &mut *elev,
                    &mut platform,
                );
            }
        }

        if let Some(script) = &args.script_after {
            script_now = headless_script::Session {
                diagnostics: diagnostics.clone(),
                host: &mut host,
                route: &mut session,
                stores: &mut stores,
                map: &map,
                reader: &reader,
                elevation: &mut *elev,
                platform: &mut platform,
                peak: &mut peak_runtime,
                player: &mut player,
                size: (args.width, args.height),
            }
            .run(&mut app, script, script_now, replay_clock, None);
            settle_at(
                &mut host,
                &mut session,
                &mut app,
                &mut stores,
                map.planner_map(),
                &mut *elev,
                &mut platform,
                obc_app::device_core::PassClock { ride: replay_clock, ui: InputClock(script_now) },
            );
        }

        // `--freeze` engages the Recalculating freeze through the same seam a drained plan command
        // takes, so the snapshot shows the real banner over the real frozen map.
        if args.freeze {
            app.debug_set_plan_live(true);
        }

        let mut fb = Framebuffer::new(args.width, args.height);
        let mut scratch = Box::new(obc_render::RenderScratch::new());

        session.sync(&app, stores.routes);
        let route = obc_host_core::frame::active_route(&session, stores.routes);
        let scene = obc_host_core::frame::Scene { reader: &reader, route: route.as_ref() };

        // `--expect-screen`: the recipe states where its gestures were supposed to land, and the
        // sim checks it against the `screens!` table's own name before a pixel is written. Checked
        // below every seam that can still change the top screen, so what is verified is what gets
        // saved.
        if let Some(expected) = &args.expect_screen {
            let landed = app.top_screen().name();
            if landed != expected {
                diagnostics.record("failed", serde_json::json!({"expected_screen": expected, "screen": landed}));
                if let Err(error) = diagnostics.check() {
                    eprintln!("{error}");
                }
                eprintln!("error: --expect-screen {expected}, but the script landed on {landed}");
                std::process::exit(1);
            }
        }

        peak_runtime.finish(&mut app);
        let panorama = peak_runtime.panorama();
        let t0 = std::time::Instant::now();
        let mut stats = map_file::render_frame(
            &mut app,
            &mut scratch,
            &mut fb,
            scene,
            panorama,
            (args.width as f32, args.height as f32),
            device_rgb888,
        );
        stats.render_us = t0.elapsed().as_micros() as u32;
        peak_runtime.note_frame_presented(&app);
        if diagnostics.enabled() {
            diagnostics.record(
                "render",
                serde_json::json!({
                    "screen": app.top_screen().name(), "host_render_us": stats.render_us,
                    "map_reads": stats.map_sd_reads, "map_bytes": stats.map_bytes_read,
                    "features_drawn": stats.features_drawn, "features_dropped": stats.features_dropped,
                }),
            );
        }
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
        // The span, point and ring scratch, split by render path.
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
        if let Err(e) = write_png(&fb, args.scale, path) {
            eprintln!("{e}");
            std::process::exit(1);
        }

        diagnostics.record("finished", serde_json::json!({"png": path, "screen": app.top_screen().name()}));
        if let Err(error) = diagnostics.check() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        eprintln!("wrote {path}");
        return;
    }

    // Interactive: hand the map to the eframe host window.
    if let Err(e) = gui::run(map, routes, trips, rides, tracks, args) {
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
    fn after_script_requires_headless_replay() {
        assert!(parse(&["--script-after", "p"]).is_err());
        assert!(parse(&["--gpx", "ride.gpx", "--script-after", "p"]).is_err());
        assert!(parse(&["--png", "out.png", "--script-after", "p"]).is_err());
        assert_eq!(
            parse(&["--gpx", "ride.gpx", "--png", "out.png", "--script-after", "p f d h f"])
                .unwrap()
                .script_after
                .as_deref(),
            Some("p f d h f")
        );
    }

    #[test]
    fn script_replay_ranges_reject_invalid_times_and_preserve_defaults() {
        let parse_range = |start: &str, end: &str| {
            parse(&["--gpx", "ride.gpx", "--png", "out.png", "--script", "T", "--script-at", start, "--at", end])
                .and_then(|a| a.replay_range(100.0))
        };
        for (start, end) in [("-1", "10"), ("NaN", "10"), ("1", "inf"), ("20", "10"), ("10", "101")] {
            assert!(parse_range(start, end).is_err(), "{start}..{end}");
        }
        for flags in [
            vec!["--script-at", "0"],
            vec!["--script-at", "0", "--gpx", "ride.gpx", "--script", "T"],
            vec!["--script-at", "0", "--gpx", "ride.gpx", "--png", "out.png"],
        ] {
            assert!(parse(&flags).is_err());
        }
        assert_eq!(parse_range("25", "75").unwrap(), (25.0, 25.0, 75.0));
        assert_eq!(parse_range("100", "100").unwrap(), (100.0, 100.0, 100.0));
        assert_eq!(parse(&[]).unwrap().replay_range(100.0).unwrap(), (50.0, 0.0, 50.0));
        assert_eq!(parse(&["--at", "75"]).unwrap().replay_range(100.0).unwrap(), (75.0, 0.0, 75.0));
    }

    #[test]
    fn trimmed_replay_polls_actual_positions_with_elapsed_ride_time() {
        use obc_replay::gpx::TrackPoint;
        let mut player = GpxPlayer::new(Track {
            points: vec![
                TrackPoint { lat: 48_000_000, lon: 7_000_000, ele: Some(100.0), t: 0.0 },
                TrackPoint { lat: 48_000_000, lon: 7_000_100, ele: Some(110.0), t: 10.0 },
            ],
        });
        let args = parse(&["--gpx", "ride.gpx", "--png", "out.png", "--script", "T", "--script-at", "5", "--at", "8"])
            .unwrap();
        let (script, from, end) = args.replay_range(player.duration()).unwrap();
        player.seek(script);
        assert_eq!(player.poll().unwrap().lon, 7_000_050);
        // The script's `T` token polls this same position through the normal location port.
        player.seek(script);
        assert_eq!(player.poll().unwrap().lon, 7_000_050);
        player.seek(from);
        player.play();
        let mut baro = BaroSensor::new();
        for second in 1..=3 {
            let (ride, sensors) = headless_replay_advance(&mut player, &mut baro, 1.0, from);
            assert_eq!(ride.0, second * 1000);
            assert_eq!(sensors.loc.poll().unwrap().lon, 7_000_050 + second as i32 * 10);
        }
        assert_eq!(player.time(), end);
    }

    #[test]
    fn explicit_clock_offsets_use_the_device_range_and_require_a_time() {
        for offset in ["-720", "0", "60", "120", "840"] {
            let args = parse(&["--clock", "2026-09-14T10:00", "--utc-offset-min", offset]).unwrap();
            assert_eq!(args.utc_offset_min, Some(offset.parse().unwrap()));
        }
        assert!(parse(&["--clock-after-script", "2026-09-14T10:00", "--utc-offset-min", "60"]).is_ok());
        for offset in ["-721", "841", "no"] {
            assert!(parse(&["--clock", "2026-09-14T10:00", "--utc-offset-min", offset]).is_err());
        }
        assert!(parse(&["--utc-offset-min", "60"]).is_err());
    }

    #[test]
    fn persistent_commands_do_not_mix_reopen_with_import_or_reset() {
        let reopen = parse_args_from(["--card", "card.obc"].into_iter().map(String::from)).unwrap();
        assert!(reopen.map.is_empty());
        assert!(parse(&["--card", "card.obc"]).is_err());
        assert!(
            parse_args_from(["--card", "card.obc", "--inject", "trip-upload=1"].into_iter().map(String::from)).is_err()
        );
        assert!(parse(&["--create-card", "card.obc", "--png", "out.png"]).is_err());
        assert!(parse(&["--create-card", "card.obc"]).is_ok());
    }

    #[test]
    fn grouped_flags_parse_typed_states() {
        assert_eq!(parse(&["--peak-view", "scheidegg"]).unwrap().peak_view, Some(peak_view::Preset::KleineScheidegg));
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

    /// The seed forms refuse the values that would silently snapshot a different frame: a progress
    /// bar past full, a zero-length transfer, and an unknown failure reason.
    #[test]
    fn seed_forms_reject_states_the_device_cannot_reach() {
        assert!(parse(&["--inject", "map-transfer=receiving:500/400"]).is_err());
        assert!(parse(&["--inject", "map-transfer=receiving:0/0"]).is_err());
        assert!(parse(&["--inject", "map-transfer=aborted"]).is_err());
        assert!(parse(&["--inject", "map-transfer=failed:melted"]).is_err());
        assert!(parse(&["--inject", "trip-upload=nope"]).is_err());
        assert!(parse(&["--dfu", "failed=exploded"]).is_err());
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
        for flag in [
            "--size",
            "--scale",
            "--png",
            "--heading",
            "--peak-view",
            "--gpx",
            "--at",
            "--script-at",
            "--center",
            "--zoom",
            "--script",
            "--script-after",
            "--expect-screen",
            "--boot",
            "--routes-dir",
            "--tracks-dir",
            "--import",
            "--physical",
            "--palette",
            "--battery",
            "--clock",
            "--no-card",
            "--route-cleanup",
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
        }
        for removed in ["--true-color", "--colorway", "--calibrate", "--screenshot", "--boot-fault", "--set"] {
            assert!(!HELP.contains(removed), "help still advertises {removed}");
        }
        // A grouped flag's forms are the vocabulary a snapshot recipe writes.
        for form in [
            "trip-upload=N",
            "map-transfer=receiving:RECEIVED/TOTAL",
            "map-transfer=installed",
            "map-transfer=failed:KIND",
            "failed=WHY[:VERSION]",
        ] {
            assert!(HELP.contains(form), "help is missing {form}");
        }
    }
}
