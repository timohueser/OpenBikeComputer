//! Shared helpers for the `obc-app` tests: the in-crate staging harnesses ([`super`]) and the
//! integration tests alike, which pull this very file in through `tests/common/mod.rs`. `App` is
//! named through `obc_app::` so the source compiles unchanged on both sides of the crate boundary
//! (lib.rs aliases the crate to itself under `cfg(test)`).

#![allow(dead_code)]

use std::collections::VecDeque;

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use obc_app::device_core::{
    DerivedInputs, DerivedTargets, ExternalFacts, NavigatorTag, OperationToken, OutcomeSlots, PassClock, PassInputs,
    PassPlan, PlatformSupport, Revision, StoreIdentity, StoreRevision,
};
use obc_app::navigator::{NavigatorEffect, NavigatorOutcome, PlannerWork};
use obc_app::{App, Dirty, Gesture};
use obc_ports::{Button, ButtonEvent, Fix, InputClock, InputEvent, InputSource, LocationSource, RideClock, Sensors};
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, PoiCategory, Reader, SliceSource};
use obc_route::{RouteIndex, RouteReader, Waypoints, WptEntry};

/// A `w`×`h` `Rgb888` buffer implementing `DrawTarget`, with clipped writes.
pub struct Buf {
    pub w: i32,
    pub h: i32,
    pub px: Vec<Rgb888>,
}

impl Buf {
    pub fn new(w: i32, h: i32) -> Self {
        Buf { w, h, px: vec![Rgb888::BLACK; (w * h) as usize] }
    }
    pub fn get(&self, x: i32, y: i32) -> Rgb888 {
        self.px[(y * self.w + x) as usize]
    }
    pub fn count(&self, c: Rgb888) -> usize {
        self.px.iter().filter(|&&p| p == c).count()
    }
    pub fn put(&mut self, x: i32, y: i32, c: Rgb888) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = c;
        }
    }
    /// Count pixels of color `c` in the right-edge band, split by screen half: returns
    /// `(top_half, bottom_half)`. The bulge pokes in from `x = w`, so it always lands in
    /// `x >= w - 20`.
    pub fn edge_halves(&self, c: Rgb888) -> (usize, usize) {
        let (mut top, mut bot) = (0, 0);
        for y in 0..self.h {
            for x in (self.w - 20).max(0)..self.w {
                if self.px[(y * self.w + x) as usize] == c {
                    if y < self.h / 2 {
                        top += 1;
                    } else {
                        bot += 1;
                    }
                }
            }
        }
        (top, bot)
    }
}

impl OriginDimensions for Buf {
    fn size(&self) -> Size {
        Size::new(self.w as u32, self.h as u32)
    }
}

impl DrawTarget for Buf {
    type Color = Rgb888;
    type Error = core::convert::Infallible;
    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            self.put(p.x, p.y, c);
        }
        Ok(())
    }
    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let clip = area.intersection(&self.bounding_box());
        if let Some(br) = clip.bottom_right() {
            for y in clip.top_left.y..=br.y {
                for x in clip.top_left.x..=br.x {
                    self.put(x, y, color);
                }
            }
        }
        Ok(())
    }
}

/// A minimal valid `.obcm`: one sea-backdrop style, one LOD with a single empty leaf and no chunks,
/// an empty POI directory and an empty hours pool. It renders as a flat backdrop, so the only
/// non-backdrop pixels come from whatever is drawn on top. `marker` is the header's marker color.
pub fn build_min_obcm(marker: u16) -> Vec<u8> {
    let profiles: &[&str] = &["Default"];
    // Every offset a header or directory carries is a count of `U = 16`-byte units, so every
    // structure one reaches starts on a unit boundary and the bytes between them are `0xFF` filler.
    // The header is not a unit multiple, so the style table begins at the next boundary.
    use obc_formats::obcm::{OffsetScale, FILLER};
    const SCALE: OffsetScale = OffsetScale::DEFAULT;
    let unit = SCALE.unit() as usize;
    let align_up = |at: usize| -> usize { at.next_multiple_of(unit) };
    let scaled = |at: usize| -> u32 {
        assert_eq!(at % unit, 0, "byte {at} is not on a unit boundary");
        (at / unit) as u32
    };

    let style_off: usize = align_up(obc_formats::obcm::HEADER_LEN);
    // Style table (8-byte style record): count=1, then (id=1, z=0, color=0x001F blue sea, weight=1,
    // flags=0, color2=0x0000 — solid, no secondary color).
    let mut styles = vec![1u8];
    styles.push(1);
    styles.push(0);
    styles.extend_from_slice(&0x001Fu16.to_le_bytes());
    styles.push(1);
    styles.push(0); // flags byte
    styles.extend_from_slice(&0x0000u16.to_le_bytes()); // color2 (absent ⇒ 0x0000)

    let lod_tab_off = align_up(style_off + styles.len());
    let index_off = align_up(lod_tab_off + 18); // one 18-byte LOD entry

    // LOD entry: max_mpp=+inf, index_off (scaled), node_count=1, chunk_size=16, chunk_count=0.
    let mut table = Vec::new();
    table.extend_from_slice(&f32::INFINITY.to_le_bytes());
    table.extend_from_slice(&scaled(index_off).to_le_bytes());
    table.extend_from_slice(&1u32.to_le_bytes());
    table.extend_from_slice(&16u16.to_le_bytes());
    table.extend_from_slice(&0u32.to_le_bytes());

    // Index: a single empty leaf (no chunk), then the offset table — the one `chunk_count + 1`
    // entry a chunkless LOD carries — then filler to the boundary `data_start` would land on, so
    // the section behind it can be named.
    let mut index = Vec::new();
    index.extend_from_slice(&0x7FFF_FFFFu32.to_le_bytes());
    index.extend_from_slice(&0u32.to_le_bytes());
    index.resize(align_up(index_off + index.len()) - index_off, FILLER);

    // POI section starts right after the index + offset table (no LOD chunks here). Empty directory:
    // count=6, chunk_size=512, six 13-byte entries (all node_count/chunk_count 0), then the two pool
    // fields (hours_pool_offset u32 + hours_pool_count u16), then an empty hours pool (a bare
    // `count 0`). The directory length is 3 + 6*13 + 6 = 87.
    let poi_section_off = index_off + index.len();
    let dir_len = 3 + 6 * 13 + 6;
    // Every zero-length region still has to be nameable, so it points at the first unit boundary
    // past the directory rather than at the directory's last byte.
    let after_dir = align_up(poi_section_off + dir_len);
    let mut poi_dir = vec![6u8]; // category_count
    poi_dir.extend_from_slice(&512u16.to_le_bytes()); // shared chunk_size
    for id in 1u8..=6 {
        poi_dir.push(id);
        poi_dir.extend_from_slice(&scaled(after_dir).to_le_bytes()); // index_offset (zero-length)
        poi_dir.extend_from_slice(&0u32.to_le_bytes()); // node_count
        poi_dir.extend_from_slice(&0u32.to_le_bytes()); // chunk_count
    }
    poi_dir.extend_from_slice(&scaled(after_dir).to_le_bytes()); // hours_pool_offset
    poi_dir.extend_from_slice(&0u16.to_le_bytes()); // hours_pool_count = 0
    poi_dir.resize(after_dir - poi_section_off, FILLER); // filler to the pool's boundary
    poi_dir.extend_from_slice(&0u16.to_le_bytes()); // the empty pool's own `count u16` = 0
    poi_dir.resize(align_up(poi_section_off + poi_dir.len()) - poi_section_off, FILLER);

    // Empty nav section at the tail: the 40-byte directory + the always-present profile table (the
    // caller's names, every multiplier 16 = 1.0×, climb-blind — this fixture has no graph to climb).
    // Zero-length index + edge pool "start" just past the profile table.
    let nav_section_off = poi_section_off + poi_dir.len();
    // The 40-byte directory is not a unit multiple, so the profile table starts at the section's
    // byte 48, with eight bytes of filler behind the directory.
    let profile_table_off = align_up(nav_section_off + obc_formats::obcm::NAV_DIR_LEN);
    let mut profile_table = Vec::new();
    for name in profiles {
        let base = profile_table.len();
        profile_table.extend_from_slice(name.as_bytes());
        profile_table.resize(base + 12, 0xFF); // 0xFF-padded 12-byte name
        profile_table.extend_from_slice(&[16u8; 32]); // highway multipliers (1.0×)
        profile_table.extend_from_slice(&[16u8; 8]); // surface multipliers (1.0×)
        profile_table.push(0); // climb_weight
        profile_table.resize(base + 56, 0); // three reserved bytes, zero
    }
    let after_nav = align_up(profile_table_off + profile_table.len());
    let dark_style_off = after_nav;
    let mut nav_dir = Vec::new();
    nav_dir.extend_from_slice(&scaled(after_nav).to_le_bytes()); // index_offset (zero-length)
    nav_dir.extend_from_slice(&0u32.to_le_bytes()); // index_node_count
    nav_dir.extend_from_slice(&0u32.to_le_bytes()); // node_chunk_count
    nav_dir.extend_from_slice(&scaled(after_nav).to_le_bytes()); // edge_pool_offset (zero-length)
    nav_dir.extend_from_slice(&0u32.to_le_bytes()); // edge_chunk_count
    nav_dir.extend_from_slice(&512u16.to_le_bytes()); // chunk_size (pinned)
    nav_dir.extend_from_slice(&scaled(profile_table_off).to_le_bytes()); // profile_table_offset
    nav_dir.push(profiles.len() as u8); // profile_count
    nav_dir.push(0); // reserved — a field, so `0`, unlike a gap
    nav_dir.extend_from_slice(&scaled(after_nav).to_le_bytes()); // snap_index_offset (zero-length)
    nav_dir.extend_from_slice(&0u32.to_le_bytes()); // snap_index_node_count
    nav_dir.extend_from_slice(&0u32.to_le_bytes()); // snap_chunk_count
    nav_dir.resize(profile_table_off - nav_section_off, FILLER);
    nav_dir.extend_from_slice(&profile_table);
    nav_dir.resize(after_nav - nav_section_off, FILLER);

    let mut f = Vec::new();
    f.extend_from_slice(b"OBCM");
    f.push(obc_formats::obcm::VERSION);
    for v in [-1000i32, -1000, 1000, 1000] {
        f.extend_from_slice(&v.to_le_bytes()); // bbox: min_lat, min_lon, max_lat, max_lon
    }
    f.extend_from_slice(&scaled(style_off).to_le_bytes());
    f.push(1); // lod count
    f.extend_from_slice(&scaled(lod_tab_off).to_le_bytes());
    f.extend_from_slice(&marker.to_le_bytes());
    f.extend_from_slice(&scaled(poi_section_off).to_le_bytes());
    f.extend_from_slice(&scaled(nav_section_off).to_le_bytes());
    f.push(SCALE.log2()); // offset scale
    f.extend_from_slice(&0u32.to_le_bytes()); // terrain offset — this fixture has no raster
    f.extend_from_slice(&0u32.to_le_bytes()); // …and its length is `0` exactly when the offset is
    f.extend_from_slice(&[0; 16]); // no landmark or peak section
    f.extend_from_slice(&scaled(dark_style_off).to_le_bytes());
    f.extend_from_slice(&marker.to_le_bytes());
    debug_assert_eq!(f.len(), obc_formats::obcm::HEADER_LEN);
    f.resize(style_off, FILLER);
    f.extend_from_slice(&styles);
    f.resize(lod_tab_off, FILLER);
    f.extend_from_slice(&table);
    f.resize(index_off, FILLER);
    f.extend_from_slice(&index);
    f.extend_from_slice(&poi_dir);
    f.extend_from_slice(&nav_dir);
    debug_assert_eq!(f.len(), dark_style_off);
    f.extend_from_slice(&styles);
    f
}

/// A scripted `InputSource` draining a queue of raw input events, one per `poll`.
pub struct Keys(pub VecDeque<InputEvent>);

impl InputSource for Keys {
    fn poll(&mut self) -> Option<InputEvent> {
        self.0.pop_front()
    }
}

pub fn keys(evs: &[InputEvent]) -> Keys {
    Keys(evs.iter().copied().collect())
}

pub fn step(n: i32) -> InputEvent {
    InputEvent::Step(n)
}
pub fn down(b: Button) -> InputEvent {
    InputEvent::Button(ButtonEvent::Down(b))
}
pub fn up(b: Button) -> InputEvent {
    InputEvent::Button(ButtonEvent::Up(b))
}
/// A tap (down then up within the hold threshold) → a `Press` (Select) or `Back` gesture.
pub fn tap(b: Button) -> [InputEvent; 2] {
    [down(b), up(b)]
}

/// A `LocationSource` that replays the same fix on every poll, like the simulator's override.
pub struct ReplayFix(pub Option<Fix>);
impl LocationSource for ReplayFix {
    fn poll(&mut self) -> Option<Fix> {
        self.0
    }
}

/// A `LocationSource` that emits its fix exactly once, then `None` — the real one-fresh-fix-per-
/// tick contract, with no per-poll replay.
pub struct OnceFix(pub Option<Fix>);
impl LocationSource for OnceFix {
    fn poll(&mut self) -> Option<Fix> {
        self.0.take()
    }
}

/// A `LocationSource` that never has a fix.
pub struct NoFix;
impl LocationSource for NoFix {
    fn poll(&mut self) -> Option<Fix> {
        None
    }
}

/// Tick once over the Grimsel route re-typed `bike`, as the host hands the active route's reader,
/// and answer the settings' bike type after it.
pub fn tick_typed_route(app: &mut App, bike: obc_app::settings::BikeType) -> obc_app::settings::BikeType {
    let mut bytes = include_bytes!("../../../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr").to_vec();
    bytes[obc_formats::obcr::BIKE_TYPE_OFF] = bike as u8;
    let src = SliceSource(&bytes);
    let index = RouteIndex::read(&src).expect("the fixture route parses");
    app.tick(RideClock(1_000), Sensors::new(&mut NoFix), Some(&RouteReader::new(&index, &src)));
    app.settings().bike_type
}

/// Tick once with no fix and no sensors, then composite one frame of `app` over `bytes` into a
/// 120×120 recording [`Buf`].
pub fn render_120(app: &mut App, bytes: &[u8]) -> Buf {
    app.tick(RideClock(0), Sensors::new(&mut NoFix), None);
    let cache = MapCache::new();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("valid obcm");
    let reader = Reader::new(&src, &tables, &cache);
    let mut buf = Buf::new(120, 120);
    let mut scratch = Box::new(obc_render::RenderScratch::new());
    app.render_frame(Some(&mut scratch), &mut buf, &reader, None, 120.0, 120.0, |c| {
        let (r, g, b) = rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    });
    buf
}

/// A `ByteSink` over a growable `Vec`: the host's write-the-whole-file-to-RAM backing, and what a
/// converter in a test writes into.
#[derive(Default)]
pub struct VecSink(pub Vec<u8>);

impl obc_formats::io::ByteSink for VecSink {
    fn write(&mut self, bytes: &[u8]) -> Result<(), obc_formats::io::Error> {
        self.0.extend_from_slice(bytes);
        Ok(())
    }
    fn patch_at(&mut self, offset: u32, bytes: &[u8]) -> Result<(), obc_formats::io::Error> {
        let at = offset as usize;
        self.0[at..at + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
}

/// The resident waypoint table of an `.obcr`, through the production window loader — the table a
/// real import hands the app, as opposed to the synthetic [`wpts`] one.
///
/// The loader returns one window, so an over-cap file would hand back a prefix. A caller here
/// means "every waypoint the file holds", and gets told when that is not what it has.
pub fn wpts_from_obcr(bytes: &[u8]) -> Waypoints {
    let src = SliceSource(bytes);
    let index = RouteIndex::read(&src).expect("a converted .obcr parses");
    let wpts = RouteReader::new(&index, &src).load_waypoints(0);
    assert!(!wpts.truncated, "the file holds more waypoints than one window: load a window yourself");
    wpts
}

/// A stored ride's header facts, named and otherwise nominal: enough for a list row and its detail
/// page. The one builder every ride-list test seeds from.
pub fn ride_summary(name: &str) -> obc_app::RideSummary {
    obc_app::RideSummary {
        name: heapless::String::try_from(name).unwrap(),
        start_time: 1_720_000_000,
        distance_m: 1_000,
        moving_time_s: 600,
        climb_m: 10,
        synced: false,
        synced_at_utc: 0,
        ..Default::default()
    }
}

/// A synthetic waypoint table from `(distance, name)` pairs: every entry on the line, uncategorised.
pub fn wpts(items: &[(u32, &str)]) -> Waypoints {
    let full: Vec<_> = items.iter().map(|&(d, n)| (d, n, None, 0)).collect();
    wpts_detailed(&full)
}

/// The full shape: `(distance, name, category, lateral offset)`, for the Up-ahead timeline.
pub fn wpts_detailed(items: &[(u32, &str, Option<PoiCategory>, i16)]) -> Waypoints {
    let mut w = Waypoints::new();
    for &(dist_along_m, name, category, lateral_offset_m) in items {
        let mut n = heapless::String::new();
        n.push_str(name).unwrap();
        w.entries.push(WptEntry { dist_along_m, lon: 0, lat: 0, category, lateral_offset_m, name: n }).unwrap();
    }
    w
}

/// Every capability the test platform implements. A suite that needs a device without one names it.
pub const EVERY_CAPABILITY: PlatformSupport =
    PlatformSupport { detour: true, settings_persistence: true, dfu: true, bonding: true, storage_space_report: true };

/// Run one DeviceCore pass at `ms` with the executor's answers — the production frame every host
/// drives. The ports these suites do not exercise stay empty; a suite that needs one drives
/// [`App::run_pass`] itself.
pub fn pass(
    app: &mut App,
    ms: u32,
    outcomes: &mut OutcomeSlots,
    facts: &mut ExternalFacts,
    route: Option<&RouteReader<'_>>,
) -> PassPlan {
    pass_with_gestures(app, ms, outcomes, facts, route, &[])
}

/// [`pass`], with rider input arriving on it. The pass runs outcomes, then input, then Navigator,
/// so this is the only way to put a gesture between an answer and the release that follows it.
pub fn pass_with_gestures(
    app: &mut App,
    ms: u32,
    outcomes: &mut OutcomeSlots,
    facts: &mut ExternalFacts,
    route: Option<&RouteReader<'_>>,
    gestures: &[Gesture],
) -> PassPlan {
    let mut loc = NoFix;
    app.run_pass(PassInputs {
        now: PassClock { ride: RideClock(ms), ui: InputClock(ms) },
        gestures,
        sensors: Sensors::new(&mut loc),
        route,

        support: EVERY_CAPABILITY,
        outcomes,
        facts,
        derived: DerivedInputs::NONE,
        targets: DerivedTargets::NONE,
    })
}

/// One pass with nothing owed — the frame that only collects what the app already decided.
pub fn quiet_pass(app: &mut App, ms: u32) -> PassPlan {
    let mut facts = ExternalFacts::NONE;
    pass(app, ms, &mut OutcomeSlots::new(), &mut facts, None)
}

/// One pass carrying a single external fact.
pub fn pass_with_fact(app: &mut App, ms: u32, note: impl FnOnce(&mut ExternalFacts)) -> PassPlan {
    let mut facts = ExternalFacts::NONE;
    note(&mut facts);
    pass(app, ms, &mut OutcomeSlots::new(), &mut facts, None)
}

/// Report the store a device with a card has, through one real pass. `store_writable` admits a
/// catalog mutation, a route plan and a ride recording, and it rises only from a `StoreRevision`
/// fact, so a suite that starts a ride needs this first.
pub fn mount_store(app: &mut App) {
    pass_with_fact(app, 0, |facts| {
        facts.note_store_revision(StoreRevision { store: StoreIdentity::new(1), revision: Revision::new(1) })
    });
}

/// A runtime host in miniature. Each [`frame`](Frames::frame) recognises raw button events through
/// the app's own shared recogniser and then runs one DeviceCore pass with the gestures that came
/// out — the single-loop composition the simulator and the web demo use, and therefore the only one
/// in which the render keys are compared. The sensor ports are built inside the frame from plain
/// values, so a caller never has to keep a port alive across the pass's borrows.
pub struct Frames {
    outcomes: OutcomeSlots,
    /// Cumulative fuel-gauge polls across every frame so far.
    pub fuel_polls: u32,
}

impl Default for Frames {
    fn default() -> Self {
        Frames::new()
    }
}

impl Frames {
    pub fn new() -> Self {
        Frames { outcomes: OutcomeSlots::new(), fuel_polls: 0 }
    }

    /// One frame at `ms`: recognise `evs`, then run the pass with `fix` on the location port and
    /// `battery` on the fuel gauge (`None` = no gauge wired at all).
    pub fn frame(
        &mut self,
        app: &mut App,
        ms: u32,
        evs: &[InputEvent],
        fix: Option<Fix>,
        battery: Option<u8>,
    ) -> Dirty {
        let batch = app.recognize(InputClock(ms), &mut keys(evs));
        let mut loc = OnceFix(fix);
        let mut gauge = CountingGauge { value: battery, polls: 0 };
        let mut facts = ExternalFacts::NONE;
        let plan = app.run_pass(PassInputs {
            now: PassClock { ride: RideClock(ms), ui: InputClock(ms) },
            gestures: &batch,
            sensors: Sensors { fuel: Some(&mut gauge), ..Sensors::new(&mut loc) },
            route: None,

            support: EVERY_CAPABILITY,
            outcomes: &mut self.outcomes,
            facts: &mut facts,
            derived: DerivedInputs::NONE,
            targets: DerivedTargets::NONE,
        });
        self.fuel_polls += gauge.polls;
        plan.render
    }

    /// A quiet frame: no input, no fix, no gauge.
    pub fn idle(&mut self, app: &mut App, ms: u32) -> Dirty {
        self.frame(app, ms, &[], None, None)
    }
}

/// The fuel gauge behind [`Frames`]: reports a settable level and counts what it was asked for.
struct CountingGauge {
    value: Option<u8>,
    polls: u32,
}

impl obc_ports::FuelGauge for CountingGauge {
    fn poll(&mut self) -> Option<u8> {
        self.polls += 1;
        self.value
    }
}

/// The suites' navigation executor: one pass at a time, it takes the search DeviceCore hands out,
/// answers it with a scripted terminal result and returns the workspace when Navigator asks for it.
/// An answer it deposits is read by the next pass. Its passes are the app's frames, so after one
/// there is no dirt left for [`App::take_dirty`] to report.
pub struct Planner<'r> {
    /// The clock its passes run at.
    ms: u32,
    /// The active route's reader. A pass without it is the route line vanishing, which resets the
    /// matcher, so a suite that rides a real route hands it over.
    route: Option<&'r RouteReader<'r>>,
    /// What it has answered, waiting for the next pass to read.
    outcomes: OutcomeSlots,
    /// The operation the app is running, when it handed one out.
    token: Option<OperationToken<NavigatorTag>>,
    /// The last operation the app abandoned — what a late answer carries.
    abandoned: Option<OperationToken<NavigatorTag>>,
    work: Option<PlannerWork>,
    /// Gestures the next pass carries, so a rider input can land on the pass that reads an
    /// answer. One-shot: the pass that takes them leaves the queue empty.
    next_gestures: heapless::Vec<Gesture, 4>,
    /// Whether the app asked for the workspace back since the last read.
    released: bool,
    /// Whether the last release asked the host to keep the operation's publication.
    retained: Option<bool>,
    /// Whether the app asked for the planned detour to be spliced since the last read.
    commit_asked: bool,
    /// What its passes asked to repaint since the last read.
    render: Dirty,
}

impl Default for Planner<'_> {
    fn default() -> Self {
        Planner::at(0)
    }
}

impl<'r> Planner<'r> {
    /// A planner whose passes run at `ms`, for a suite that drives the animation clock itself.
    pub fn at(ms: u32) -> Self {
        Planner {
            ms,
            route: None,
            outcomes: OutcomeSlots::new(),
            token: None,
            abandoned: None,
            work: None,
            next_gestures: heapless::Vec::new(),
            released: false,
            retained: None,
            commit_asked: false,
            render: Dirty::CLEAN,
        }
    }

    /// A planner riding `route` — every pass it runs carries the reader Navigator needs.
    pub fn on(route: &'r RouteReader<'r>) -> Self {
        Planner { route: Some(route), ..Planner::at(0) }
    }

    /// Run one pass and serve whatever navigation work it hands out, leaving the answer in the
    /// slots for the next one. The single-pass shape is the board's own loop.
    pub fn one_pass(&mut self, app: &mut App) -> PassPlan {
        let ms = self.ms;
        let mut facts = ExternalFacts::NONE;
        // Queued input rides the pass that reads an answer: the board runs outcomes, then input,
        // then Navigator, and only there is that order observable.
        let gestures = if self.outcomes.navigator.is_empty() {
            heapless::Vec::new()
        } else {
            core::mem::take(&mut self.next_gestures)
        };
        let mut plan = pass_with_gestures(app, ms, &mut self.outcomes, &mut facts, self.route, &gestures);
        self.render.map |= plan.render.map;
        self.render.overlay |= plan.render.overlay;
        match plan.effects.navigator.take() {
            Some(NavigatorEffect::Acquire { token, work }) => {
                self.token = Some(token);
                self.work = Some(work);
                self.outcomes.navigator.try_put(NavigatorOutcome::Acquired { token }).unwrap();
            }
            Some(NavigatorEffect::Step { token } | NavigatorEffect::CommitRoute { token }) => {
                self.token = Some(token);
            }
            Some(NavigatorEffect::CommitDetour { token }) => {
                self.token = Some(token);
                self.commit_asked = true;
            }
            Some(NavigatorEffect::Release { token, retain_result, .. }) => {
                self.released = true;
                self.retained = Some(retain_result);
                self.abandoned = self.token.take();
                let _ = self.outcomes.navigator.try_put(NavigatorOutcome::Released { token });
            }
            None => {}
        }
        plan
    }

    /// Run passes until DeviceCore stops handing out navigation work, starting with `seed` in the
    /// slots.
    fn settle(&mut self, app: &mut App, seed: Option<NavigatorOutcome>) {
        if let Some(outcome) = seed {
            let _ = self.outcomes.navigator.try_put(outcome);
        }
        for _ in 0..8 {
            let plan = self.one_pass(app);
            if self.outcomes.navigator.is_empty() && !plan.immediate && plan.effects.navigator.is_empty() {
                break;
            }
        }
    }

    /// The search DeviceCore handed out since the last call, if it handed one out.
    pub fn take_work(&mut self, app: &mut App) -> Option<PlannerWork> {
        self.settle(app, None);
        self.work.take()
    }

    /// Answer the running search. `outcome` is built from the operation's own token, which is what
    /// Navigator validates the answer against.
    pub fn answer(&mut self, app: &mut App, outcome: impl FnOnce(OperationToken<NavigatorTag>) -> NavigatorOutcome) {
        self.settle(app, None);
        let token = self.token.take().expect("an operation is running to answer");
        let mut answer = outcome(token);
        if let NavigatorOutcome::PlanFinished { route, .. } = answer {
            self.settle(
                app,
                Some(NavigatorOutcome::Stepped { token, progress: obc_app::navigator::PlannerProgress::Reached }),
            );
            answer = NavigatorOutcome::PlanFinished { token: self.token.take().expect("commit requested"), route };
        }
        self.settle(app, Some(answer));
    }

    /// Queue `gestures` onto the pass that reads the next answer, which is the board's own order:
    /// outcomes, then input, then Navigator. It is how a rider pressing a button at the moment an
    /// answer lands is reproduced.
    pub fn press_on_the_next_pass(&mut self, gestures: &[Gesture]) {
        self.next_gestures = heapless::Vec::from_slice(gestures).expect("at most four gestures on one pass");
    }

    /// Answer an operation the app already abandoned — the slow executor that finished its search
    /// after the rider walked away.
    pub fn answer_late(
        &mut self,
        app: &mut App,
        outcome: impl FnOnce(OperationToken<NavigatorTag>) -> NavigatorOutcome,
    ) {
        let token = self.abandoned.take().expect("an operation was abandoned to answer late");
        self.settle(app, Some(outcome(token)));
    }

    /// The operation the app abandoned, kept for a second late answer.
    pub fn abandoned(&self) -> Option<OperationToken<NavigatorTag>> {
        self.abandoned
    }

    /// Deposit an answer built by hand — a late one for an operation this planner still remembers.
    pub fn deliver(&mut self, app: &mut App, outcome: NavigatorOutcome) {
        self.settle(app, Some(outcome));
    }

    /// Whether DeviceCore asked for the workspace back since the last read — one-shot.
    pub fn took_release(&mut self, app: &mut App) -> bool {
        self.settle(app, None);
        core::mem::take(&mut self.released)
    }

    /// Whether the last release told the host to keep this operation's publication. `None` until
    /// one has been asked for. A `false` here is what turns into a publish compensation on the
    /// board, which retracts the route the app may already be riding.
    pub fn retained_result(&mut self, app: &mut App) -> Option<bool> {
        self.settle(app, None);
        self.retained
    }

    /// Whether DeviceCore asked for the planned detour to be spliced since the last read — one-shot.
    pub fn took_commit(&mut self, app: &mut App) -> bool {
        self.settle(app, None);
        core::mem::take(&mut self.commit_asked)
    }

    /// What its passes asked to repaint since the last read — one-shot, like [`App::take_dirty`].
    pub fn take_render(&mut self) -> Dirty {
        core::mem::take(&mut self.render)
    }
}

/// A selected place for tests of the shared profile drawer.
pub fn selected_place() -> obc_app::screen::Screen {
    obc_app::screen::Screen::PoiDetail(obc_app::screen::PoiDetailScreen::new(obc_reader::Poi {
        opening: obc_reader::hours::OpeningStatus::Unknown,
        metadata: Default::default(),
        lat: 43_736_000,
        lon: 7_421_000,
        subtype: 1,
        name: heapless::String::try_from("Fontaine").unwrap(),
        hours_ref: u16::MAX,
        distance_m: 100,
    }))
}
