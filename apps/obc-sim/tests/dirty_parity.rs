//! **The differential repaint test** (#1447): render-on-demand must be indistinguishable from
//! rendering every frame.
//!
//! Two identical devices are driven through the same replay, pass for pass. The **reference**
//! renders the whole frame every pass and is therefore always correct by construction. The
//! **candidate** renders only what its [`PassPlan`] asked for. After every single pass the two
//! composited framebuffers are compared byte for byte: a difference is an **under-redraw**, which
//! the dirty contract calls a bug, and the replay step that caused it is named in the failure.
//!
//! The candidate also counts its repaints. Over-redraw is safe but not free — a full map render is
//! tens of milliseconds on the panel — so the counts are reported and held to a ceiling: the
//! candidate must never repaint *more* often than there are passes, and the quiet stretches of the
//! replay must stay quiet.
//!
//! ## The two-buffer model, and why it is the honest one
//!
//! The panel keeps two things: the clean map frame the renderer produced, and the glass, which is
//! that frame with the transient overlay composited on top. A map repaint re-renders the clean
//! frame; an overlay repaint re-composites the bulge over the *unchanged* clean frame, and a
//! trailing overlay repaint with nothing live is what wipes the last bulge off. That is exactly what
//! the board's `present_bulge` does with its row span, so modelling it here is what makes the
//! comparison mean something.

use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Size},
    pixelcolor::Rgb888,
    prelude::*,
    primitives::Rectangle,
};
use obc_app::device_core::{
    DerivedInputs, DerivedTargets, ExternalFacts, OutcomeSlots, PassClock, PassInputs, PlatformSupport, RouteUpload,
};
use obc_app::screen::MapTransfer;
use obc_app::{App, AppState, BleLink, BleStatus, Dirty, SensorPhase, SensorStatus};
use obc_formats::io::{ByteSink, SliceSource};
use obc_ports::{
    Button, ButtonEvent, CadenceSource, Fix, FuelGauge, HeartRateSource, InputClock, InputEvent, InputSource,
    LocationSource, PowerSource, RideClock, Sensors,
};
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader};
use obc_route::{RouteIndex, RouteReader, RouteSummary};

const W: u32 = 240;
const H: u32 = 320;
/// The route's latitude and its western end — the replay walks east along it.
const LAT: f64 = 48.0;
const LON0: f64 = 7.8;

// ---------------------------------------------------------------------------------------------
// The frame buffer and the byte ports.
// ---------------------------------------------------------------------------------------------

/// A plain RGB888 frame. Comparison is over the whole buffer, so nothing about *where* a difference
/// is can hide it.
#[derive(Clone, PartialEq, Eq)]
struct Frame(Vec<Rgb888>);

impl Frame {
    fn new() -> Frame {
        Frame(vec![Rgb888::BLACK; (W * H) as usize])
    }

    /// Where the two frames first differ, and how many pixels do — the failure message's evidence.
    fn diff(&self, other: &Frame) -> Option<(usize, usize)> {
        let mut first = None;
        let mut count = 0;
        for (i, (a, b)) in self.0.iter().zip(other.0.iter()).enumerate() {
            if a != b {
                first.get_or_insert(i);
                count += 1;
            }
        }
        first.map(|i| (i, count))
    }
}

impl OriginDimensions for Frame {
    fn size(&self) -> Size {
        Size::new(W, H)
    }
}

impl DrawTarget for Frame {
    type Color = Rgb888;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            if p.x >= 0 && p.y >= 0 && (p.x as u32) < W && (p.y as u32) < H {
                self.0[(p.y as u32 * W + p.x as u32) as usize] = c;
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        for p in area.points() {
            if p.x >= 0 && p.y >= 0 && (p.x as u32) < W && (p.y as u32) < H {
                self.0[(p.y as u32 * W + p.x as u32) as usize] = color;
            }
        }
        Ok(())
    }
}

fn color_of(c: u16) -> Rgb888 {
    let (r, g, b) = rgb565_to_rgb888(c);
    Rgb888::new(r, g, b)
}

/// A `ByteSink` over a growable `Vec` — the GPX→OBCR conversion's backing.
#[derive(Default)]
struct VecSink(Vec<u8>);

impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), obc_formats::io::Error> {
        self.0.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), obc_formats::io::Error> {
        let o = off as usize;
        self.0[o..o + b.len()].copy_from_slice(b);
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// The sensor ports: plain values, fed per pass.
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct Ports {
    fix: Option<Fix>,
    hr: Option<u16>,
    power: Option<u16>,
    cadence: Option<u8>,
    battery: Option<u8>,
}

struct One<T>(Option<T>);
impl LocationSource for One<Fix> {
    fn poll(&mut self) -> Option<Fix> {
        self.0.take()
    }
}
impl HeartRateSource for One<u16> {
    fn poll(&mut self) -> Option<u16> {
        self.0.take()
    }
}
impl PowerSource for One<(u16,)> {
    fn poll(&mut self) -> Option<u16> {
        self.0.take().map(|v| v.0)
    }
}
impl CadenceSource for One<u8> {
    fn poll(&mut self) -> Option<u8> {
        self.0.take()
    }
}
impl FuelGauge for One<(u8,)> {
    fn poll(&mut self) -> Option<u8> {
        self.0.map(|v| v.0)
    }
}

/// Raw button events for one frame, drained by the shared recogniser.
struct Keys(std::collections::VecDeque<InputEvent>);
impl InputSource for Keys {
    fn poll(&mut self) -> Option<InputEvent> {
        self.0.pop_front()
    }
}

// ---------------------------------------------------------------------------------------------
// The replay.
// ---------------------------------------------------------------------------------------------

/// One replay step: what the host feeds the device before the pass, and what the ports carry.
#[derive(Default)]
struct Step {
    /// A short label naming the change category — quoted back when the frames diverge.
    what: &'static str,
    at_ms: u32,
    keys: Vec<InputEvent>,
    ports: Ports,
    /// Host seams applied to both instances identically, before the pass.
    feed: Option<fn(&mut App)>,
    /// An external fact the pass consumes at stage 2 — the door a runtime's own facts come through.
    fact: Option<fn(&mut ExternalFacts)>,
    /// The screen this step must leave on top. Every navigation in the replay states its
    /// destination, so a binding that moves elsewhere fails here instead of quietly draining the
    /// replay of the screens it exists to visit.
    expect: Option<&'static str>,
}

fn step(what: &'static str, at_ms: u32) -> Step {
    Step { what, at_ms, ..Step::default() }
}

impl Step {
    fn keys(mut self, evs: &[InputEvent]) -> Step {
        self.keys = evs.to_vec();
        self
    }
    fn fix(mut self, i: u32) -> Step {
        self.ports.fix = Some(fix_at(i));
        self
    }
    fn hr(mut self, bpm: u16) -> Step {
        self.ports.hr = Some(bpm);
        self
    }
    fn power(mut self, w: u16) -> Step {
        self.ports.power = Some(w);
        self
    }
    fn cadence(mut self, rpm: u8) -> Step {
        self.ports.cadence = Some(rpm);
        self
    }
    fn battery(mut self, pct: u8) -> Step {
        self.ports.battery = Some(pct);
        self
    }
    fn feed(mut self, f: fn(&mut App)) -> Step {
        self.feed = Some(f);
        self
    }
    fn fact(mut self, f: fn(&mut ExternalFacts)) -> Step {
        self.fact = Some(f);
        self
    }
    fn expect(mut self, screen: &'static str) -> Step {
        self.expect = Some(screen);
        self
    }
}

/// The `i`-th fix along the route, moving east at ~150 m per step with a live course and speed.
fn fix_at(i: u32) -> Fix {
    let lon = LON0 + 0.0020 * i as f64;
    Fix { lat: (LAT * 1e6) as i32, lon: (lon * 1e6) as i32, course: Some(90.0), speed_mps: Some(6.0 + i as f32 * 0.1) }
}

/// A fix well north of the route — far outside the corridor, so the matcher reports off-route.
fn fix_off_route(i: u32) -> Fix {
    let lon = LON0 + 0.0020 * i as f64;
    Fix::at(((LAT + 0.02) * 1e6) as i32, (lon * 1e6) as i32)
}

fn tap(b: Button) -> [InputEvent; 2] {
    [InputEvent::Button(ButtonEvent::Down(b)), InputEvent::Button(ButtonEvent::Up(b))]
}

// ---------------------------------------------------------------------------------------------
// The two instances.
// ---------------------------------------------------------------------------------------------

const SUPPORT: PlatformSupport = PlatformSupport {
    detour: true,
    settings_persistence: true,
    dfu: true,
    weather: true,
    bonding: true,
    storage_space_report: true,
    retention_metadata: true,
};

/// One device plus the panel it draws onto.
struct Instance {
    app: App,
    outcomes: OutcomeSlots,
    scratch: Box<obc_render::RenderScratch>,
    /// The last full map render — what the panel holds behind the transient overlay.
    clean: Frame,
    /// What is actually on glass: `clean` with this frame's overlay composited over it.
    glass: Frame,
    map_repaints: usize,
    overlay_repaints: usize,
}

impl Instance {
    fn new(app: App) -> Instance {
        Instance {
            app,
            outcomes: OutcomeSlots::new(),
            scratch: Box::new(obc_render::RenderScratch::new()),
            clean: Frame::new(),
            glass: Frame::new(),
            map_repaints: 0,
            overlay_repaints: 0,
        }
    }

    /// Run one pass with this step's input, then paint per `render`: `None` = always (the
    /// reference), `Some(plan)` = only what the plan asked for (the candidate).
    fn advance(&mut self, s: &Step, route: Option<&RouteReader<'_>>, reader: &Reader, on_demand: bool) -> Dirty {
        if let Some(feed) = s.feed {
            feed(&mut self.app);
        }
        let gestures = self.app.recognize(InputClock(s.at_ms), &mut Keys(s.keys.iter().copied().collect()));

        let mut loc = One(s.ports.fix);
        let mut hr = One(s.ports.hr);
        let mut power = One(s.ports.power.map(|w| (w,)));
        let mut cadence = One(s.ports.cadence);
        let mut fuel = One(s.ports.battery.map(|p| (p,)));
        let mut facts = ExternalFacts::NONE;
        if let Some(note) = s.fact {
            note(&mut facts);
        }
        let plan = self.app.run_pass(PassInputs {
            now: PassClock { ride: RideClock(s.at_ms), ui: InputClock(s.at_ms) },
            gestures: &gestures,
            sensors: Sensors {
                hr: Some(&mut hr),
                power: Some(&mut power),
                cadence: Some(&mut cadence),
                fuel: Some(&mut fuel),
                ..Sensors::new(&mut loc)
            },
            route,
            support: SUPPORT,
            outcomes: &mut self.outcomes,
            facts: &mut facts,
            derived: DerivedInputs::NONE,
            targets: DerivedTargets::NONE,
        });

        let render_map = !on_demand || plan.render.map;
        let render_overlay = !on_demand || plan.render.overlay;
        if render_map {
            self.map_repaints += 1;
            self.app.render_map(Some(&mut self.scratch), &mut self.clean, reader, route, W as f32, H as f32, color_of);
        }
        if render_map || render_overlay {
            if render_overlay {
                self.overlay_repaints += 1;
            }
            // The overlay always composites over the *clean* frame — a bulge that has gone quiet is
            // wiped by re-presenting the clean rows underneath it, never by painting over itself.
            self.glass = self.clean.clone();
            self.app.render_overlay(&mut self.glass, W as f32, H as f32, color_of);
        }
        plan.render
    }
}

// ---------------------------------------------------------------------------------------------
// The fixtures.
// ---------------------------------------------------------------------------------------------

/// A due-east 40-point route along 48.0000° N from 7.8000° E, with two named waypoints so the
/// next-waypoint chip has something to move between.
fn route_bytes() -> Vec<u8> {
    let mut gpx = String::from(r#"<?xml version="1.0"?><gpx version="1.1"><trk><trkseg>"#);
    for i in 0..40 {
        let lon = LON0 + 0.0020 * i as f64;
        gpx.push_str(&format!(r#"<trkpt lat="{LAT:.4}" lon="{lon:.4}"><ele>200.0</ele></trkpt>"#));
    }
    gpx.push_str("</trkseg></trk></gpx>");
    let mut sink = VecSink::default();
    obc_route::gpx_to_obcr(&SliceSource(gpx.as_bytes()), "East", &mut sink).expect("the replay route converts");
    sink.0
}

/// The smallest map that draws something: one style, one rung, one chunk of line work under the
/// route, so a camera move genuinely changes pixels.
fn map_bytes() -> Vec<u8> {
    use obcm_testkit::{build_file, pack_line16, seal, LodSpec, Style};
    const STYLES: &[Style] = &[(1, 0, 0x07E0, 3, 1, false, None)];
    let anchor = ((LON0 * 1e6) as i32, (LAT * 1e6) as i32);
    let chunk = seal(pack_line16(1, anchor.0, anchor.1, &[(2_000, 400), (2_000, -400), (2_000, 400)]), 4096);
    build_file(
        (7_000_000, 47_000_000, 9_000_000, 49_000_000),
        STYLES,
        &[LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![chunk], chunk_size: 4096 }],
    )
}

/// The catalog entry for the replay route, read from its own header — no hand-built twin. Built
/// once, because the replay's host seams are plain `fn` pointers with nothing to capture.
fn route_summary() -> RouteSummary {
    static SUMMARY: std::sync::OnceLock<RouteSummary> = std::sync::OnceLock::new();
    SUMMARY
        .get_or_init(|| RouteSummary::read(&SliceSource(&route_bytes())).expect("the replay route's header reads"))
        .clone()
}

/// The device both instances start from: the three live sensor tiles pinned to the grid — one page
/// of them, so the grid's auto-cycle never fires — because the whole point of the Statistics key is
/// that a value the rider chose to see repaints when it moves.
fn riding_device(camera: AppState) -> App {
    use obc_app::{StatField, StatFieldList};
    let mut app = App::new(camera);
    let fields =
        StatFieldList::decode(3, &[StatField::HeartRate as u8, StatField::Power as u8, StatField::Cadence as u8]);
    app.set_settings(obc_app::Settings { stat_fields: fields, ..*app.settings() });
    app
}

/// A saved, connected heart-rate sensor — the Sensors page's status row.
fn connected_hr() -> [SensorStatus; 3] {
    let mut s = [SensorStatus::default(); 3];
    s[0] = SensorStatus { phase: SensorPhase::Connected, battery: Some(78), last_value_ms: 1_000 };
    s
}

// ---------------------------------------------------------------------------------------------
// The replay itself.
// ---------------------------------------------------------------------------------------------

/// Every change category the render keys claim to cover, in one continuous ride. The clock advances
/// monotonically, so the timers (the no-fix window, the sensor staleness gate, the upload popup's
/// auto-close, the idle return) all fire inside the replay rather than being staged.
fn replay() -> Vec<Step> {
    let mut steps = vec![
        // --- the boot frame, then quiet -------------------------------------------------------
        step("boot", 0).expect("Map"),
        step("quiet after boot", 100),
        // --- the route arrives: a host seam for the catalog, an external fact for the card -----
        step("route uploaded", 500)
            .feed(|app| app.set_routes_with_ids(&[route_summary()], &[7]))
            .fact(|f| f.note_route_upload(RouteUpload { id: 7, replaced: false, elevation: None }))
            .expect("RouteReceived"),
        step("quiet with the upload card up", 800),
        step("dismiss the upload card", 1_000).keys(&tap(Button::Back)).expect("Map"),
        step("activate the route", 1_200).feed(|app| app.activate_route(0)),
        // --- start a ride, so the Map has its Statistics sibling to swap to --------------------
        step("open the start card", 1_400).keys(&tap(Button::Select)).expect("RideStart"),
        step("start the ride", 1_600).keys(&tap(Button::Select)).expect("Map"),
    ];

    // --- fix acquisition, movement, staleness and recovery ------------------------------------
    steps.push(step("fix acquisition", 2_000).fix(0));
    for i in 1..6 {
        steps.push(step("fix movement (camera, progress)", 2_000 + i * 1_000).fix(i));
    }
    steps.push(step("fix goes stale (the no-fix banner)", 12_000));
    steps.push(step("still stale", 14_000));
    steps.push(step("fix recovery", 16_000).fix(6));

    // --- off route and back --------------------------------------------------------------------
    steps.push(Step { ports: Ports { fix: Some(fix_off_route(7)), ..Ports::default() }, ..step("off route", 17_000) });
    steps.push(Step {
        ports: Ports { fix: Some(fix_off_route(8)), ..Ports::default() },
        ..step("still off route", 18_000)
    });
    steps.push(step("back on route", 19_000).fix(9));
    // Far enough along that the next waypoint and the active climb move on.
    for i in 10..16 {
        steps.push(step("route progress and the next waypoint", 19_000 + (i - 9) * 1_000).fix(i));
    }

    // --- the Statistics grid: the one screen that draws the live sensor tiles -------------------
    steps.push(step("swap to the riding grid", 26_000).keys(&tap(Button::Back)).expect("Statistics"));
    // **No fix on any of these.** A sensor sample that arrives between fixes is exactly the edge
    // the grid's key exists for: nothing else about the device moves, so if the value is not in the
    // key, the tile freezes on glass (epic #744, SR3).
    steps.push(step("heart rate arrives, no fix", 26_500).hr(120));
    steps.push(step("power arrives, no fix", 27_000).power(210));
    steps.push(step("cadence arrives, no fix", 27_500).cadence(84));
    steps.push(step("an unchanged heart rate", 28_000).hr(120));
    steps.push(step("a changed heart rate", 29_000).hr(131));
    steps.push(step("quiet on the grid", 30_000));
    // Past the 5 s staleness gate with no fresh sample: every tile blanks to `--`.
    steps.push(step("the sensor tiles go stale", 36_000));
    steps.push(step("still blank", 37_000));
    // Progress and the climb move under the grid, with the fix that carries them.
    steps.push(step("progress under the grid", 38_000).fix(16));
    steps.push(step("more progress under the grid", 39_000).fix(17));

    // --- the battery: on a riding view, and on Home ---------------------------------------------
    steps.push(step("battery read while riding", 40_000).battery(75));
    steps.push(step("battery change while riding", 71_000).battery(61));

    // --- back to the Map for the camera and overlay work ----------------------------------------
    steps.push(step("swap back to the map", 72_000).keys(&tap(Button::Back)).expect("Map"));
    steps.push(step("zoom step", 72_500).keys(&[InputEvent::Step(1)]));
    steps.push(step("zoom step back", 73_000).keys(&[InputEvent::Step(-1)]));
    steps.push(
        step("hold begins (the bulge charges)", 73_500).keys(&[InputEvent::Button(ButtonEvent::Down(Button::Select))]),
    );
    steps.push(step("the bulge animates", 73_800));
    steps.push(step("the hold fires — pan mode", 74_300));
    steps.push(step("the bulge's trailing clear", 74_600));
    steps.push(step("pan the camera", 74_900).keys(&[InputEvent::Step(1)]));
    steps.push(step("leave pan mode", 75_400).keys(&tap(Button::Back)).expect("Map"));

    // --- the freeze banner ----------------------------------------------------------------------
    steps.push(step("a planner run starts (freeze on)", 76_000).feed(|app| app.debug_set_plan_live(true)));
    steps.push(step("frozen, with fixes still arriving", 77_000).fix(18));
    steps.push(step("the planner answers (freeze off)", 78_000).feed(|app| app.debug_set_plan_live(false)));

    // --- the cards: arrival, replacement, removal ------------------------------------------------
    steps.push(
        step("a passkey card arrives", 79_000)
            .feed(|app| {
                app.set_ble_status(BleStatus { link: BleLink::Connected, paired: true, passkey: Some(123_456) })
            })
            .expect("Passkey"),
    );
    steps.push(
        step("the passkey is unchanged", 79_500).feed(|app| {
            app.set_ble_status(BleStatus { link: BleLink::Connected, paired: true, passkey: Some(123_456) })
        }),
    );
    steps.push(
        step("the passkey clears", 80_000)
            .feed(|app| app.set_ble_status(BleStatus { link: BleLink::Connected, paired: true, passkey: None }))
            .expect("Map"),
    );
    steps.push(
        step("a map transfer starts", 81_000)
            .feed(|app| app.set_map_transfer(Some(MapTransfer::Receiving { received_kib: 10, total_kib: 400 })))
            .expect("MapTransfer"),
    );
    steps.push(
        step("the transfer progresses", 82_000)
            .feed(|app| app.set_map_transfer(Some(MapTransfer::Receiving { received_kib: 200, total_kib: 400 }))),
    );
    steps.push(step("the transfer ends", 83_000).feed(|app| app.set_map_transfer(None)).expect("Map"));

    // --- the sensor settings and weather seams ----------------------------------------------------
    steps.push(step("a saved sensor connects", 84_000).feed(|app| app.set_sensor_status(&connected_hr())));
    steps
        .push(step("a scan hit appears", 85_000).feed(|app| {
            app.set_sensor_scan_hits(&[obc_app::SensorScanHit::new(0, 0, [1, 2, 3, 4, 5, 6], "HRM", -55)])
        }));
    steps.push(step("the scan list clears", 86_000).feed(|app| app.set_sensor_scan_hits(&[])));
    steps.push(step("weather freshness moves", 87_000).feed(|app| app.weather_feed_changed()));
    steps.push(step("the rain step count changes", 88_000).feed(|app| app.set_rain_view(4, 0.02)));
    steps.push(step("the rain step count is unchanged", 89_000).feed(|app| app.set_rain_view(4, 0.02)));

    // --- an upload card, left to time out ----------------------------------------------------------
    // Mid-ride the same upload lands as the **swap prompt** instead — same family, same 30 s
    // auto-close, and the replay leaves this one alone so the timeout dismissal is exercised too.
    steps.push(
        step("a second upload card arrives", 90_000)
            .fact(|f| f.note_route_upload(RouteUpload { id: 7, replaced: false, elevation: None }))
            .expect("RouteSwap"),
    );
    steps.push(step("waiting out the popup", 105_000).expect("RouteSwap"));
    steps.push(step("the popup times out", 121_000).expect("Map"));

    // --- and a long quiet tail -----------------------------------------------------------------------
    for i in 0..5 {
        steps.push(step("quiet tail", 122_000 + i * 1_000));
    }

    // --- the one battery fact a map base draws: the low-battery cue ------------------------------
    // A map base draws no gauge, but it does draw the top-left low-battery glyph, so the *cue* (not
    // the level) is in the Map key. Each crossing is placed on the gauge's own ~30 s cadence, which
    // is what makes the reading land at all.
    steps.push(step("the battery crosses into the low-battery cue", 155_000).battery(5).expect("Map"));
    steps.push(step("quiet with the cue up", 156_000).expect("Map"));
    steps.push(step("the battery charges back over the cue", 190_000).battery(40).expect("Map"));
    steps.push(step("quiet with the cue gone", 191_000).expect("Map"));
    steps
}

/// The parked-device replay: everything Home draws, and nothing that needs a fix.
fn home_replay() -> Vec<Step> {
    let mut steps = vec![
        step("boot on Home", 0).expect("Home"),
        step("the first battery read", 100).battery(75),
        step("quiet", 1_000),
        // The gauge is read on a ~30 s cadence, so a change is only seen at the next read. The
        // clock's minute rollovers (60 s, 120 s) are drained on their own passes, so the gauge
        // assertions below are about the gauge and not about the digits beside it.
        step("an unchanged level at the cadence", 30_500).battery(75),
        step("the minute rolls over", 61_000).battery(75),
        step("quiet", 61_500),
        step("a changed level at the next cadence", 91_500).battery(61),
        step("quiet after the gauge moved", 92_000),
        // The connected indicator: chrome Home draws in its title area.
        step("the phone connects", 92_500)
            .feed(|app| app.set_ble_status(BleStatus { link: BleLink::Connected, paired: true, passkey: None })),
        step("an unchanged link", 93_000)
            .feed(|app| app.set_ble_status(BleStatus { link: BleLink::Connected, paired: true, passkey: None })),
        step("the phone disconnects", 93_500)
            .feed(|app| app.set_ble_status(BleStatus { link: BleLink::Advertising, paired: true, passkey: None })),
        // Walk into the menu and leave the device alone: the idle return lands back on Home and
        // re-rolls the backdrop's jitter, which is Home's whole animation state.
        step("open the menu", 94_000).keys(&tap(Button::Select)).expect("Menu"),
        step("waiting out the idle timeout", 110_000).expect("Menu"),
        step("the idle return re-rolls the backdrop", 125_000).expect("Home"),
    ];
    for i in 0..10 {
        steps.push(step("quiet tail", 126_000 + i * 100));
    }
    steps
}

/// Drive one replay through both instances and compare the glass after **every** pass. Returns the
/// candidate's `(map, overlay)` repaint counts beside the reference's, so a caller can hold them to
/// a ceiling.
fn run_replay(
    label: &str,
    app: impl Fn() -> App,
    steps: &[Step],
    route: Option<&RouteReader<'_>>,
    reader: &Reader,
) -> ((usize, usize), (usize, usize)) {
    let mut reference = Instance::new(app());
    let mut candidate = Instance::new(app());

    for (n, s) in steps.iter().enumerate() {
        reference.advance(s, route, reader, false);
        let planned = candidate.advance(s, route, reader, true);

        if let Some(want) = s.expect {
            assert_eq!(candidate.app.top_screen().name(), want, "{label} step {n} ({}) must land on {want}", s.what);
        }
        if let Some((first, count)) = reference.glass.diff(&candidate.glass) {
            panic!(
                "{label} step {n} ({}) at {} ms: on-demand rendering lost {count} pixel(s), first at ({}, {}).\n\
                 The pass planned {planned:?}. A visible fact moved with no render key naming it and \
                 no explicit request covering it — that is the under-redraw the dirty contract forbids.",
                s.what,
                s.at_ms,
                first as u32 % W,
                first as u32 / W,
            );
        }
    }
    println!(
        "dirty parity ({label}): {} passes | map {}/{} | overlay {}/{}",
        steps.len(),
        candidate.map_repaints,
        reference.map_repaints,
        candidate.overlay_repaints,
        reference.overlay_repaints
    );
    ((candidate.map_repaints, candidate.overlay_repaints), (reference.map_repaints, reference.overlay_repaints))
}

/// **The ride.** Frame-for-frame parity across a route-following ride: the fix, the grid, the
/// camera, the overlay, the freeze, and every host-pushed card.
#[test]
fn on_demand_rendering_is_pixel_identical_to_rendering_every_pass() {
    let map = map_bytes();
    let map_src = SliceSource(&map);
    let tables = MapTables::parse(&map_src).expect("the replay map parses");
    let cache = MapCache::new();
    let reader = Reader::new(&map_src, &tables, &cache);

    let obcr = route_bytes();
    let route_src = SliceSource(&obcr);
    let idx = RouteIndex::read(&route_src).expect("the replay route parses");
    let route = RouteReader::new(&idx, &route_src);

    let camera = AppState::new((LON0 * 1e6) as i32, (LAT * 1e6) as i32, 0.05);
    let steps = replay();
    let ((map_repaints, overlay), (ref_map, ref_overlay)) =
        run_replay("ride", || riding_device(camera), &steps, Some(&route), &reader);

    // Over-redraw is safe, so this is a ceiling and not an equality — but a candidate that repainted
    // as often as the reference would mean render-on-demand had stopped demanding anything.
    assert_eq!(ref_map, steps.len(), "the reference renders the map every pass, by definition");
    assert!(
        map_repaints < steps.len(),
        "the candidate repainted the map on every one of {} passes — nothing is on demand",
        steps.len()
    );
    assert!(overlay <= ref_overlay, "candidate overlay repaints {overlay} exceeded the reference's {ref_overlay}");
}

/// **The parked device.** Home is the screen a bikepacker leaves the computer on, and it draws three
/// things nothing else does: the battery gauge, the connected indicator, and the screensaver
/// backdrop the idle return re-rolls. None of them moves with a fix, so this replay carries none.
#[test]
fn a_parked_device_repaints_only_what_home_draws() {
    let map = map_bytes();
    let map_src = SliceSource(&map);
    let tables = MapTables::parse(&map_src).expect("the replay map parses");
    let cache = MapCache::new();
    let reader = Reader::new(&map_src, &tables, &cache);

    let camera = AppState::new((LON0 * 1e6) as i32, (LAT * 1e6) as i32, 0.05);
    let steps = home_replay();
    let ((map_repaints, _), _) = run_replay("home", || App::new_idle(camera), &steps, None, &reader);
    assert!(map_repaints < steps.len(), "a parked device does not repaint every pass");
}

/// The quiet half of the same contract: with nothing at all happening, the candidate must plan
/// **zero** repaints — the render-on-demand claim in its strongest form, and the one a render key
/// that named too much would break silently.
#[test]
fn a_device_with_nothing_happening_plans_no_repaint_at_all() {
    let map = map_bytes();
    let map_src = SliceSource(&map);
    let tables = MapTables::parse(&map_src).expect("the replay map parses");
    let cache = MapCache::new();
    let reader = Reader::new(&map_src, &tables, &cache);

    let mut device = Instance::new(App::new_idle(AppState::new((LON0 * 1e6) as i32, (LAT * 1e6) as i32, 0.05)));
    // The boot frame, plus the Home clock's first minute tick, are the device's own — drain them.
    for ms in [0, 60_000, 120_000] {
        device.advance(&step("settling", ms), None, &reader, true);
    }
    let before = device.map_repaints;
    for i in 0..40u32 {
        device.advance(&step("quiet", 120_000 + i * 100), None, &reader, true);
    }
    assert_eq!(device.map_repaints, before, "an idle device with no input and no fix must render nothing");
}
