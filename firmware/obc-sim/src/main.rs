//! OBC desktop simulator — host shell around the shared renderer.
//!
//! All map drawing lives in `obc_render`, the same code the nRF54L firmware runs
//! against the LS021B7DD02. This binary owns only the host concerns: argument
//! parsing, the eframe window + pan/zoom event loop, PNG output, and the color
//! policy (device 64-color quantization by default, or `--true-color`).
//!
//! Host logic shared with the landing page's wasm host (`obc-web-demo`) — replay stepping, the
//! frame-interleaved `NavPlan`, the in-memory byte sink — lives in `obc-host-core`, not here.

use std::time::Instant;

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use obc_app::{App, AppState, TrackAction};
use obc_ports::{
    Button, ButtonEvent, Fix, InputClock, InputEvent, InputSource, LocationSource, TrackError, TrackPoint, TrackSink,
};
use obc_reader::{rgb565_to_device64, rgb565_to_rgb888, MapCache, MapTables, Reader, SliceSource};
use obc_render::text::{draw_text, Font, TextAlign};

mod calib;
mod device_input;
mod dfu;
mod framebuffer;
mod gui;
mod palette;
mod present;
mod rides;
mod routes;
mod settings_store;
mod sim_compass;
mod sim_location;
mod sim_sensors;
mod track;
mod trips;
use framebuffer::Framebuffer;
use obc_host_core::{finish_nav_plan, initial_camera, replay_step, NavPlan, ReplaySensors, VecSink};
use obc_replay::{gpx::Track, BaroSensor, GpxPlayer};
use obc_route::{RouteIndex, RouteReader};
use rides::RideStore;
use routes::RouteStore;
use track::TrackStore;
use trips::TripStore;

struct Args {
    map: String,
    width: u32,
    height: u32,
    scale: u32,
    png: Option<String>,
    /// Launch the GUI, save its first composited frame to this path, then exit.
    screenshot: Option<String>,
    true_color: bool,
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
    /// Render the font/palette preview instead of the map. Needs no map.
    text_demo: bool,
    /// A gesture script applied before a headless `--png` render, to snapshot a specific
    /// screen. Tokens (one char, spaces ignored): `r`/`l` = turn cw/ccw, `p` = press,
    /// `h` = hold, `b` = back, `B` = back-hold, `H`/`M` = leave the encoder / Back held
    /// partway (snapshots the in-flight long-press hint), `w` = wait ~800 ms so an
    /// in-flight animation (the Menu needle sweep) settles before the snapshot.
    script: Option<String>,
    /// Headless `--png` only: render from the device's real power-on state (Home / Idle,
    /// no route) instead of straight from the map.
    boot: bool,
    /// Folder of `.obcr` routes — the stand-in for the device SD card; defaults to `routes/`.
    routes_dir: Option<String>,
    /// Folder for saved `.gpx` tracks + the in-progress `.obct` log; defaults to `tracks/`.
    tracks_dir: Option<String>,
    /// Headless `--gpx` only: after replaying, finalise the active ride to a `.gpx`
    /// (verifies the load→ride→save loop without the GUI).
    save_track: bool,
    /// Convert this GPX into the routes folder and exit. Needs no map.
    import: Option<String>,
    /// Render the device window at the panel's true physical size (needs a saved
    /// calibration). Falls back to the scaled view if uncalibrated.
    physical: bool,
    /// Open the GUI straight into the 1:1 size-calibration screen.
    calibrate: bool,
    /// Show the device's 64-color gamut and nothing else. Needs no map.
    palette: bool,
    /// Initial housing body color: `coral` | `mint` | `mustard` | `slate` (default slate).
    colorway: Option<String>,
    /// Initial battery charge (0–100 %) shown on the Home gauge; stands in for the not-yet-
    /// wired fuel gauge. Defaults to full.
    battery: Option<u8>,
    /// Seed for the Home screensaver's contour pattern. On the device the seed is the
    /// wall-clock millis at each return to Home; this pins it for a headless render.
    home_seed: Option<u32>,
    /// Headless `--png` only: seed the device's local wall-clock to `YYYY-MM-DDTHH:MM` (in manual
    /// mode, so `local_clock()` returns it verbatim). Pins the POI-detail "today's hours" weekday +
    /// the OPEN/CLOSED-now badge for a reproducible render. Defaults to the device default
    /// (2025-01-01 12:00, a Wednesday noon).
    clock: Option<obc_app::settings::DateTime>,
    /// Headless `--png` only: the UI language `en` | `de` | `fr` | `es` (epic #602). Seeded into
    /// `Settings.language` before the render, so a scripted screen draws its de/fr/es copy from the
    /// i18n catalog — the per-language snapshot mechanism. Defaults to `en` (the device default), so
    /// omitting it leaves the English output byte-identical.
    lang: Option<obc_app::settings::Language>,
    /// Headless `--png` only: render with a phone linked over BLE, so the connected indicator
    /// shows (the menu title bar / Home). Stands in for the sim control panel's "Phone connected"
    /// toggle when capturing a snapshot.
    ble_connected: bool,
    /// Headless `--png` only: inject a BLE pairing passkey so the host-pushed passkey card is up
    /// (epic #447, P2), for the `passkey-card.png` snapshot. Stands in for the sim control panel's
    /// "Pairing" toggle.
    ble_passkey: Option<u32>,
    /// Headless `--png` only: render with a stored bond, so the Bluetooth screen's Paired row
    /// reads "yes" (and its Forget row arms). Stands in for the control panel's "Paired" toggle.
    ble_paired: bool,
    /// Headless `--png` only: drive the **Sensors settings screen** (epic #707, SE7) with a canned
    /// fake central manager — two saved sensors (HR Connected · 78 %, Power Searching) with Cadence
    /// Not set on the three-row list, plus a filtered scan-hit set for the scan-list screen. Stands in
    /// for the GUI host's fake manager. (Distinct from `--sensors-demo`, which pins the SE5 stat tiles.)
    sensors_screen: bool,
    /// Headless `--png` only: leave a recorded create-route request **un-drained**, so the
    /// planning screen (spinner) stays on top for its snapshot instead of the plan completing
    /// before the render. Implied by `--inject-nav-fail`.
    nav_hold: bool,
    /// Headless `--png` only: inject a **routing failure** (`exhausted` | `nopath`) after the
    /// script runs, through the real `App::notify_nav_result` seam — so the two failure cards
    /// render deterministically for the snapshot net. Needed because the range tier ("Too far to
    /// route here." = the router's fixed table exhausting) is unreachable on the small fixture
    /// graphs: grimsel plans even ~25 km routes inside the 1536-node table and monaco spans ~4 km.
    /// The script must leave the CREATE ROUTE confirm on top (the card replaces it).
    inject_nav_fail: Option<String>,
    /// Headless `--png` only: inject a committed route upload `(object id, replaced-existing)`
    /// after the script runs (epic #447, P4), so the three upload popups render — the idle
    /// "ROUTE RECEIVED" prompt, the mid-ride swap prompt, or (`--inject-upload-replace` of the
    /// navigated id) the "ROUTE UPDATED" info card. Stands in for the control panel's inject
    /// buttons; the catalog is already scanned, so this is exactly the device's rescan-then-event
    /// order.
    inject_upload: Option<(u16, bool)>,
    /// Headless `--png` only: raise device warnings through the real `App::notify_warning` seam
    /// after the script, so the advisory warning card renders. A comma-list of `gps` / `altimeter`
    /// / `compass` / `map` (issue #504) / `rec` (the mid-ride ride-log write error, issue #11) —
    /// e.g. `gps,map`. Stands in for hardware the sim can't trip for real. `rec` here renders the
    /// card directly; to drive it through the *actual* record-failure path, use `--fail-track`.
    inject_warning: Option<obc_app::WarningFlags>,
    /// Headless `--png` only: render the standalone **boot fault** screen (`nocard` | `nomap` |
    /// `badmap`) instead of the app — the undismissable storage-failure screen `main` shows before
    /// the app exists. Snapshots the three fatal SD/map sites without needing a bad card.
    boot_fault: Option<obc_app::BootFault>,
    /// Headless `--png` only: after the track replay, open the [`Climb`](obc_app::screen) screen
    /// directly (epic #506, C4) via `App::debug_open_climb`, so the striped-profile snapshot renders
    /// before C5 wires the screen into the Back-cycle. A no-op unless the replay left a climb active
    /// (so pair it with a `--gpx`/`--at` that reaches one).
    open_climb: bool,
    /// Headless `--gpx` replay only: feed a **fixed synthetic HR/power/cadence** through SE2's HAL
    /// sensor traits for one final tick (epic #707, SE5), and pin the three new sensor stat tiles
    /// (HR/PWR/RPM) onto the Statistics grid, so the tiles render live values in the snapshot. A
    /// minimal stub — SE8 replaces it with the sim control panel's real sensor sliders. Requires a
    /// `--gpx` (the fixed source rides on the replay's location + clock).
    sensors_demo: bool,
    /// Headless `--gpx` replay: make **every ride-log write fail**, as if the SD card were pulled
    /// mid-ride (issue #11). Each logged fix's `TrackSink::record` returns `Err`, so the app raises
    /// the "recording error" warning through the real record path (not the `--inject-warning rec`
    /// shortcut). Pair with a script that starts a ride and a `--gpx`/`--at` that logs a point —
    /// e.g. `--gpx <t> --at 30 --script "p p p p" --fail-track --png out.png`.
    fail_track: bool,
    /// Headless `--png` only: after the script left the "Checking card..." wait on top (System menu
    /// → Install), answer the DFU scan (epic #615 S5, #620) through the real
    /// `App::notify_dfu_scan_result` seam, swapping the wait for the confirm screen. The flavour
    /// selects which warnings render: `normal` (a newer version, rollback available), `same` (the
    /// installed version restaged → same-version warning), or `first` (no rollback → no-undo
    /// warning). The board runs the scan for real; the sim stages a synthetic `UPDATE.BIN`.
    dfu_scan: Option<dfu::DfuScanKind>,
    /// Headless `--png` only: with `--dfu-scan`, press Install on the confirm so the "Preparing
    /// update..." progress spinner renders (the sim never drains the arm, so it stays up).
    dfu_progress: bool,
    /// Headless `--png` only: with `--dfu-scan --dfu-progress`, run the board drain's terminal
    /// swap (`App::show_dfu_installing`) so the static "Installing update" card renders — the
    /// pre-reset frame the MIP panel holds through the whole install.
    dfu_installing: bool,
    /// Headless `--png` only: answer the DFU scan with a typed error so the error card renders
    /// (`notfound` | `unreadable` | `damaged` | `toolarge` | `fragmented`). Needs the "Checking
    /// card..." wait on top (System menu → Install), like `--dfu-scan`.
    dfu_error: Option<obc_app::DfuScanError>,
    /// Headless `--png` only: raise the one-time post-update toast through the real
    /// `App::notify_update_confirmed` seam, tagged with this version, so the "Updated to vX" card
    /// renders (the first-healthy-boot toast).
    dfu_confirmed: Option<String>,
}

impl Default for Args {
    /// Device resolution + all knobs off — the CLI parser's base. The resolution is the single
    /// [`obc_platform`] frame authority, not a re-declared literal (`--size` overrides it for
    /// off-device experiments).
    fn default() -> Self {
        Args {
            map: String::new(),
            width: obc_platform::FRAME_W as u32,
            height: obc_platform::FRAME_H as u32,
            scale: 1,
            png: None,
            screenshot: None,
            true_color: false,
            heading: None,
            gpx: None,
            at: None,
            center: None,
            zoom_mul: 1.0,
            text_demo: false,
            script: None,
            boot: false,
            routes_dir: None,
            tracks_dir: None,
            save_track: false,
            import: None,
            physical: false,
            calibrate: false,
            palette: false,
            colorway: None,
            battery: None,
            home_seed: None,
            clock: None,
            lang: None,
            ble_connected: false,
            ble_passkey: None,
            ble_paired: false,
            sensors_screen: false,
            nav_hold: false,
            inject_nav_fail: None,
            inject_upload: None,
            inject_warning: None,
            boot_fault: None,
            open_climb: false,
            sensors_demo: false,
            fail_track: false,
            dfu_scan: None,
            dfu_progress: false,
            dfu_installing: false,
            dfu_error: None,
            dfu_confirmed: None,
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

/// Parse a `--clock` value `YYYY-MM-DDTHH:MM` into a [`DateTime`](obc_app::settings::DateTime).
/// Rejects a malformed stamp with a message (out-of-range fields are clamped by `Settings::decode`'s
/// sanitiser when seeded, but the format itself must be well-formed).
fn parse_clock(s: &str) -> Result<obc_app::settings::DateTime, String> {
    let (date, time) = s.split_once('T').ok_or("--clock format is YYYY-MM-DDTHH:MM")?;
    let mut d = date.split('-');
    let mut t = time.split(':');
    let year = d.next().and_then(|v| v.parse().ok()).ok_or("bad --clock year")?;
    let month = d.next().and_then(|v| v.parse().ok()).ok_or("bad --clock month")?;
    let day = d.next().and_then(|v| v.parse().ok()).ok_or("bad --clock day")?;
    let hour = t.next().and_then(|v| v.parse().ok()).ok_or("bad --clock hour")?;
    let minute = t.next().and_then(|v| v.parse().ok()).ok_or("bad --clock minute")?;
    Ok(obc_app::settings::DateTime { year, month, day, hour, minute })
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

fn parse_args() -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
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
            "--screenshot" => a.screenshot = Some(it.next().ok_or("--screenshot needs a path")?),
            "--true-color" => a.true_color = true,
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
            "--text-demo" => a.text_demo = true,
            "--script" => a.script = Some(it.next().ok_or("--script needs a token string")?),
            "--boot" => a.boot = true,
            "--routes-dir" => a.routes_dir = Some(it.next().ok_or("--routes-dir needs a path")?),
            "--tracks-dir" => a.tracks_dir = Some(it.next().ok_or("--tracks-dir needs a path")?),
            "--save-track" => a.save_track = true,
            "--import" => a.import = Some(it.next().ok_or("--import needs a GPX path")?),
            "--physical" => a.physical = true,
            "--calibrate" => a.calibrate = true,
            "--palette" => a.palette = true,
            "--colorway" => a.colorway = Some(it.next().ok_or("--colorway needs a name")?),
            "--battery" => {
                a.battery = Some(
                    it.next().and_then(|s| s.parse().ok()).filter(|&b| b <= 100).ok_or("--battery needs 0..=100")?,
                )
            }
            "--home-seed" => {
                a.home_seed = Some(it.next().and_then(|s| s.parse().ok()).ok_or("--home-seed needs a u32")?)
            }
            "--clock" => {
                a.clock = Some(parse_clock(&it.next().ok_or("--clock needs YYYY-MM-DDTHH:MM")?)?);
            }
            "--lang" => {
                a.lang = Some(parse_lang(&it.next().ok_or("--lang needs en|de|fr|es")?)?);
            }
            "--ble-connected" => a.ble_connected = true,
            "--nav-hold" => a.nav_hold = true,
            "--open-climb" => a.open_climb = true,
            "--sensors-demo" => a.sensors_demo = true,
            "--fail-track" => a.fail_track = true,
            "--dfu-scan" => {
                a.dfu_scan = Some(dfu::DfuScanKind::parse(&it.next().ok_or("--dfu-scan needs normal|same|first")?)?);
            }
            "--dfu-progress" => a.dfu_progress = true,
            "--dfu-installing" => a.dfu_installing = true,
            "--dfu-error" => {
                a.dfu_error = Some(match it.next().ok_or("--dfu-error needs a variant")?.as_str() {
                    "notfound" => obc_app::DfuScanError::NotFound,
                    "unreadable" => obc_app::DfuScanError::Unreadable,
                    "damaged" => obc_app::DfuScanError::Damaged,
                    "toolarge" => obc_app::DfuScanError::TooLarge,
                    "fragmented" => obc_app::DfuScanError::TooFragmented,
                    other => return Err(format!("--dfu-error: unknown variant `{other}`")),
                });
            }
            "--dfu-confirmed" => a.dfu_confirmed = Some(it.next().ok_or("--dfu-confirmed needs a version")?),
            "--inject-nav-fail" => {
                let kind = it.next().ok_or("--inject-nav-fail needs exhausted|nopath")?;
                if kind != "exhausted" && kind != "nopath" {
                    return Err("--inject-nav-fail needs exhausted|nopath".into());
                }
                a.inject_nav_fail = Some(kind);
            }
            "--inject-upload" => {
                let id = it.next().and_then(|s| s.parse().ok()).ok_or("--inject-upload needs an object id")?;
                a.inject_upload = Some((id, false));
            }
            "--inject-upload-replace" => {
                let id = it.next().and_then(|s| s.parse().ok()).ok_or("--inject-upload-replace needs an object id")?;
                a.inject_upload = Some((id, true));
            }
            "--inject-warning" => {
                let spec = it.next().ok_or("--inject-warning needs gps,altimeter,compass,map")?;
                let mut w = obc_app::WarningFlags::NONE;
                for tok in spec.split(',') {
                    w |= match tok.trim() {
                        "gps" => obc_app::WarningFlags::NO_GPS,
                        "altimeter" | "baro" => obc_app::WarningFlags::NO_ALTIMETER,
                        "compass" | "imu" => obc_app::WarningFlags::NO_COMPASS,
                        "map" => obc_app::WarningFlags::MAP_SLOW,
                        "rec" | "record" => obc_app::WarningFlags::REC_ERROR,
                        _ => return Err("--inject-warning tokens: gps|altimeter|compass|map|rec".into()),
                    };
                }
                a.inject_warning = Some(w);
            }
            "--boot-fault" => {
                let kind = it.next().ok_or("--boot-fault needs nocard|nomap|badmap")?;
                a.boot_fault = Some(match kind.as_str() {
                    "nocard" => obc_app::BootFault::NoCard,
                    "nomap" => obc_app::BootFault::NoMap,
                    "badmap" => obc_app::BootFault::BadMap,
                    _ => return Err("--boot-fault needs nocard|nomap|badmap".into()),
                });
            }
            "--ble-passkey" => {
                a.ble_passkey = Some(
                    it.next()
                        .and_then(|s| s.parse().ok())
                        .filter(|&n| n <= 999_999)
                        .ok_or("--ble-passkey needs 0..=999999")?,
                )
            }
            "--ble-paired" => a.ble_paired = true,
            "--sensors-screen" => a.sensors_screen = true,
            other => {
                if a.map.is_empty() {
                    a.map = other.to_string();
                } else {
                    return Err(format!("unexpected arg: {other}"));
                }
            }
        }
    }
    // `--text-demo`, `--palette` and `--import` need no map file.
    if a.map.is_empty() && !a.text_demo && !a.palette && a.import.is_none() {
        return Err("missing map path".into());
    }
    Ok(a)
}

fn color_of(c: u16, true_color: bool) -> Rgb888 {
    let (r, g, b) = if true_color { rgb565_to_rgb888(c) } else { rgb565_to_device64(c) };
    Rgb888::new(r, g, b)
}

/// Pack 8-bit RGB into RGB565 (the color space the renderer quantizes from), so the
/// demo palette below can be written as the spec's hexes.
const fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    (((r as u16) >> 3) << 11) | (((g as u16) >> 2) << 5) | ((b as u16) >> 3)
}

// The "explorer's field map" palette, in RGB565 so it travels through the same
// `color_of` quantization as map styles.
const PARCHMENT: u16 = rgb565(0xEA, 0xDF, 0xC0);
const HUD: u16 = rgb565(0x2E, 0x25, 0x1A);
const INK: u16 = rgb565(0x2C, 0x21, 0x14);
const AMBER: u16 = rgb565(0xE3, 0xA5, 0x2B);
const FOREST: u16 = rgb565(0x4F, 0x6B, 0x43);
const WOOD: u16 = rgb565(0x5B, 0x3F, 0x28);
const WARNING: u16 = rgb565(0xC0, 0x49, 0x2E);

/// Render the font ladder + palette through the device-64 `color_of`, so the PNG
/// shows exactly what the panel would (`--true-color` shows the un-quantized reference).
fn render_text_demo(fb: &mut Framebuffer, true_color: bool) {
    let col = |c: u16| color_of(c, true_color);
    let w = fb.width() as i32;

    let _ = fb.clear(col(PARCHMENT));
    let _ = fb.fill_solid(&Rectangle::new(Point::zero(), Size::new(fb.width(), 28)), col(HUD));
    draw_text(fb, "TERMINUS FONT DEMO", Point::new(w / 2, 3), Font::Label, TextAlign::Center, col(PARCHMENT));

    // Font ladder: each tier's caption over a true-size sample, annotated with its measured
    // cap height in mm so the size targets are checkable (render `--physical` for device scale).
    let sample = "12.5 km/h";
    let mut y = 36;
    for (caption, font) in [
        ("Label  ter24  2.0mm", Font::Label),
        ("Body   ter28  2.4mm", Font::Body),
        ("Disply ter32  2.7mm", Font::Display),
    ] {
        draw_text(fb, caption, Point::new(8, y), Font::Label, TextAlign::Left, col(WOOD));
        y += Font::Label.line_height() as i32 + 2;
        draw_text(fb, sample, Point::new(8, y), font, TextAlign::Left, col(INK));
        y += font.line_height() as i32 + 8;
    }

    // Palette — each name in its own color, so the PNG shows whether they stay distinct
    // and legible after device-64 quantization.
    for (name, c) in [("amber", AMBER), ("forest", FOREST), ("wood", WOOD), ("warning", WARNING)] {
        draw_text(fb, name, Point::new(8, y), Font::Label, TextAlign::Left, col(c));
        y += Font::Label.line_height() as i32 + 2;
    }

    y += 6;
    draw_text(fb, "LEFT", Point::new(8, y), Font::Label, TextAlign::Left, col(INK));
    draw_text(fb, "CENTER", Point::new(w / 2, y), Font::Label, TextAlign::Center, col(INK));
    draw_text(fb, "RIGHT", Point::new(w - 8, y), Font::Label, TextAlign::Right, col(INK));
}

/// Headless one-shot for a drained request: loop the planner to completion (`plan_route`), then
/// commit through the shared [`finish_nav_plan`] — scripted flows don't need frame-interleaved
/// stepping (the live GUI holds an [`obc_host_core::NavPlan`] instead).
fn run_nav_request(app: &mut obc_app::App, store: &mut RouteStore, reader: &Reader, req: &obc_app::NavRequest) {
    use obc_route::nav::{plan_route, NavScratch};
    // Zeroed heap allocation, no giant stack temp (invariant owned by `NavScratch::new_boxed`).
    // The `NavScratch` annotation pins the default `NAV_MAX_NODES` table (an assoc-fn call can't
    // infer the struct's const-generic default the way a type in position does).
    let mut scratch: Box<NavScratch> = NavScratch::new_boxed();
    let mut tiles = obc_reader::NavTileCache::new();
    let mut sink = VecSink::default();
    // The rider's bike-type setting (N5 §8.6); an out-of-range index falls back to profile 0 in the router.
    let profile_idx = app.settings().bike_profile_idx;
    let outcome = plan_route(reader, req.from, req.to, req.name(), profile_idx, &mut scratch, &mut tiles, &mut sink);
    let stats = tiles.stats();
    finish_nav_plan(app, store, outcome, sink.bytes(), stats);
}

/// Reconcile the track store to the app's tracking intent (drains the one-shot action,
/// opens / closes the `.obct` log). The save name comes from the active route's catalog entry.
fn reconcile_tracks(app: &mut App, tracks: &mut TrackStore) {
    let action = app.activity.take_track_action();
    let session = app.activity.session;
    let name = app.activity.active_route.and_then(|i| app.routes().get(i)).map(|r| r.name.as_str().to_string());
    // Snapshot the ride totals for a Save so the durable `RD{id}.ORD` ride object the Rides screen
    // lists carries them, exactly as the device does (#454).
    let stats = matches!(action, Some(obc_app::TrackAction::Save)).then(|| app.ride_stats());
    tracks.reconcile(action, session, name.as_deref(), stats);
}

/// A [`TrackSink`] whose every append fails — the `--fail-track` stand-in for the SD card being
/// pulled mid-ride (issue #11). It stores nothing and returns `Err`, so the app sees a genuine
/// record failure and raises the recording-error card through the real path, not a shortcut.
struct FailTrackSink;
impl TrackSink for FailTrackSink {
    fn record(&mut self, _p: TrackPoint) -> Result<(), TrackError> {
        Err(TrackError)
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

/// Feed one batch of raw events to the app at time `now` (ms).
fn feed(app: &mut App, now: u32, events: Vec<InputEvent>) {
    app.handle_input(InputClock(now), &mut ScriptInput(events.into()));
}

/// Apply a gesture script (see `Args::script`) to `app`. Synthesizes the raw encoder/Back
/// events with a rising clock — including the threshold crossing that turns a held button
/// into a `Hold`/`BackHold` — exactly as the real recognizer would see them.
///
/// `render` draws one throwaway headless frame against the current app state — the `d` token uses
/// it to **flush lazy draw-time state** that only fills at draw (the POI-list snapshot, then the
/// detail's hours read), so a script can `p` into a POI *and then* `d p` to open its detail (the
/// Press needs the snapshot the first draw takes). Without a `d` the whole script runs before the
/// single final render, so lazy state never fills mid-script.
fn apply_script(app: &mut App, script: &str, render: &mut dyn FnMut(&mut App)) {
    let down = |b| InputEvent::Button(ButtonEvent::Down(b));
    let up = |b| InputEvent::Button(ButtonEvent::Up(b));
    let hold = obc_app::DEFAULT_HOLD_MS;
    let mut now: u32 = 100;

    // A turn detent: feed it, then nudge the clock.
    let turn = |app: &mut App, now: &mut u32, dir: i32| {
        feed(app, *now, vec![InputEvent::Turn(dir)]);
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

    for ch in script.chars() {
        match ch {
            ' ' => {}
            'r' => turn(app, &mut now, 1),
            'l' => turn(app, &mut now, -1),
            'p' => tap(app, &mut now, Button::Encoder),
            'b' => tap(app, &mut now, Button::Back),
            'h' => press_hold(app, &mut now, Button::Encoder),
            'B' => press_hold(app, &mut now, Button::Back),
            'H' => partial_hold(app, &mut now, Button::Encoder),
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
            // detail's hours read) so the next gesture sees it — e.g. `p d p` opens a POI list, fills
            // its snapshot, then presses a POI into its detail.
            'd' => render(app),
            // Idle-elapse: jump the clock 5 min forward with no input and run one animation pass, so
            // the app-level idle-return timeout (Part B) fires deterministically for a snapshot —
            // e.g. `B l p I` sits in Settings, elapses, and lands back on Home. Longer than every
            // configurable timeout (max 5 min), so it fires for any `Idle return` setting but Never.
            'I' => {
                now += 5 * 60_000 + 1_000;
                feed(app, now, vec![]);
            }
            other => eprintln!("warning: ignoring unknown --script token '{other}'"),
        }
    }
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\nusage: obc-sim <map.obcm> [--size WxH] [--scale N] [--png OUT] [--true-color] [--heading DEG] [--gpx TRACK.gpx] [--at SEC] [--center LON,LAT] [--zoom MULT] [--text-demo] [--palette] [--script TOKENS] [--boot] [--routes-dir DIR] [--tracks-dir DIR] [--save-track] [--import GPX] [--physical] [--calibrate] [--colorway NAME] [--battery PCT] [--home-seed N] [--clock YYYY-MM-DDTHH:MM] [--lang en|de|fr|es] [--ble-connected] [--ble-passkey N] [--ble-paired] [--sensors-screen] [--inject-upload ID] [--inject-upload-replace ID] [--nav-hold] [--inject-nav-fail exhausted|nopath] [--inject-warning gps,altimeter,compass,map] [--boot-fault nocard|nomap|badmap] [--open-climb]");
            std::process::exit(2);
        }
    };

    // Font/palette preview: render text on a blank panel and exit. Before the map read (needs none).
    if args.text_demo {
        let mut fb = Framebuffer::new(args.width, args.height);
        render_text_demo(&mut fb, args.true_color);
        let path = args.png.as_deref().unwrap_or("text_demo.png");
        if let Err(e) = write_png(&fb, args.scale, path) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        eprintln!("wrote {path}");
        return;
    }

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

    let bytes = std::fs::read(&args.map).unwrap_or_else(|e| {
        eprintln!("cannot read {}: {e}", args.map);
        std::process::exit(1);
    });

    // Validate + log once up front; the borrow ends with this block so `bytes` can move
    // into the GUI (which rebuilds the cheap `Reader` view per frame).
    {
        let cache = MapCache::new();
        let src = SliceSource(&bytes);
        let tables = MapTables::parse(&src).unwrap_or_else(|e| {
            eprintln!("invalid OBCM file: {e:?}");
            std::process::exit(1);
        });
        let reader = Reader::new(&src, &tables, &cache);
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
        let cache = MapCache::new();
        let src = SliceSource(&bytes);
        let tables = MapTables::parse(&src).expect("validated above");
        let reader = Reader::new(&src, &tables, &cache);
        let (mut cx, mut cy, mut zoom) = initial_camera(&reader, args.width);
        if let Some((lon, lat)) = args.center {
            cx = lon;
            cy = lat;
        }
        zoom *= args.zoom_mul;
        let mut state = AppState::new(cx, cy, zoom);
        if let Some(b) = args.battery {
            state.battery_pct = b;
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
        if let Some(seed) = args.home_seed {
            app.reseed_home(seed);
        }
        // `--clock` / `--lang` seed the headless Settings. `--clock` pins the local wall-clock in
        // manual mode (`gps_time = false` ⇒ `local_clock()` returns it verbatim) for the POI-detail
        // weekday + OPEN/CLOSED-now badge; `--lang` selects the UI language (epic #602) so a scripted
        // screen draws its de/fr/es copy. Both stay at the device default otherwise — with neither
        // flag `set_settings` isn't called, and `--clock` alone still leaves `language` English, so
        // the existing snapshots' output is byte-unchanged. `set_settings` restamps the WallClock
        // from this local set-point (see `App::set_settings`).
        if args.clock.is_some() || args.lang.is_some() || args.sensors_demo || args.sensors_screen {
            let mut settings = obc_app::settings::Settings::default();
            if let Some(clock) = args.clock {
                settings.gps_time = false;
                settings.clock = clock;
            }
            if let Some(lang) = args.lang {
                settings.language = lang;
            }
            // `--sensors-demo` (epic #707, SE5): pin the three new sensor tiles onto the visible
            // Statistics page so the snapshot shows them. A dedicated demo selection — HR / PWR /
            // RPM first, then a few live neighbours — replacing the default six.
            if args.sensors_demo {
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
            // `--sensors-screen` (SE7): two saved slots so the row screen reads Connected / Searching
            // (the Not-set third stays empty). A settings write, so it survives into the row status
            // gate; the live phase + battery come from the status snapshot pushed after the script.
            if args.sensors_screen {
                settings.saved_sensors[0] = obc_app::SavedSensor::saved(1, [0x66, 0x55, 0x44, 0x33, 0x22, 0x11]);
                settings.saved_sensors[1] = obc_app::SavedSensor::saved(0, [0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F]);
            }
            app.set_settings(settings);
        }
        // Mirror the map's §8.6 routing-profile names for the Bike-type screen + overview label (N5).
        app.set_nav_profiles(tables.nav_profiles());
        // Device-info built-ins for the System settings screen (T8 item 6): the running firmware
        // version (the sim's own crate version stands in for the board's git-describe tag) and the
        // loaded map's name (filename stem) + OBCM version from the parsed header. The card-free scan
        // is answered after the script (below), mirroring the on-entry FAT scan seam.
        app.set_fw_version(env!("CARGO_PKG_VERSION"));
        let map_stem = std::path::Path::new(&args.map).file_stem().and_then(|s| s.to_str()).unwrap_or("map");
        app.set_map_info(map_stem, tables.version);
        // Load the routes folder so the Route menu has real entries and a picked route
        // can be drawn.
        let mut store = RouteStore::open(args.routes_dir());
        app.set_routes_with_ids(store.catalog(), store.ids());
        // Scan the `.obt` trips beside the routes (epic #526, TR2) — grouped-route folders. Fed
        // **after** the routes so the stage ids resolve against the catalog. The TR3 menu draws the
        // folder rows; until then the grouping is resolved but unrendered (the flat menu is intact).
        let mut trip_store = TripStore::open(args.routes_dir());
        app.set_trips(&trip_store.inputs());
        // Load the tracks folder so the Rides screen (#454) lists real `RD{id}.ORD` rides + their
        // synced flags.
        let mut ride_store = RideStore::open(args.tracks_dir());
        app.set_rides(ride_store.catalog(), ride_store.ids());
        // Inject the BLE link state (epic #447) **before** the script runs: `--ble-connected` shows
        // the connected indicator, `--ble-passkey N` puts the host-pushed passkey card up (P2), and
        // `--ble-paired` a stored bond — so a scripted gesture on the Bluetooth screen (its Forget
        // hold arms only while paired) sees the bond, exactly as the control panel drives it live.
        let link = if args.ble_connected { obc_app::BleLink::Connected } else { obc_app::BleLink::Advertising };
        app.set_ble_status(obc_app::BleStatus { link, passkey: args.ble_passkey, paired: args.ble_paired });
        if let Some(script) = &args.script {
            // The `d` token flushes lazy draw-time state (the POI snapshot / detail hours) by drawing
            // one throwaway frame against the map reader — `route: None` since the POI screens the
            // token targets never draw the route, and the route isn't opened until below anyway.
            // It then drains a pending create-route request (epic #116, R4), so a script can walk
            // the whole POI→route flow: the request's answer swaps the confirm for the overview /
            // failure card, which the next token (or the final render) sees.
            let (rw, rh, rtc) = (args.width, args.height, args.true_color);
            // `--nav-hold` / `--inject-nav-fail` leave the request un-drained so the planning
            // screen stays up (for its own snapshot, or for the injected answer to land in).
            let hold_nav = args.nav_hold || args.inject_nav_fail.is_some();
            let mut render = |app: &mut App| {
                // A pending Ride-detail track request (#680) fills before the draw, so a `d` frame
                // (and every gesture after it) sees the elevation band, mirroring the GUI's
                // per-frame drain.
                if let Some(id) = app.take_ride_track_request() {
                    app.set_ride_profile(ride_store.profile_by_id(id));
                    app.set_ride_preview(&ride_store.preview_by_id(id));
                }
                let mut fb = Framebuffer::new(rw, rh);
                let _ = app.render_frame(&mut fb, &reader, None, rw as f32, rh as f32, |c| color_of(c, rtc));
                if !hold_nav {
                    if let Some(req) = app.take_nav_request() {
                        run_nav_request(app, &mut store, &reader, &req);
                    }
                }
            };
            apply_script(&mut app, script, &mut render);
            // A create-route request recorded by the script's last press (no trailing `d`): drain
            // it now so the final render shows the answer, mirroring the delete drains below.
            if !hold_nav {
                if let Some(req) = app.take_nav_request() {
                    run_nav_request(&mut app, &mut store, &reader, &req);
                }
            }
            // A scripted hold-to-delete in the Route menu (epic #447 P6) records a delete request;
            // execute it here (delete the file + re-feed the id-carrying catalog) so the rendered
            // frame reflects the route being gone, mirroring the GUI's per-frame drain.
            if let Some(id) = app.take_route_delete() {
                if store.delete_by_id(id) {
                    app.set_routes_with_ids(store.catalog(), store.ids());
                }
            }
            // A scripted hold-to-delete on the Ride detail (#680) — same per-frame drain, ride
            // namespace: delete the `RD{id}.ORD` + sidecar flag and re-feed the ride catalog (the
            // detail popped back to the list, whose highlight the remap keeps sane).
            if let Some(id) = app.take_ride_delete() {
                if ride_store.delete_by_id(id) {
                    app.set_rides(ride_store.catalog(), ride_store.ids());
                }
            }
            // A scripted long-press → confirm on a trip folder (epic #526, TR3) records a cascade
            // delete: remove the trip's `.obt` AND every member route file, then re-feed both catalogs
            // (routes first, so the trip's stage ids resolve) — the folder is gone and the menu
            // regroups. Members resolved from the trip store before anything is deleted.
            if let Some(trip_id) = app.take_trip_delete() {
                for rid in trip_store.member_route_ids(trip_id) {
                    store.delete_by_id(rid);
                }
                trip_store.delete_by_id(trip_id);
                app.set_routes_with_ids(store.catalog(), store.ids());
                app.set_trips(&trip_store.inputs());
            }
            // An open Ride detail's track request left by the script's last press (no trailing
            // `d`): fill the resident ride profile now so the final render draws the band.
            if let Some(id) = app.take_ride_track_request() {
                app.set_ride_profile(ride_store.profile_by_id(id));
                app.set_ride_preview(&ride_store.preview_by_id(id));
            }
        }

        // Inject a routing failure (epic #116, R4) after the script left the CREATE ROUTE
        // confirm on top: the answer goes through the real `notify_nav_result` seam, so the
        // snapshot pins the exact error→tier mapping (`exhausted` → "Too far to route here.",
        // anything else → "Couldn't find a route.").
        if let Some(kind) = &args.inject_nav_fail {
            let err = if kind == "exhausted" { obc_route::NavError::Exhausted } else { obc_route::NavError::NoPath };
            app.notify_nav_result(Err(err));
        }

        // Inject a committed route upload (epic #447, P4) after the script (so a `p p p` script
        // is already riding when the event lands): the catalog above is the "already rescanned"
        // store, and this is the upload event with the id — the device's exact order.
        if let Some((id, replaced)) = args.inject_upload {
            // Build the route's mini elevation band from the committed OBCR at "commit time",
            // exactly the seam the board fills (#682) — the idle card draws it.
            let elevation = store.elevation_sparkline(id);
            app.notify_route_uploaded(id, replaced, elevation);
        }
        // Raise device warnings (issue #504) through the real `notify_warning` seam, so the advisory
        // card renders — the sim has no I²C probe / fragmented card to trip it for real.
        if let Some(w) = args.inject_warning {
            app.notify_warning(w);
        }

        // Answer the System screen's card-free scan (T8 item 6). On the board this is a FAT
        // free-cluster scan; the sim has no card, so a fixed built-in stands in (1.2 GB) — posted
        // through the real `set_card_free` seam once the screen's on-entry request is drained.
        if app.take_card_scan_request() {
            const SIM_CARD_FREE: u64 = 1_288_490_188; // ~1.2 GiB → "1.2 GB"
            app.set_card_free(Some(SIM_CARD_FREE));
        }

        // DFU sideload snapshots (epic #615 S5, #620). The scan runs board-side on the device; here
        // the script left the "Checking card..." wait on top (System menu → Install) and these
        // answer it through the real `notify_dfu_scan_result` / `notify_update_confirmed` seams — so
        // the confirm / progress / error / toast screens render off the same app state the device
        // reaches. The sim stages a synthetic `UPDATE.BIN` and runs the real `obc-dfu` scan.
        if let Some(kind) = args.dfu_scan {
            app.notify_dfu_scan_result(kind.report());
            if args.dfu_progress {
                // Confirm (Install is the default selection) → the arm one-shot + the progress
                // spinner. A tap: down, then up 80 ms later, well under the long-press threshold.
                let now = 500_000u32;
                feed(&mut app, now, vec![InputEvent::Button(ButtonEvent::Down(Button::Encoder))]);
                feed(&mut app, now + 80, vec![InputEvent::Button(ButtonEvent::Up(Button::Encoder))]);
                // The board drain's terminal swap: spinner → the static "Installing update" card
                // (the pre-reset frame), through the same seam the device uses.
                if args.dfu_installing {
                    app.show_dfu_installing();
                }
            }
        }
        if let Some(e) = args.dfu_error {
            app.notify_dfu_scan_result(Err(e));
        }
        // The one-time post-update toast: raise the confirmed-update fact, then run one animation
        // pass — `reconcile_update_toast` pushes the "Updated to vX" card there, like the device.
        if let Some(version) = &args.dfu_confirmed {
            app.notify_update_confirmed(version);
            app.advance_animations(InputClock(500_000));
        }
        // `--sensors-screen` (SE7, epic #707): after the script lands on the Sensors screen (or its
        // scan list), push the per-slot status + the canned scan-hit set — the fake central manager,
        // so the three-row screen reads Connected · 78 % / Searching / Not set and the scan list shows
        // filtered hits. The row screen ignores the hits; the scan-list screen ignores the status.
        if args.sensors_screen {
            let status = [
                obc_app::SensorStatus { phase: obc_app::SensorPhase::Connected, battery: Some(78), last_value_ms: 0 },
                obc_app::SensorStatus { phase: obc_app::SensorPhase::Searching, battery: None, last_value_ms: 0 },
                obc_app::SensorStatus::default(),
            ];
            app.set_sensor_status(&status);
            let hits = [
                obc_app::SensorScanHit::new(0, 1, [0x66, 0x55, 0x44, 0x33, 0x22, 0x11], "HRM-Dual", -58),
                obc_app::SensorScanHit::new(0, 1, [0x21, 0x43, 0x65, 0x87, 0xA9, 0xCB], "Forerunner", -74),
                obc_app::SensorScanHit::new(1, 0, [0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F], "Stages LR", -67),
                obc_app::SensorScanHit::new(2, 0, [0x01, 0x02, 0x03, 0x04, 0x05, 0x06], "", -80),
            ];
            app.set_sensor_scan_hits(&hits);
        }

        // The script may have loaded a route; open its geometry for the Map.
        store.sync_active(app.activity.active_route);
        let route_src = store.active_source();
        let route_index = route_src.as_ref().and_then(|s| RouteIndex::read(s).ok());
        let route = match (route_index.as_ref(), route_src.as_ref()) {
            (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
            _ => None,
        };
        // An open Route overview wants the route's decimated shape preview (#678 rework 3's
        // track/elevation pager): decimate the just-opened geometry once — the cue is false again
        // the moment the copy is in, mirroring the board's ride-loop fill and the GUI's per-frame one.
        if app.nav_preview_missing() {
            if let Some(r) = route.as_ref() {
                let pts = r.preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>();
                app.set_nav_preview(&pts);
            }
        }

        let mut tracks = TrackStore::open(args.tracks_dir());

        // Replay the track from the start up to `--at`, ticking the app each step so the
        // map-matcher locks on and the ride accumulators + breadcrumb fill. A coarse-but-
        // bounded step keeps long tracks fast while staying under the dropout/teleport gates.
        if let Some(p) = player.as_mut() {
            let mut baro = BaroSensor::new();
            p.seek(0.0);
            p.play();
            let step = (replay_to / 400.0).clamp(1.0, 8.0);
            let mut t = 0.0;
            // `--fail-track` (issue #11): feed the app a sink whose every write fails, as if the card
            // were pulled mid-ride, so a logged fix drives the real `record → Err → recording-error
            // card` path. Kept out of the store so the live log (and `--save-track`) still work.
            let mut fail_sink = FailTrackSink;
            while t < replay_to {
                reconcile_tracks(&mut app, &mut tracks);
                let sink: Option<&mut dyn TrackSink> =
                    if args.fail_track { Some(&mut fail_sink) } else { tracks.sink() };
                replay_step(&mut app, p, &mut baro, None, step, route.as_ref(), sink, ReplaySensors::default());
                t += step;
            }
        }

        // `--sensors-demo` (epic #707, SE5): one final tick fed a **fixed synthetic** HR/power/
        // cadence through SE2's HAL sensor traits, so the three new stat tiles render live values in
        // the Statistics-grid snapshot (the grid was pinned to HR/PWR/RPM in the settings seed
        // above). Stamped at the replay's own `now_ms` so `Activity`'s 5 s staleness gate reads them
        // fresh. Deliberately minimal — SE8 replaces this with the sim control panel's real sliders.
        if args.sensors_demo {
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
                let now_ms = (p.time() * 1000.0) as u32;
                let (mut hr, mut power, mut cadence) = (DemoHr, DemoPower, DemoCadence);
                let sensors = obc_ports::Sensors {
                    loc: p,
                    altimeter: None,
                    temperature: None,
                    clock: None,
                    compass: None,
                    track: None,
                    fuel: None,
                    hr: Some(&mut hr),
                    power: Some(&mut power),
                    cadence: Some(&mut cadence),
                };
                app.tick(obc_ports::RideClock(now_ms), sensors, route.as_ref());
            }
        }

        // `--open-climb` (epic #506, C4): swap the base riding view for the Climb screen now the
        // replay has driven the matcher onto a climb, so the snapshot captures the striped profile.
        // C5 makes it reachable by gesture; until then this debug seam is the only way in.
        if args.open_climb {
            app.debug_open_climb();
        }

        // `--save-track`: finalise the active ride to a `.gpx` (verifies the save loop).
        if args.save_track {
            if tracks.is_recording() {
                app.activity.request_track(TrackAction::Save);
                app.activity.end_session();
                reconcile_tracks(&mut app, &mut tracks);
            } else {
                eprintln!("--save-track: no active ride (start a route first, e.g. --boot --script ppp)");
            }
        }

        let mut fb = Framebuffer::new(args.width, args.height);
        let tc = args.true_color;

        // The standalone boot-fault screen (issue #504): drawn *without* the app — at boot there may
        // be no map to build one around — so it bypasses `render_frame`, exactly as `main` does at
        // the fatal SD/map sites.
        if let Some(fault) = args.boot_fault {
            obc_app::draw_boot_fault(&mut fb, args.width as i32, args.height as i32, |c| color_of(c, tc), fault);
            if let Err(e) = write_png(&fb, args.scale, path) {
                eprintln!("{e}");
                std::process::exit(1);
            }
            eprintln!("wrote {path}");
            return;
        }

        // Time the whole frame draw into `render_us` (the no_std renderer has no clock, so
        // the host fills it) — same field the live panel shows.
        let t0 = Instant::now();
        let mut stats =
            app.render_frame(&mut fb, &reader, route.as_ref(), args.width as f32, args.height as f32, |c| {
                color_of(c, tc)
            });
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
        // TEMP debug (scratch-budget investigation): span/point/ring scratch split by render path.
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
        eprintln!("wrote {path}");
        return;
    }

    // Interactive: hand the map to the eframe host window.
    if let Err(e) = gui::run(bytes, args) {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}
