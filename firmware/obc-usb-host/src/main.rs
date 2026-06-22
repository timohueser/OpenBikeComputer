//! OBC USB feeder — the bench stand-in for real GPS / altimeter / compass hardware (issue #38).
//!
//! The prototype has no sensors (and never a good fix indoors), so this little desktop app
//! replays a recorded `.gpx` out over the device's USB-CDC debug link as fake fixes: it drives
//! the **same** [`GpxPlayer`]/[`BaroSensor`] the simulator uses (deriving course/speed from
//! motion, throttled to ~1 Hz), so a real recorded ride moves the rider on-device exactly as it
//! does in the sim. A compass slider sets the heading the device shows when stopped, a button row
//! injects encoder/Back input (taps + holds) so the UI is drivable without touching the hardware,
//! and a readout shows the device's render-stats telemetry. It's the host twin of the sim's
//! control panel, pointed at real glass instead of the in-process app.
//!
//! Wire format (see `obc-platform::debug_usb`): host→device `F <lat> <lon> <course|-> <speed|->`,
//! `A <m>`, `C <deg>`, and input injection `K t <n>` / `K e <d|u>` / `K b <d|u>`; device→host
//! `T <frame_us> <lod> <feat_drawn> <feat_tried> <feat_dropped> <chunks> <hits> <misses> <reads>
//! <bytes>`. ASCII, newline-terminated.
//!
//! Usage: `obc-usb-host [--gpx FILE] [--port NAME] [--baud N] [--list]`.

use std::io::{self, Read as _, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;
use obc_app::{AltimeterSource, Fix, LocationSource};
use obc_replay::{BaroSensor, GpxPlayer, Track};

/// How long a "hold" button keeps the edge down before releasing — past the device's ~500 ms
/// long-press threshold, so the recogniser fires Hold / BackHold.
const HOLD_MS: u64 = 700;

/// Device→host render-stats telemetry — the integer fields of `obc-platform::debug_usb::Telemetry`
/// (the same numbers as the RTT `map frame` log / the sim's Render Stats panel).
#[derive(Clone, Copy, Default)]
struct Telemetry {
    frame_us: u32,
    lod: u32,
    feat_drawn: u32,
    feat_tried: u32,
    feat_dropped: u32,
    chunks: u32,
    cache_hits: u32,
    cache_misses: u32,
    sd_reads: u32,
    bytes_read: u32,
}

/// Parse a `T …` telemetry line; `None` for anything else (so other device chatter is ignored).
fn parse_telemetry(line: &str) -> Option<Telemetry> {
    let mut it = line.split_ascii_whitespace();
    if it.next()? != "T" {
        return None;
    }
    Some(Telemetry {
        frame_us: it.next()?.parse().ok()?,
        lod: it.next()?.parse().ok()?,
        feat_drawn: it.next()?.parse().ok()?,
        feat_tried: it.next()?.parse().ok()?,
        feat_dropped: it.next()?.parse().ok()?,
        chunks: it.next()?.parse().ok()?,
        cache_hits: it.next()?.parse().ok()?,
        cache_misses: it.next()?.parse().ok()?,
        sd_reads: it.next()?.parse().ok()?,
        bytes_read: it.next()?.parse().ok()?,
    })
}

/// Format a GPS fix as an `F` line. Missing course/speed (a standstill) become the `-` sentinel,
/// so the field stays positional — mirroring `obc-platform::debug_usb::parse_line`.
fn fix_line(f: &Fix) -> String {
    let course = f.course.map_or_else(|| "-".to_string(), |c| format!("{c:.1}"));
    let speed = f.speed_mps.map_or_else(|| "-".to_string(), |s| format!("{s:.2}"));
    format!("F {} {} {} {}\n", f.lat, f.lon, course, speed)
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
fn spawn_reader(
    mut port: Box<dyn serialport::SerialPort>,
    tx: mpsc::Sender<HostEvent>,
    stop: Arc<AtomicBool>,
) {
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
    serialport::available_ports()
        .map(|ports| ports.into_iter().map(|p| p.port_name).collect())
        .unwrap_or_default()
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
        viewport: egui::ViewportBuilder::default()
            .with_title("OBC USB Feeder")
            .with_inner_size([460.0, 720.0]),
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
    // compass slider (heading the device shows when stopped); `last_sent` throttles `C` lines
    compass_deg: f32,
    last_compass_sent: Option<f32>,
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
            compass_deg: 0.0,
            last_compass_sent: None,
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
                self.gpx_label = Some(format!(
                    "{name} — {} pts, {}",
                    player.point_count(),
                    fmt_clock(player.duration())
                ));
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
        if self.conn.is_none() {
            return;
        }
        let Some(player) = self.player.as_mut() else { return };
        if !player.is_playing() {
            return;
        }
        player.advance(dt);
        if let Some(mut fix) = player.poll() {
            // GpxPlayer derives speed in *playback* time (the track's real speed), but at >1× the
            // device sees the positions arrive faster and derives a higher average — so scale the
            // reported instantaneous speed by the multiplier too. Then the device's KPH and AVG KPH
            // move together, as on a real GPS where the reported speed agrees with the motion.
            if let Some(s) = fix.speed_mps {
                fix.speed_mps = Some(s * player.speed());
            }
            self.pending.push(fix_line(&fix));
        }
        self.baro.feed(player.elevation_at(player.time()), player.time());
        if let Some(alt) = self.baro.poll() {
            self.pending.push(format!("A {alt:.2}\n"));
        }
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
        self.pending_ups
            .push((Instant::now() + Duration::from_millis(HOLD_MS), format!("K {k} u")));
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
        let connected = self.conn.is_some();

        egui::CentralPanel::default().show(ctx, |ui| {
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

            // --- Input injection (drive the device's encoder + Back remotely) ---
            full_group(ui, |ui| {
                ui.label(egui::RichText::new("Input (encoder + Back)").strong());
                ui.add_enabled_ui(connected, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("◀ Prev").clicked() {
                            self.key("K t -1");
                        }
                        if ui.button("Next ▶").clicked() {
                            self.key("K t 1");
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Select (tap)").clicked() {
                            self.tap('e');
                        }
                        if ui.button("Select (hold)").clicked() {
                            self.hold('e');
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

        // After the UI: queue compass-on-change, advance the replay, release any due holds, flush.
        self.queue_compass();
        let dt = ctx.input(|i| i.stable_dt) as f64;
        self.step_playback(dt);
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
