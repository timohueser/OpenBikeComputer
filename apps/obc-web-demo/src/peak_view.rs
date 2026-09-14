//! Cooperatively build the real terrain panorama between browser frames.

use obc_app::{
    peak_view::{runtime::Status, surface::Builder, terrain::Terrain, Panorama, SummitSearch},
    App, PeakViewProfile, Screen,
};
use obc_reader::{MapTables, Reader, SliceSource};
use std::sync::LazyLock;

pub static TERRAIN: LazyLock<SliceSource<'static>> = LazyLock::new(|| {
    let tables = MapTables::parse(&SliceSource(super::demo::DEMO_MAP)).expect("demo map parses");
    let region = tables.terrain().expect("demo map contains terrain");
    SliceSource(&super::demo::DEMO_MAP[region.offset as usize..(region.offset + region.len) as usize])
});

pub struct Runtime {
    terrain: Box<Terrain<'static>>,
    builder: Option<Box<Builder>>,
    search: SummitSearch,
    position: Option<(i32, i32)>,
    failed: bool,
}

impl Runtime {
    pub fn new() -> Self {
        Self {
            terrain: Box::new(Terrain::parse(&*TERRAIN).expect("demo surface terrain parses")),
            builder: None,
            search: SummitSearch::default(),
            position: None,
            failed: false,
        }
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
            if !app.peak_view_is_base() {
                self.reset();
            }
            return;
        }
        let Some(position) = app.state.user_fix.map(|fix| (fix.lat, fix.lon)).or(self.position) else {
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
            if let Some(ground) = self.terrain.ground_height(position.0, position.1) {
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
            builder.step(&mut *self.terrain, 1024);
            self.failed = self.terrain.failed();
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
