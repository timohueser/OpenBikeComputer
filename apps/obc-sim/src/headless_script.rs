//! Normal button scripts before or after GPX motion, using the same host pass and renderer.
use super::*;
use obc_ports::RideClock;

pub(crate) struct Session<'a, 's> {
    pub diagnostics: diagnostics::Diagnostics,
    pub host: &'a mut HostLoop,
    pub route: &'a mut ActiveRouteSession,
    pub stores: &'a mut Stores<'s>,
    pub map: &'a map_file::LoadedMap,
    pub reader: &'a obc_reader::Reader<'a>,
    pub elevation: &'a mut dyn obc_route::ElevationSource,
    pub platform: &'a mut HeadlessPlatform,
    pub peak: &'a mut peak_view::Runtime,
    pub player: &'a mut Option<GpxPlayer>,
    pub size: (u32, u32),
}
impl Session<'_, '_> {
    pub fn run(&mut self, app: &mut App, script: &str, now: u32, ride: RideClock, position: Option<f64>) -> u32 {
        let mut scratch = Box::new(obc_render::RenderScratch::new());
        let mut hook = |app: &mut App, what: ScriptHook, now: u32| {
            if let ScriptHook::Before(token) | ScriptHook::After(token) = what {
                if self.diagnostics.enabled() {
                    self.diagnostics.record(
                        if matches!(what, ScriptHook::Before(_)) { "input" } else { "input_result" },
                        serde_json::json!({"token": token.to_string(), "ui_ms": now, "screen": app.top_screen().name()}),
                    );
                }
                return;
            }
            if what == ScriptHook::Tick {
                if let Some(player) = self.player.as_mut() {
                    // A refresh at the current position emits through the normal GPS port.
                    player.seek(position.unwrap_or_else(|| player.time()));
                    self.route.sync(app, self.stores.routes);
                    let mut plan = {
                        let route = self
                            .route
                            .index()
                            .zip(self.stores.routes.active_source())
                            .map(|(index, source)| RouteReader::new(index, source));
                        self.host.pass(
                            app,
                            obc_app::device_core::PassClock { ride, ui: InputClock(now) },
                            &[],
                            obc_ports::Sensors::new(player),
                            route.as_ref(),
                            gui::SIM_SUPPORT,
                        )
                    };
                    self.host.execute(
                        app,
                        &mut plan,
                        self.route,
                        self.stores.routes,
                        self.stores.rides,
                        self.stores.tracks,
                        self.stores.trips,
                        self.map.planner_map(),
                        self.elevation,
                        self.platform,
                    );
                }
            }
            settle_at(
                self.host,
                self.route,
                app,
                self.stores,
                self.map.planner_map(),
                self.elevation,
                self.platform,
                obc_app::device_core::PassClock { ride, ui: InputClock(now) },
            );
            self.peak.finish(app);
            if what == ScriptHook::Render {
                let route = obc_host_core::frame::active_route(self.route, self.stores.routes);
                let mut fb = Framebuffer::new(self.size.0, self.size.1);
                let _ = map_file::render_frame(
                    app,
                    &mut scratch,
                    &mut fb,
                    obc_host_core::frame::Scene { reader: self.reader, route: route.as_ref() },
                    self.peak.panorama(),
                    (self.size.0 as f32, self.size.1 as f32),
                    device_rgb888,
                );
            }
        };
        hook(app, ScriptHook::Tick, now);
        apply_script(app, script, now, &mut hook)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::io::ByteSource;
    use obc_host_core::{flat_store::HostStore, FlatRideRecorder};
    use obc_replay::gpx::TrackPoint;

    #[test]
    fn after_replay_buttons_save_the_same_recording_with_its_last_fix() {
        let owner = HostStore::memory().unwrap();
        let source =
            map_file::MapSource::load_single(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/grimsel-demo.obcm")).unwrap();
        let map = map_file::LoadedMap::open_in(source, &owner).unwrap();
        let reader = map.reader();
        let mut routes = RouteStore::new(owner.clone(), &[]).unwrap();
        let mut rides = RideStore::new(owner.clone()).unwrap();
        let mut trips = TripStore::new(owner.clone()).unwrap();
        let exports = std::env::temp_dir().join(format!(
            "obc-after-script-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        let mut tracks = TrackStore::new(FlatRideRecorder::new(owner.clone()).unwrap(), owner.clone(), &exports);
        let mut stores = Stores { routes: &mut routes, rides: &mut rides, trips: &mut trips, tracks: &mut tracks };
        let mut host = HostLoop::new();
        let mut route = ActiveRouteSession::new();
        let mut elevation = map.elevation();
        let mut platform = HeadlessPlatform::default();
        let mut peak = peak_view::Runtime::new(&map, None);
        let mut player = Some(GpxPlayer::new(Track {
            points: vec![
                TrackPoint { lat: 46_700_000, lon: 8_300_000, ele: Some(100.0), t: 0.0 },
                TrackPoint { lat: 46_700_000, lon: 8_300_200, ele: Some(100.0), t: 10.0 },
            ],
        }));
        let mut app = App::new(AppState::new(8_300_000, 46_700_000, 0.01));
        let mut script = Session {
            diagnostics: diagnostics::Diagnostics::default(),
            host: &mut host,
            route: &mut route,
            stores: &mut stores,
            map: &map,
            reader: &reader,
            elevation: &mut *elevation,
            platform: &mut platform,
            peak: &mut peak,
            player: &mut player,
            size: (240, 320),
        };
        let before = script.run(&mut app, "p p f", 100, RideClock(0), Some(0.0));
        assert!(app.recording());
        let mut baro = BaroSensor::new();
        let player = script.player.as_mut().unwrap();
        player.play();
        for _ in 0..4 {
            let (ride, sensors) = headless_replay_advance(player, &mut baro, 1.0, 0.0);
            let mut plan = script.host.pass(
                &mut app,
                obc_app::device_core::PassClock { ride, ui: InputClock(before) },
                &[],
                sensors,
                None,
                gui::SIM_SUPPORT,
            );
            script.host.execute(
                &mut app,
                &mut plan,
                script.route,
                script.stores.routes,
                script.stores.rides,
                script.stores.tracks,
                script.stores.trips,
                map.planner_map(),
                script.elevation,
                script.platform,
            );
        }
        let after = script.run(&mut app, "T p f d h f", before, RideClock(4000), None);
        assert!(after > before);
        assert_eq!(script.player.as_ref().unwrap().time(), 4.0);
        assert!(!app.recording());
        let catalog = script.stores.rides.catalog();
        assert_eq!(catalog.len(), 1);
        let source = owner.open(obc_storage::flat::ObjectId(catalog[0].id), obc_storage::flat::Revision(1)).unwrap();
        let info = obc_route::RideInfo::read(&source).unwrap();
        assert!(info.point_count >= 4);
        let mut previous = 0;
        let mut segments = 0;
        for index in 0..info.point_count {
            let mut bytes = [0; obc_formats::track::RECORD_LEN];
            source.read_at(index as u64 * bytes.len() as u64, &mut bytes).unwrap();
            let point = obc_formats::track::decode_record(&bytes);
            assert!(point.t_ms >= previous);
            previous = point.t_ms;
            segments += usize::from(point.segment_start);
            if index == info.point_count - 1 {
                assert_eq!(point.lon, 8_300_080);
            }
        }
        assert_eq!(previous, 4000);
        assert_eq!(segments, 1);
        std::fs::remove_dir_all(exports).unwrap();
    }
}
