//! Build the real terrain panorama cooperatively, one bounded step per host frame.
//!
//! The frame-stepped hosts have no worker thread, so the panorama grows inside the frame loop:
//! [`Runtime::update`] takes one bounded step per tick and yields to input and paint. The
//! simulator runs the same builder on a thread instead ([`obc_app::peak_view::runtime`]).

use crate::flat_map::FlatMap;
use crate::flat_store::ObjectSource;
use obc_app::peak_view::{runtime::Status, surface::Builder, terrain::Terrain, Panorama, SummitSearch};
use obc_app::{App, PeakViewProfile, Screen};
use obc_formats::io::{ByteSource, Error, WindowSource};
use obc_formats::obct;
use obc_reader::Reader;

/// The terrain region of a card map, read through the map's own retained revision.
struct MapTerrain {
    source: ObjectSource,
    offset: u64,
    len: u64,
}

impl ByteSource for MapTerrain {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let end = offset.checked_add(buf.len() as u64).ok_or(Error::BadOffset)?;
        if end > self.len {
            return Err(Error::BadOffset);
        }
        self.source.read_at(self.offset + offset, buf)
    }
}

/// The OBCT surface region of `map` as an owned source, or `None` when the map carries no surface
/// terrain this build can draw a panorama from.
fn map_surface(map: &FlatMap) -> Option<Box<dyn ByteSource>> {
    let region = map.tables().terrain()?;
    let source = map.source();
    let window = WindowSource::new(&source, region.offset, region.len)?;
    let mut header = [0; obct::HEADER_LEN];
    window.read_at(0, &mut header).ok()?;
    obct::validate_header_prefix(&header).ok()?;
    if header[4] != obct::SURFACE_VERSION || header[7] & obct::SURFACE_FLAG == 0 {
        return None;
    }
    Some(Box::new(MapTerrain { source, offset: region.offset, len: region.len }))
}

/// A terrain parse next to the bytes it reads.
///
/// [`Terrain`] borrows its source for its whole life and keeps a tile cache the stepping depends
/// on, so the source cannot be a per-tick temporary. The runtime owns both instead.
struct OwnedTerrain {
    /// Borrows the source below. Declared first, so it is dropped before the bytes it reads.
    terrain: Box<Terrain<'static>>,
    /// Read only through `terrain`; held so those reads stay valid.
    _source: Box<dyn ByteSource>,
}

impl OwnedTerrain {
    fn parse(source: Box<dyn ByteSource>) -> Result<Self, Error> {
        // SAFETY: the source is boxed, so its address is stable for as long as this struct lives,
        // and nothing hands it out or moves it afterwards. `terrain` is the first field, so it is
        // dropped before the source it borrows.
        let borrowed: &'static dyn ByteSource = unsafe { &*(&*source as *const dyn ByteSource) };
        Ok(Self { terrain: Box::new(Terrain::parse(borrowed)?), _source: source })
    }
}

/// One host's Peak View: the terrain, the in-flight panorama build, and the summit search.
pub struct Runtime {
    terrain: OwnedTerrain,
    builder: Option<Box<Builder>>,
    search: SummitSearch,
    position: Option<(i32, i32)>,
    failed: bool,
}

impl Runtime {
    /// Over any terrain source — a shipped payload, a fixture, a map region.
    pub fn new(source: Box<dyn ByteSource>) -> Result<Self, Error> {
        Ok(Self {
            terrain: OwnedTerrain::parse(source)?,
            builder: None,
            search: SummitSearch::default(),
            position: None,
            failed: false,
        })
    }

    /// Over the card map's surface terrain. `None` when the map has none, or it does not parse.
    pub fn over_map(map: &FlatMap) -> Option<Self> {
        Self::new(map_surface(map)?).ok()
    }

    pub fn reset(&mut self) {
        self.builder = None;
        self.search = SummitSearch::default();
        self.position = None;
        self.failed = false;
    }

    pub fn panorama(&self) -> Option<&Panorama> {
        self.builder.as_ref().filter(|_| !self.failed).map(|builder| &builder.panorama)
    }

    pub fn update(&mut self, app: &mut App, reader: &Reader<'_>) {
        if !matches!(app.top_screen(), Screen::PeakView(_)) {
            if !app.peak_view_retains_panorama() {
                self.reset();
            }
            return;
        }
        let Some(position) =
            app.peak_view_position().or_else(|| app.state.user_fix.map(|fix| (fix.lat, fix.lon))).or(self.position)
        else {
            app.set_peak_view_status(Status::Waiting);
            return;
        };
        // Complete an in-flight view before relocating. Turning only selects other sectors.
        if self.builder.as_ref().is_none_or(|builder| builder.complete())
            && self.position.is_none_or(|old| obc_app::peak_view::moved(old, position))
        {
            self.reset();
            self.position = Some(position);
            app.state.peak_view_peak_count = 0;
            let measured = app.recorder.fused_elevation_m();
            if let Some(ground) = self.terrain.terrain.eye_ground(position.0, position.1, measured) {
                let mut profile = PeakViewProfile::at(position.0, position.1, 0);
                profile.set_ground(ground);
                let mut peaks = obc_app::peak_view::Candidates::new();
                if obc_app::peak_view::collect_summits(reader, position, profile.observer_elevation_m, &[], &mut peaks)
                    .is_ok()
                {
                    profile.peaks = &peaks;
                    profile.default_heading_q4 = app.peak_view_heading_q4();
                    profile.set_ground(ground);
                    self.builder = Some(Box::new(Builder::new(&profile)));
                }
            }
            self.failed = self.builder.is_none();
        }
        if let Some(builder) = &mut self.builder {
            let heading = app.peak_view_heading_q4();
            let progress = builder.progress();
            builder.set_heading(heading);
            // Each step has a bounded hierarchy-node budget; yield to input and paint each frame.
            builder.step(&mut *self.terrain.terrain, 1024);
            self.failed = self.terrain.terrain.failed();
            if builder.complete() && !self.failed && self.search.refill(builder, reader).is_err() {
                self.failed = true;
            }
            app.state.peak_view_profile = Some(builder.profile());
            app.state.peak_view_peak_count = 0;
            for (i, peak) in builder.display_peaks().enumerate() {
                app.state.peak_view_peaks[i] = peak;
                app.state.peak_view_peak_count += 1;
            }
            let status = if self.failed {
                Status::Unavailable
            } else if builder.complete() {
                Status::Ready
            } else {
                Status::Building(u64::from(builder.progress()))
            };
            if builder.progress() != progress || self.failed || builder.complete() {
                app.set_peak_view_status(status);
            }
        }
        if self.failed {
            app.set_peak_view_status(Status::Unavailable);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_app::AppState;

    const MAP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../apps/obc-sim/assets/grimsel-demo.obcm");

    /// Every sector of the circle is built — what the runtime reports as `Status::Ready`. The
    /// status goes into the screen, which keeps no getter, so the panorama itself is asked.
    fn complete(runtime: &Runtime) -> bool {
        runtime
            .panorama()
            .is_some_and(|panorama| (0..1440).step_by(10).all(|bearing| panorama.ready_at_bearing_q4(bearing)))
    }

    /// The card path end to end: the map's OBCT surface region as an owned source, the parse that
    /// borrows it, and bounded steps that build a real skyline out of the card's own bytes.
    #[test]
    fn a_card_maps_surface_builds_a_panorama_in_bounded_steps() {
        let bytes = std::fs::read(MAP).expect("the demo map is checked in");
        let owner = crate::flat_store::HostStore::memory().unwrap();
        let map = FlatMap::from_bytes_in(&owner, &bytes).expect("the demo map imports");
        let mut runtime = Runtime::over_map(&map).expect("the demo map carries surface terrain");

        let (cam_lon, cam_lat, zoom) = crate::initial_camera(&map.reader(), 240);
        let mut app = Box::new(obc_app::App::new(AppState::new(cam_lon, cam_lat, zoom)));
        app.state.user_fix = Some(obc_ports::Fix::at(46_585_000, 7_961_000));
        app.state.compass_deg = Some(141.25);
        app.state.peak_view_profile = Some(PeakViewProfile::at(0, 0, 0));
        assert!(app.show_peak_view());

        let mut steps = 0;
        while !complete(&runtime) && steps < 4_000 {
            runtime.update(&mut app, &map.reader());
            steps += 1;
        }
        assert!(complete(&runtime), "the card's own terrain builds the whole circle in bounded steps");
        assert!(steps > 1, "and it yields between steps rather than building it all in one");
        assert!(app.state.peak_view_peak_count > 0, "named summits come from the same map");
        assert!(app.state.peak_view_profile.unwrap().observer_elevation_m > 1_000, "ground height from the DEM");
        let mut frame = crate::RgbaFrame::new(240, 320);
        let draw = |app: &mut App, frame: &mut crate::RgbaFrame| {
            app.render_frame(None, frame, &map.reader(), None, 240.0, 320.0, |c| {
                let (r, g, b) = obc_reader::rgb565_to_rgb888(c);
                embedded_graphics::pixelcolor::Rgb888::new(r, g, b)
            });
        };
        app.apply_gesture(obc_app::Gesture::Press);
        draw(&mut app, &mut frame);
        let observer = app.peak_view_position().unwrap();
        let heading = app.peak_view_heading_q4();
        app.state.user_fix = Some(obc_ports::Fix::at(46_560_000, 8_340_000));
        runtime.update(&mut app, &map.reader());
        assert_eq!(runtime.position, Some(observer), "Browse retains the observer when GPS moves");
        app.apply_gesture(obc_app::Gesture::Press);
        assert!(matches!(app.top_screen(), Screen::PeakArticle(_)), "Mönch's installed article opens");
        runtime.update(&mut app, &map.reader());
        assert!(complete(&runtime), "reading retains the completed panorama");
        app.apply_gesture(obc_app::Gesture::Back);
        runtime.update(&mut app, &map.reader());
        assert!(complete(&runtime));
        assert_eq!(runtime.position, Some(observer), "Back retains the frozen Browse observer");
        assert_eq!(app.peak_view_heading_q4(), heading);
        app.apply_gesture(obc_app::Gesture::Back);
        for _ in 0..4_000 {
            runtime.update(&mut app, &map.reader());
            if runtime.position == Some((46_560_000, 8_340_000)) {
                break;
            }
        }
        assert_eq!(runtime.position, Some((46_560_000, 8_340_000)), "Live resumes the current position");
    }
}
