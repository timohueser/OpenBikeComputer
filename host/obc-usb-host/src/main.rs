//! OBC USB feeder — the bench stand-in for real GPS / altimeter / compass hardware.
//!
//! Replays a recorded `.gpx` over the device's USB-CDC debug link as fake fixes, driving the same
//! [`GpxPlayer`]/[`BaroSensor`] the simulator uses (deriving course/speed from motion, throttled to
//! ~1 Hz), or emits a stationary fix at entered/searched coordinates. A compass slider sets the
//! stopped heading, a button row injects the four buttons' input (taps + holds) so the UI is
//! drivable without the hardware, and a readout shows render-stats telemetry.
//!
//! Wire format (see `obc-platform::debug_link`): host→device `F <lat> <lon> <course|-> <speed|->`,
//! `A <m>`, `C <deg>`, `H <bpm>` / `P <watts>` / `R <rpm>` (fake BLE sensor injection, epic #707
//! SE8), `Z <mpp>` (set the map's exact meters-per-pixel — the render-benchmark hook), and input
//! injection `K t <n>` / `K s <d|u>` / `K b <d|u>`; device→host
//! `T <frame_us> <lod> <feat_drawn> <feat_tried> <feat_dropped> <chunks> <hits> <misses> <reads>
//! <bytes> <collect_us> <read_us> <sort_us> <draw_us> <overlay_us> <mpp_milli>` — the last six are
//! the per-stage render breakdown + the frame's camera scale. ASCII, newline-terminated.
//!
//! Usage: `obc-usb-host [--gpx FILE] [--port NAME] [--baud N] [--list]`.
//!
//! This crate is *only* that bench window: the manual USB control panel plus the telemetry readout.
//! It moves no map data. Putting a volume set on a device — the `OBCA_Spec.md` §5.3 digest checks
//! and the §5.4 send order — is implemented once, in the builder's TypeScript control plane
//! (`builder/app/src/lib/usb/`, which both the browser and the desktop app drive; the desktop's
//! native `usb::sendfile` moves bytes only) and on the device's receive side. This feeder carries no
//! second copy of those rules.

use std::collections::HashMap;
use std::io::{self, Read as _, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;
use obc_ports::{AltimeterSource, Fix, LocationSource};
// The canonical USB-CDC codec, authored once on the device side, so the two halves of the protocol
// can't drift: device→host `Telemetry`/`parse_telemetry` + host→device `format_fix` `F`-line
// encoder. DEFAULT features only, so the pure codec is pulled without embassy-sync.
use obc_platform::debug_link::{format_cadence, format_fix, format_hr, format_power, parse_telemetry, Telemetry};
use obc_replay::{effort_from_speed, BaroSensor, GpxPlayer, Track};
use serde::Deserialize;

/// How long a "hold" button keeps the edge down before releasing — comfortably past the device's
/// long-press threshold, so the recogniser fires Hold / BackHold. Derived from the app's own
/// [`obc_app::DEFAULT_HOLD_MS`] so it can't drift if that threshold is ever retuned.
const HOLD_MS: u64 = obc_app::DEFAULT_HOLD_MS as u64 + 200;

/// How often to emit a synthetic `H`/`P`/`R` sensor sample while enabled — the ~1 Hz cadence a real
/// BLE sensor notifies at, comfortably inside the device's 5 s staleness window.
const SENSOR_PERIOD: Duration = Duration::from_millis(1000);

/// Stationary fixes stay fresh on-device only when they keep arriving, just like a real receiver.
const FIXED_FIX_PERIOD: Duration = Duration::from_secs(1);

const DEFAULT_GEOCODER_URL: &str = "https://nominatim.openstreetmap.org/search";
const GEOCODER_USER_AGENT: &str =
    concat!("OpenBikeComputer-USB-Feeder/", env!("CARGO_PKG_VERSION"), " github.com/timohueser/OpenBikeComputer");

/// The synthetic-sensor control state (mirrors `obc-sim`'s `SensorConfig`): per-quantity enable +
/// fixed-value slider, plus the *effort follows speed* switch (when set, all three are synthesized
/// from the replayed speed and the individual toggles/sliders are ignored).
struct SensorPanel {
    hr_enabled: bool,
    power_enabled: bool,
    cadence_enabled: bool,
    hr_bpm: u16,
    power_w: u16,
    cadence_rpm: u8,
    effort_follows_speed: bool,
}

impl Default for SensorPanel {
    fn default() -> Self {
        SensorPanel {
            hr_enabled: false,
            power_enabled: false,
            cadence_enabled: false,
            hr_bpm: 140,
            power_w: 200,
            cadence_rpm: 85,
            effort_follows_speed: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct PlaceResult {
    name: String,
    lat: f64,
    lon: f64,
}

#[derive(Deserialize)]
struct PlaceWire {
    display_name: String,
    lat: String,
    lon: String,
}

type PlaceSearch = Result<Vec<PlaceResult>, String>;

/// Manual position control for weather and other location-dependent bench tests. Search is
/// deliberately click-driven (never autocomplete), cached for the process lifetime, and runs off
/// the UI thread. The endpoint can be replaced without rebuilding through `OBC_GEOCODER_URL`.
#[derive(Default)]
struct FixedLocationPanel {
    query: String,
    lat: String,
    lon: String,
    enabled: bool,
    last_sent: Option<Instant>,
    results: Vec<PlaceResult>,
    search: Option<(String, Receiver<PlaceSearch>)>,
    cache: HashMap<String, Vec<PlaceResult>>,
    search_status: Option<String>,
}

impl FixedLocationPanel {
    fn start_search(&mut self, ctx: &egui::Context) {
        let query = self.query.trim();
        if query.is_empty() || self.search.is_some() {
            return;
        }
        let key = query.to_lowercase();
        if let Some(results) = self.cache.get(&key) {
            self.results = results.clone();
            self.search_status = Some(search_summary(&self.results));
            return;
        }

        let query = query.to_string();
        let (tx, rx) = mpsc::channel();
        let repaint = ctx.clone();
        self.search = Some((key, rx));
        self.search_status = Some("searching…".to_string());
        thread::spawn(move || {
            let result = search_places(&query);
            let _ = tx.send(result);
            repaint.request_repaint();
        });
    }

    fn poll_search(&mut self) {
        let Some((key, rx)) = &self.search else { return };
        let Ok(result) = rx.try_recv() else { return };
        let key = key.clone();
        self.search = None;
        match result {
            Ok(results) => {
                self.search_status = Some(search_summary(&results));
                self.cache.insert(key, results.clone());
                self.results = results;
            }
            Err(error) => {
                self.search_status = Some(error);
                self.results.clear();
            }
        }
    }

    fn fix(&self) -> Result<Fix, String> {
        parse_fixed_fix(&self.lat, &self.lon)
    }

    fn select(&mut self, result: &PlaceResult) {
        self.lat = format!("{:.6}", result.lat);
        self.lon = format!("{:.6}", result.lon);
        self.query = result.name.clone();
        self.last_sent = None;
    }
}

fn search_summary(results: &[PlaceResult]) -> String {
    match results.len() {
        0 => "no places found".to_string(),
        1 => "1 place found".to_string(),
        n => format!("{n} places found"),
    }
}

fn search_places(query: &str) -> PlaceSearch {
    let endpoint = std::env::var("OBC_GEOCODER_URL").unwrap_or_else(|_| DEFAULT_GEOCODER_URL.to_string());
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .user_agent(GEOCODER_USER_AGENT)
        .build();
    let agent: ureq::Agent = config.into();
    let mut response = agent
        .get(&endpoint)
        .query("q", query)
        .query("format", "jsonv2")
        .query("limit", "3")
        .call()
        .map_err(|error| format!("place search failed: {error}"))?;
    let body = response
        .body_mut()
        .with_config()
        .limit(64 * 1024)
        .read_to_string()
        .map_err(|error| format!("place search response: {error}"))?;
    decode_place_results(&body)
}

fn decode_place_results(body: &str) -> PlaceSearch {
    serde_json::from_str::<Vec<PlaceWire>>(body)
        .map_err(|error| format!("place search response: {error}"))?
        .into_iter()
        .map(|wire| {
            let lat = wire.lat.parse::<f64>().map_err(|_| "place search returned a bad latitude".to_string())?;
            let lon = wire.lon.parse::<f64>().map_err(|_| "place search returned a bad longitude".to_string())?;
            Ok(PlaceResult { name: wire.display_name, lat, lon })
        })
        .collect()
}

fn parse_fixed_fix(lat: &str, lon: &str) -> Result<Fix, String> {
    let lat = parse_degrees("latitude", lat, -90.0, 90.0)?;
    let lon = parse_degrees("longitude", lon, -180.0, 180.0)?;
    Ok(Fix::at((lat * 1_000_000.0).round() as i32, (lon * 1_000_000.0).round() as i32))
}

fn parse_degrees(label: &str, value: &str, min: f64, max: f64) -> Result<f64, String> {
    let value = value.trim().parse::<f64>().map_err(|_| format!("enter a valid {label}"))?;
    if !value.is_finite() || !(min..=max).contains(&value) {
        return Err(format!("{label} must be between {min} and {max}"));
    }
    Ok(value)
}

/// What the serial reader thread sends up to the UI.
enum HostEvent {
    Telemetry(Telemetry),
    Line(String),
    Disconnected(String),
}

/// A live serial connection: the write handle (UI thread) plus the reader thread's channel and
/// its stop flag. Dropping it signals the reader to exit and closes the port.
struct Connection {
    name: String,
    port: Box<dyn serialport::SerialPort>,
    events: Receiver<HostEvent>,
    stop: Arc<AtomicBool>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Open `name` and spawn the line-reader thread (reads a clone of the port, parses telemetry, and
/// forwards events to the UI). The original handle stays for writes.
fn connect(name: &str, baud: u32) -> Result<Connection, String> {
    let port = serialport::new(name, baud)
        .timeout(Duration::from_millis(100))
        .open()
        .map_err(|e| format!("open {name}: {e}"))?;
    let reader = port.try_clone().map_err(|e| format!("clone {name}: {e}"))?;
    let (tx, events) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    spawn_reader(reader, tx, stop.clone());
    Ok(Connection { name: name.to_string(), port, events, stop })
}

/// The reader thread: accumulate bytes into `\n`-terminated lines, surface telemetry + raw lines,
/// and exit on a non-timeout error (unplug) or when `stop` is set.
fn spawn_reader(mut port: Box<dyn serialport::SerialPort>, tx: mpsc::Sender<HostEvent>, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        let mut acc: Vec<u8> = Vec::new();
        let mut buf = [0u8; 256];
        while !stop.load(Ordering::Relaxed) {
            match port.read(&mut buf) {
                Ok(0) => {}
                Ok(n) => {
                    acc.extend_from_slice(&buf[..n]);
                    while let Some(pos) = acc.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = acc.drain(..=pos).collect();
                        let s = String::from_utf8_lossy(&line);
                        let s = s.trim();
                        if s.is_empty() {
                            continue;
                        }
                        if let Some(t) = parse_telemetry(s) {
                            let _ = tx.send(HostEvent::Telemetry(t));
                        }
                        let _ = tx.send(HostEvent::Line(s.to_string()));
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::TimedOut => {}
                Err(e) => {
                    let _ = tx.send(HostEvent::Disconnected(e.to_string()));
                    break;
                }
            }
        }
    });
}

struct Args {
    gpx: Option<String>,
    port: Option<String>,
    baud: u32,
    list: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args { gpx: None, port: None, baud: 115_200, list: false };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--gpx" => a.gpx = Some(it.next().ok_or("--gpx needs a path")?),
            "--port" => a.port = Some(it.next().ok_or("--port needs a name")?),
            "--baud" => a.baud = it.next().and_then(|s| s.parse().ok()).ok_or("bad --baud")?,
            "--list" => a.list = true,
            other => return Err(format!("unexpected arg: {other}")),
        }
    }
    Ok(a)
}

fn list_ports() -> Vec<String> {
    serialport::available_ports().map(|ports| ports.into_iter().map(|p| p.port_name).collect()).unwrap_or_default()
}

fn main() -> eframe::Result<()> {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\nusage: obc-usb-host [--gpx FILE] [--port NAME] [--baud N] [--list]");
            std::process::exit(2);
        }
    };

    if args.list {
        let ports = list_ports();
        if ports.is_empty() {
            eprintln!("no serial ports found");
        } else {
            for p in ports {
                println!("{p}");
            }
        }
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("OBC USB Feeder").with_inner_size([500.0, 900.0]),
        ..Default::default()
    };
    eframe::run_native("OBC USB Feeder", options, Box::new(|_cc| Ok(Box::new(FeederApp::new(args)))))
}

struct FeederApp {
    // serial
    ports: Vec<String>,
    selected_port: Option<String>,
    baud: u32,
    conn: Option<Connection>,
    status: String,
    // replay
    player: Option<GpxPlayer>,
    gpx_label: Option<String>,
    gpx_error: Option<String>,
    baro: BaroSensor,
    fixed_location: FixedLocationPanel,
    // compass slider (heading the device shows when stopped); `last_sent` throttles `C` lines
    compass_deg: f32,
    last_compass_sent: Option<f32>,
    // Synthetic BLE sensors (epic #707 SE8): mirror the sim panel — per-quantity enable + slider,
    // plus one "effort follows speed" switch synthesizing all three from the replayed speed. Sent as
    // `H`/`P`/`R` at ~1 Hz while enabled, using the canonical `debug_link` encoders so the two halves
    // can't drift. `last_sensor_sent` throttles to the emit cadence; `last_speed_mps` is the most
    // recent (multiplier-scaled) fix speed the effort synth reads; `sensor_phase` walks the wobble.
    sensors: SensorPanel,
    last_sensor_sent: Option<Instant>,
    last_speed_mps: f32,
    sensor_phase: u32,
    // telemetry + log
    telemetry: Option<Telemetry>,
    log: std::collections::VecDeque<String>,
    // outgoing lines queued during a frame, flushed to the port after the UI closure
    pending: Vec<String>,
    // scheduled button-up edges for "hold" presses: (when, line) without the trailing `\n`
    pending_ups: Vec<(Instant, String)>,
}

impl FeederApp {
    fn new(args: Args) -> Self {
        let mut app = FeederApp {
            ports: list_ports(),
            selected_port: None,
            baud: args.baud,
            conn: None,
            status: "not connected".to_string(),
            player: None,
            gpx_label: None,
            gpx_error: None,
            baro: BaroSensor::new(),
            fixed_location: FixedLocationPanel::default(),
            compass_deg: 0.0,
            last_compass_sent: None,
            sensors: SensorPanel::default(),
            last_sensor_sent: None,
            last_speed_mps: 0.0,
            sensor_phase: 0,
            telemetry: None,
            log: std::collections::VecDeque::new(),
            pending: Vec::new(),
            pending_ups: Vec::new(),
        };
        app.selected_port = args.port.clone().or_else(|| app.ports.first().cloned());
        if let Some(path) = &args.gpx {
            app.load_gpx(std::path::Path::new(path));
        }
        if args.port.is_some() {
            app.toggle_connection();
        }
        app
    }

    fn load_gpx(&mut self, path: &std::path::Path) {
        match Track::load(path) {
            Ok(track) => {
                let player = GpxPlayer::new(track);
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("track");
                self.gpx_label =
                    Some(format!("{name} — {} pts, {}", player.point_count(), fmt_clock(player.duration())));
                self.gpx_error = None;
                self.player = Some(player);
                self.baro = BaroSensor::new();
            }
            Err(e) => {
                self.gpx_error = Some(e);
                self.player = None;
            }
        }
    }

    fn toggle_connection(&mut self) {
        if self.conn.is_some() {
            self.conn = None;
            self.status = "disconnected".to_string();
            return;
        }
        let Some(name) = self.selected_port.clone() else {
            self.status = "no port selected".to_string();
            return;
        };
        match connect(&name, self.baud) {
            Ok(c) => {
                self.status = format!("connected: {}", c.name);
                self.conn = Some(c);
                self.last_compass_sent = None; // re-send compass on (re)connect
            }
            Err(e) => self.status = format!("error: {e}"),
        }
    }

    fn drain_events(&mut self) {
        let mut disconnected: Option<String> = None;
        if let Some(conn) = &self.conn {
            while let Ok(ev) = conn.events.try_recv() {
                match ev {
                    HostEvent::Telemetry(t) => self.telemetry = Some(t),
                    HostEvent::Line(l) => push_log(&mut self.log, format!("« {l}")),
                    HostEvent::Disconnected(e) => disconnected = Some(e),
                }
            }
        }
        if let Some(e) = disconnected {
            self.conn = None;
            self.status = format!("link lost: {e}");
        }
    }

    /// Advance the replay one frame and queue the resulting fix/altitude lines (mirrors the sim's
    /// `replay_step`, emitting over serial instead of ticking the app).
    fn step_playback(&mut self, dt: f64) {
        if self.conn.is_none() || self.fixed_location.enabled {
            return;
        }
        let Some(player) = self.player.as_mut() else { return };
        if !player.is_playing() {
            return;
        }
        player.advance(dt);
        if let Some(mut fix) = player.poll() {
            // GpxPlayer derives speed in *playback* time, but at >1× the device sees positions
            // arrive faster and derives a higher average — so scale the reported instantaneous speed
            // by the multiplier too, keeping the device's KPH and AVG KPH in agreement.
            if let Some(s) = fix.speed_mps {
                let scaled = s * player.speed();
                fix.speed_mps = Some(scaled);
                // Stash the scaled speed the device sees, so the effort synth's HR/power/cadence
                // agree with the KPH the device displays.
                self.last_speed_mps = scaled;
            }
            self.pending.push(format_fix(&fix).to_string());
        }
        self.baro.feed(player.elevation_at(player.time()), player.time());
        if let Some(alt) = self.baro.poll() {
            self.pending.push(format!("A {alt:.2}\n"));
        }
    }

    /// Keep a stationary manual coordinate fresh on-device at the same ~1 Hz cadence as GPS.
    fn step_fixed_location(&mut self) {
        if self.conn.is_none() || !self.fixed_location.enabled {
            return;
        }
        let now = Instant::now();
        if self.fixed_location.last_sent.is_some_and(|last| now.duration_since(last) < FIXED_FIX_PERIOD) {
            return;
        }
        let Ok(fix) = self.fixed_location.fix() else { return };
        self.fixed_location.last_sent = Some(now);
        self.pending.push(format_fix(&fix).to_string());
    }

    /// Emit synthetic `H`/`P`/`R` sensor lines at ~1 Hz while connected and something is enabled.
    /// Uses the canonical `debug_link` encoders (so the wire format can't drift from the device
    /// parser). *Effort follows speed* synthesizes all three from the last fix speed with light
    /// noise; otherwise each quantity is sent from its slider while its toggle is on. Emitting stops
    /// the moment a toggle goes off, so the device's tile goes stale → `--` (its 5 s gate).
    fn step_sensors(&mut self) {
        if self.conn.is_none() {
            return;
        }
        let s = &self.sensors;
        let any = s.effort_follows_speed || s.hr_enabled || s.power_enabled || s.cadence_enabled;
        if !any {
            return;
        }
        let now = Instant::now();
        if self.last_sensor_sent.is_some_and(|last| now.duration_since(last) < SENSOR_PERIOD) {
            return;
        }
        self.last_sensor_sent = Some(now);

        if s.effort_follows_speed {
            let e = effort_from_speed(self.last_speed_mps, self.sensor_phase);
            self.pending.push(format_hr(e.hr_bpm).to_string());
            self.pending.push(format_power(e.power_w).to_string());
            self.pending.push(format_cadence(e.cadence_rpm).to_string());
        } else {
            if s.hr_enabled {
                self.pending.push(format_hr(s.hr_bpm).to_string());
            }
            if s.power_enabled {
                self.pending.push(format_power(s.power_w).to_string());
            }
            if s.cadence_enabled {
                self.pending.push(format_cadence(s.cadence_rpm).to_string());
            }
        }
        self.sensor_phase = self.sensor_phase.wrapping_add(1);
    }

    fn queue_compass(&mut self) {
        let send = match self.last_compass_sent {
            Some(prev) => (prev - self.compass_deg).abs() >= 1.0,
            None => true,
        };
        if send {
            self.last_compass_sent = Some(self.compass_deg);
            self.pending.push(format!("C {:.1}\n", self.compass_deg));
        }
    }

    /// Queue a raw input line (without the trailing newline).
    fn key(&mut self, line: &str) {
        self.pending.push(format!("{line}\n"));
    }

    /// A tap: down + up in the same frame → a Press / Back gesture.
    fn tap(&mut self, k: char) {
        self.key(&format!("K {k} d"));
        self.key(&format!("K {k} u"));
    }

    /// A hold: down now, up after [`HOLD_MS`] → a Hold / BackHold gesture (the recogniser fires at
    /// its threshold before the up arrives).
    fn hold(&mut self, k: char) {
        self.key(&format!("K {k} d"));
        self.pending_ups.push((Instant::now() + Duration::from_millis(HOLD_MS), format!("K {k} u")));
    }

    /// Move any scheduled button-up edges that are now due into the outgoing queue.
    fn flush_due_ups(&mut self) {
        let now = Instant::now();
        let mut due = Vec::new();
        self.pending_ups.retain(|(when, line)| {
            if *when <= now {
                due.push(line.clone());
                false
            } else {
                true
            }
        });
        for line in due {
            self.key(&line);
        }
    }

    /// Write queued lines to the port; drop the connection on a write error.
    fn flush_pending(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let lines = std::mem::take(&mut self.pending);
        let Some(conn) = self.conn.as_mut() else { return };
        for line in &lines {
            if let Err(e) = conn.port.write_all(line.as_bytes()) {
                self.status = format!("write failed: {e}");
                self.conn = None;
                return;
            }
            push_log(&mut self.log, format!("» {}", line.trim_end()));
        }
    }
}

impl eframe::App for FeederApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.fixed_location.poll_search();
        let connected = self.conn.is_some();

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
            // Let sliders span the panel width (leave room for the value box), so the replay /
            // compass rows use the full width instead of egui's narrow default.
            ui.spacing_mut().slider_width = (ui.available_width() - 96.0).max(180.0);

            ui.heading("OBC USB Feeder");
            ui.label(egui::RichText::new(&self.status).weak());
            ui.add_space(6.0);

            // --- Serial ---
            full_group(ui, |ui| {
                ui.label(egui::RichText::new("Serial").strong());
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("port")
                        .width(220.0)
                        .selected_text(self.selected_port.clone().unwrap_or_else(|| "—".into()))
                        .show_ui(ui, |ui| {
                            for p in &self.ports.clone() {
                                ui.selectable_value(&mut self.selected_port, Some(p.clone()), p);
                            }
                        });
                    if ui.add_enabled(!connected, egui::Button::new("⟳")).clicked() {
                        self.ports = list_ports();
                    }
                    if ui.button(if connected { "Disconnect" } else { "Connect" }).clicked() {
                        self.toggle_connection();
                    }
                });
            });
            ui.add_space(6.0);

            // --- Fixed GPS location ---
            full_group(ui, |ui| {
                ui.label(egui::RichText::new("Fixed GPS location").strong());
                ui.horizontal(|ui| {
                    let query = ui.add(
                        egui::TextEdit::singleline(&mut self.fixed_location.query)
                            .hint_text("Search a town or address")
                            .desired_width(300.0),
                    );
                    let submit = query.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    let can_search = !self.fixed_location.query.trim().is_empty()
                        && self.fixed_location.search.is_none();
                    if ui.add_enabled(can_search, egui::Button::new("Search")).clicked() || submit {
                        self.fixed_location.start_search(ctx);
                    }
                });
                if let Some(status) = &self.fixed_location.search_status {
                    ui.weak(status);
                }
                egui::ScrollArea::vertical().max_height(110.0).show(ui, |ui| {
                    for result in self.fixed_location.results.clone() {
                        if ui
                            .button(format!("{}\n{:.5}, {:.5}", result.name, result.lat, result.lon))
                            .clicked()
                        {
                            self.fixed_location.select(&result);
                            self.fixed_location.results.clear();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Lat");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.fixed_location.lat)
                            .hint_text("48.137154")
                            .desired_width(120.0),
                    );
                    ui.label("Lon");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.fixed_location.lon)
                            .hint_text("11.576124")
                            .desired_width(120.0),
                    );
                });
                let valid = self.fixed_location.fix();
                if valid.is_err() && self.fixed_location.enabled {
                    self.fixed_location.enabled = false;
                    self.fixed_location.last_sent = None;
                }
                let mut enabled = self.fixed_location.enabled;
                if ui
                    .add_enabled(valid.is_ok(), egui::Checkbox::new(&mut enabled, "Send stationary fix every second"))
                    .changed()
                {
                    self.fixed_location.enabled = enabled;
                    self.fixed_location.last_sent = None;
                }
                if let Err(error) = valid {
                    ui.weak(error);
                } else if self.fixed_location.enabled {
                    ui.label(
                        egui::RichText::new("Fixed location active — GPX replay is paused")
                            .color(egui::Color32::from_rgb(80, 180, 110)),
                    );
                }
                ui.horizontal(|ui| {
                    ui.weak("Place search ©");
                    ui.hyperlink_to("OpenStreetMap contributors", "https://www.openstreetmap.org/copyright");
                });
            });
            ui.add_space(6.0);

            // --- GPX replay ---
            full_group(ui, |ui| {
                ui.label(egui::RichText::new("GPX replay").strong());
                if ui.button("Load GPX…").clicked() {
                    if let Some(path) =
                        rfd::FileDialog::new().add_filter("GPX track", &["gpx"]).pick_file()
                    {
                        self.load_gpx(&path);
                    }
                }
                if let Some(err) = &self.gpx_error {
                    ui.colored_label(egui::Color32::from_rgb(220, 80, 80), err);
                }
                if let Some(label) = &self.gpx_label {
                    ui.label(label);
                }
                if let Some(player) = self.player.as_mut() {
                    let dur = player.duration();
                    if ui
                        .button(if player.is_playing() { "⏸ Pause" } else { "▶ Play" })
                        .clicked()
                    {
                        player.toggle();
                    }
                    if dur > 0.0 {
                        let mut t = player.time();
                        if ui
                            .add(egui::Slider::new(&mut t, 0.0..=dur).show_value(false).text("seek"))
                            .changed()
                        {
                            player.seek(t);
                        }
                        ui.label(format!("{} / {}", fmt_clock(player.time()), fmt_clock(dur)));
                        let mut speed = player.speed();
                        if ui.add(egui::Slider::new(&mut speed, 1.0..=10.0).suffix("×")).changed() {
                            player.set_speed(speed);
                        }
                        ui.label(
                            egui::RichText::new("speed-up rides faster on-device — KPH + AVG KPH scale together; at very high × the implied jump can exceed the device's glitch filter and drop fixes")
                                .weak()
                                .size(10.0),
                        );
                    }
                }
            });
            ui.add_space(6.0);

            // --- Compass (heading when stopped) ---
            full_group(ui, |ui| {
                ui.label(egui::RichText::new("Compass (heading when stopped)").strong());
                ui.add(
                    egui::Slider::new(&mut self.compass_deg, 0.0..=360.0).suffix("°").step_by(1.0),
                );
            });
            ui.add_space(6.0);

            // --- Synthetic BLE sensors (HR / power / cadence, epic #707 SE8) ---
            full_group(ui, |ui| {
                ui.label(egui::RichText::new("Sensors (HR / Power / Cadence)").strong());
                let s = &mut self.sensors;
                ui.checkbox(&mut s.effort_follows_speed, "Effort follows speed");
                ui.label(
                    egui::RichText::new("synthesize all three from the replayed GPX speed (with light noise)")
                        .weak()
                        .size(10.0),
                );
                ui.add_enabled_ui(!s.effort_follows_speed, |ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut s.hr_enabled, "HR");
                        ui.add(egui::Slider::new(&mut s.hr_bpm, 40..=220).suffix(" bpm"));
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut s.power_enabled, "Power");
                        ui.add(egui::Slider::new(&mut s.power_w, 0..=1000).suffix(" W"));
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut s.cadence_enabled, "Cadence");
                        ui.add(egui::Slider::new(&mut s.cadence_rpm, 0..=130).suffix(" rpm"));
                    });
                });
            });
            ui.add_space(6.0);

            // --- Input injection (drive the device's four buttons remotely) ---
            full_group(ui, |ui| {
                ui.label(egui::RichText::new("Input (Up/Down · Select · Back)").strong());
                ui.add_enabled_ui(connected, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("▲ Up").clicked() {
                            self.key("K t -1");
                        }
                        if ui.button("▼ Down").clicked() {
                            self.key("K t 1");
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Select (tap)").clicked() {
                            self.tap('s');
                        }
                        if ui.button("Select (hold)").clicked() {
                            self.hold('s');
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Back (tap)").clicked() {
                            self.tap('b');
                        }
                        if ui.button("Back (hold)").clicked() {
                            self.hold('b');
                        }
                    });
                });
            });
            ui.add_space(6.0);

            // --- Render-stats telemetry (device → host) ---
            full_group(ui, |ui| {
                ui.label(egui::RichText::new("Render stats (device → host)").strong());
                match self.telemetry {
                    Some(t) => {
                        egui::Grid::new("telemetry").num_columns(2).spacing([12.0, 2.0]).show(
                            ui,
                            |ui| {
                                row(ui, "Frame", &format!("{:.2} ms", t.frame_us as f32 / 1000.0));
                                row(ui, "Scale", &format!("{:.3} m/px", t.mpp_milli as f32 / 1000.0));
                                // Per-stage breakdown (render benchmark): `collect_us` includes
                                // `read_us`, so show the CPU part of collect separately. `setup`
                                // (Reader::new) is whatever the frame total has over the stages.
                                let ms = |us: u32| format!("{:.2} ms", us as f32 / 1000.0);
                                let stages = t.collect_us + t.sort_us + t.draw_us + t.overlay_us;
                                row(ui, "· read (SD)", &ms(t.read_us));
                                row(ui, "· collect-cpu", &ms(t.collect_us.saturating_sub(t.read_us)));
                                row(ui, "· sort", &ms(t.sort_us));
                                row(ui, "· draw", &ms(t.draw_us));
                                row(ui, "· overlay", &ms(t.overlay_us));
                                row(ui, "· setup", &ms(t.frame_us.saturating_sub(stages)));
                                row(ui, "LOD", &t.lod.to_string());
                                row(ui, "Features", &format!("{} / {} drawn", t.feat_drawn, t.feat_tried));
                                ui.label("Dropped");
                                let c = if t.feat_dropped > 0 {
                                    egui::Color32::from_rgb(220, 80, 80)
                                } else {
                                    ui.visuals().text_color()
                                };
                                ui.colored_label(c, t.feat_dropped.to_string());
                                ui.end_row();
                                row(ui, "Chunks", &t.chunks.to_string());
                                let reqs = t.cache_hits + t.cache_misses;
                                let hit = if reqs == 0 { 0.0 } else { 100.0 * t.cache_hits as f32 / reqs as f32 };
                                row(ui, "Map cache", &format!("{hit:.0}% hit · {} rd · {} B", t.sd_reads, t.bytes_read));
                            },
                        );
                    }
                    None => {
                        ui.weak("waiting for telemetry…");
                    }
                }
            });
            ui.add_space(6.0);

            ui.collapsing("Wire log", |ui| {
                egui::ScrollArea::vertical().max_height(140.0).stick_to_bottom(true).show(ui, |ui| {
                    for line in &self.log {
                        ui.label(egui::RichText::new(line).monospace().size(11.0));
                    }
                });
            });
            });
        });

        // After the UI: queue compass-on-change, emit one location source, sensors, releases, flush.
        self.queue_compass();
        let dt = ctx.input(|i| i.stable_dt) as f64;
        self.step_fixed_location();
        self.step_playback(dt);
        self.step_sensors();
        self.flush_due_ups();
        self.flush_pending();

        // Keep animating while playing (steady fix stream) or while holds are pending.
        let playing = self.player.as_ref().is_some_and(|p| p.is_playing());
        if playing || !self.pending_ups.is_empty() || self.conn.is_some() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
}

/// A group that fills the panel width (so sections don't shrink-wrap their content).
fn full_group(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.group(|ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
}

/// A two-column telemetry row.
fn row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(label);
    ui.label(value);
    ui.end_row();
}

/// `M:SS` clock for the seek readout.
fn fmt_clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Append to the rolling wire log, capping its length.
fn push_log(log: &mut std::collections::VecDeque<String>, line: String) {
    const MAX: usize = 200;
    log.push_back(line);
    while log.len() > MAX {
        log.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_coordinates_become_stationary_microdegree_fix() {
        assert_eq!(parse_fixed_fix("48.137154", "11.576124"), Ok(Fix::at(48_137_154, 11_576_124)));
        assert_eq!(parse_fixed_fix(" -33.8688 ", "151.2093"), Ok(Fix::at(-33_868_800, 151_209_300)));
    }

    #[test]
    fn fixed_coordinate_ranges_are_checked() {
        assert_eq!(parse_fixed_fix("91", "0").unwrap_err(), "latitude must be between -90 and 90");
        assert_eq!(parse_fixed_fix("0", "-181").unwrap_err(), "longitude must be between -180 and 180");
        assert_eq!(parse_fixed_fix("north", "0").unwrap_err(), "enter a valid latitude");
    }

    #[test]
    fn nominatim_results_decode_to_typed_places() {
        let json = r#"[{"display_name":"München, Bayern, Deutschland","lat":"48.1371079","lon":"11.5753822"}]"#;
        assert_eq!(
            decode_place_results(json),
            Ok(vec![PlaceResult {
                name: "München, Bayern, Deutschland".to_string(),
                lat: 48.1371079,
                lon: 11.5753822,
            }])
        );
    }
}
