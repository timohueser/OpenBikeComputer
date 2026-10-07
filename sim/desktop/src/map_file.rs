//! Native map import and exact persisted-card reopen.
//! Rendering and elevation retain the same map revision. Files are import inputs only.

use std::fmt;
use std::path::{Path, PathBuf};

use embedded_graphics::prelude::*;
use obc_host_core::flat_map::{FlatMap, MapError};
use obc_reader::{MapTables, Reader};
use obc_render::RenderStats;

#[derive(Debug)]
pub enum LoadError {
    Read(PathBuf, std::io::Error),
    NotObcm(obc_reader::Error),
    Import(PathBuf, MapError),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(path, err) => write!(f, "cannot read {}: {err}", path.display()),
            Self::NotObcm(err) => write!(f, "invalid OBCM file: {err:?}"),
            Self::Import(path, err) => write!(f, "cannot import {}: {err}", path.display()),
        }
    }
}

/// The open input and its path. Import reads through a bounded buffer.
pub struct MapSource {
    file: std::fs::File,
    name: String,
    path: PathBuf,
}

impl MapSource {
    pub fn load_single(path: &str) -> Result<Self, LoadError> {
        let path = Path::new(path);
        let file = std::fs::File::open(path).map_err(|err| LoadError::Read(path.to_path_buf(), err))?;
        Ok(Self { file, name: file_name(path), path: path.to_path_buf() })
    }

    pub fn display_name(&self) -> String {
        Path::new(&self.name).file_stem().map_or_else(|| self.name.clone(), |s| s.to_string_lossy().into_owned())
    }
}

pub struct LoadedMap {
    name: String,
    map: FlatMap,
}

impl LoadedMap {
    pub fn open(source: MapSource) -> Result<Self, LoadError> {
        let store = obc_host_core::flat_store::HostStore::temporary()
            .map_err(|error| LoadError::Import(source.path.clone(), MapError::from(error)))?;
        Self::open_in(source, &store)
    }

    pub fn open_in(source: MapSource, store: &obc_host_core::flat_store::HostStore) -> Result<Self, LoadError> {
        let name = source.display_name();
        let map = FlatMap::from_file_in(store, source.file).map_err(|err| match err {
            MapError::Format(err) => LoadError::NotObcm(err),
            other => LoadError::Import(source.path.clone(), other),
        })?;
        Ok(Self { name, map })
    }

    pub fn reopen(store: &obc_host_core::flat_store::HostStore) -> Result<Self, MapError> {
        let map = FlatMap::open_only_in(store)?;
        Ok(Self { name: "Card map".into(), map })
    }

    pub fn route_attribution_key(&self) -> obc_formats::obcr::RouteSourceKey {
        let source = self.map_source();
        obc_formats::obcr::RouteSourceKey {
            store: source.store_id().0,
            object: source.id().0,
            revision: source.revision().0,
        }
    }

    pub fn display_name(&self) -> &str {
        &self.name
    }

    pub fn map_source(&self) -> obc_host_core::flat_store::ObjectSource {
        self.map.source()
    }
    pub fn tables(&self) -> &MapTables {
        self.map.tables()
    }

    pub fn elevation(&self) -> Box<dyn obc_route::ElevationSource> {
        match obc_host_core::terrain::FlatElevation::open(&self.map) {
            Ok(Some(elevation)) => elevation,
            Ok(None) => Box::new(obc_route::NullElevation),
            Err(error) => {
                eprintln!("terrain: embedded map terrain unavailable ({error:?}); routes stay flat");
                Box::new(obc_route::NullElevation)
            }
        }
    }

    pub fn planner_map(&self) -> &obc_host_core::flat_map::FlatMap {
        &self.map
    }

    pub fn reader(&self) -> Reader<'_> {
        self.map.reader()
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned()
}

/// One whole frame for the sim's one-shot drivers, which own the photo phase nowhere else: it runs
/// the capture phase whenever the photo screen is in its base state. The interactive hosts pass
/// their own `FramePhoto` to [`obc_host_core::frame::render`] instead.
pub fn render_frame<D, F>(
    app: &mut obc_app::App,
    scratch: &mut obc_render::RenderScratch,
    target: &mut D,
    scene: obc_host_core::frame::Scene<'_, '_>,
    peak_view: Option<&obc_app::peak_view::Panorama>,
    (w, h): (f32, f32),
    color_fn: F,
) -> RenderStats
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    let mut photo = app.photo_base_active().then(obc_host_core::photo::Preparer::default);
    obc_host_core::frame::render(
        app,
        scratch,
        target,
        scene,
        peak_view,
        (w, h),
        &color_fn,
        &StdClock(std::time::Instant::now()),
        photo.as_mut().map(|p| p.capture()),
    )
}

/// Microsecond [`obc_render::Clock`] over a host `Instant` — the sim's stage-timing source.
pub struct StdClock(pub std::time::Instant);

impl obc_render::Clock for StdClock {
    fn now_us(&self) -> u64 {
        self.0.elapsed().as_micros() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obcm_testkit::{build_file, pack_line, seal, LodSpec, Style};

    const ASSEMBLY: (i32, i32, i32, i32) = (0, 0, 4000, 4000);
    const STYLES: &[Style] = &[(1, 0, 0x07E0, 1, 1, false, None)];

    /// A scratch directory of this test's own, removed by `Dir`'s drop.
    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            Dir(obcm_testkit::scratch::scratch_dir("obc-sim-map", tag))
        }
        fn write(&self, name: &str, bytes: &[u8]) {
            std::fs::write(self.0.join(name), bytes).expect("write fixture");
        }
        fn path(&self, name: &str) -> String {
            self.0.join(name).to_string_lossy().into_owned()
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The smallest map that draws something: one style, one rung, one chunk.
    fn map_file() -> Vec<u8> {
        let chunk = seal(pack_line(1, 100, 100, &[(50, 50), (50, -50)]), 4096);
        build_file(
            ASSEMBLY,
            STYLES,
            &[LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![chunk], chunk_size: 4096 }],
        )
    }

    /// The path: read one `.obcm`, parse its tables, and hand the app path a reader over it.
    #[test]
    fn a_map_file_loads_and_opens() {
        let dir = Dir::new("open");
        dir.write("MAP.OBCM", &map_file());
        let source = MapSource::load_single(&dir.path("MAP.OBCM")).expect("a map file loads");
        assert_eq!(source.display_name(), "MAP");
        let map = LoadedMap::open(source).expect("a map file opens");
        assert_eq!(map.reader().lods().len(), 1);
    }

    /// Load failure is fatal and names the file, so the exiting caller says which one.
    #[test]
    fn a_missing_map_file_refuses_to_load_and_names_the_file() {
        let dir = Dir::new("missing");
        let Err(err) = MapSource::load_single(&dir.path("MAP.OBCM")) else { panic!("a missing map is refused") };
        assert!(matches!(err, LoadError::Read(..)), "{err}");
        assert!(err.to_string().contains("MAP.OBCM"), "{err}");
    }

    /// A file that is not OBCM is refused at open, never rendered as an empty map.
    #[test]
    fn a_non_obcm_file_refuses_to_open() {
        let dir = Dir::new("garbage");
        dir.write("MAP.OBCM", b"not a map");
        let source = MapSource::load_single(&dir.path("MAP.OBCM")).expect("the file is readable");
        let Err(err) = LoadedMap::open(source).map(|_| ()) else { panic!("garbage does not open as OBCM") };
        assert!(matches!(err, LoadError::NotObcm(_)), "{err}");
    }
}

#[cfg(all(test, unix))]
mod terrain_tests {
    use super::*;
    use crate::card::Session;
    use obc_app::{
        device_core::{PassClock, PlatformSupport},
        navigator::NavigatorOutcome,
        App, AppState,
    };
    use obc_formats::io::ByteSource;
    use obc_host_core::{ActiveRouteSession, HostLoop, RouteRepository};
    use obc_ports::{AltimeterSource, Fix, InputClock, LocationSource, RideClock, Sensors};

    struct Location;
    impl LocationSource for Location {
        fn poll(&mut self) -> Option<Fix> {
            Some(Fix { lat: 512, lon: 512, course: Some(0.0), speed_mps: Some(0.0) })
        }
    }
    struct Altimeter;
    impl AltimeterSource for Altimeter {
        fn poll(&mut self) -> Option<f32> {
            Some(80.0)
        }
    }

    fn plan(session: &mut Session, replace: Option<&Path>) -> Option<Vec<u8>> {
        let mut app = App::new_idle(AppState::new(512, 512, 1.0));
        app.set_map_nav_graph(true);
        let mut elevation = session.map.elevation();
        // The native altitude drain uses this same retained elevation object.
        for now in 1..=40 {
            app.tick(
                RideClock(now * 1000),
                Sensors { altimeter: Some(&mut Altimeter), ..Sensors::new(&mut Location) },
                None,
            );
            assert!(app.sample_terrain(&mut *elevation));
        }
        assert!((app.recorder.current_elevation_m().unwrap() - 5.0).abs() < 1.0);
        let mut host = HostLoop::new();
        host.facts().note_store_revision(session.routes.store_scope().unwrap());
        let mut active = ActiveRouteSession::new();
        let before = session.routes.ids().len();
        let mut replaced = false;
        let mut source_changed = false;
        for step in 0..100 {
            if step == 2 {
                assert!(app.debug_start_nav((512, 512), (8192, 8192), "Terrain plan"));
            }
            let now = 50_000 + step * 100;
            let mut pass = host.pass(
                &mut app,
                PassClock { ride: RideClock(now), ui: InputClock(now) },
                &[],
                Sensors::new(&mut Location),
                None,
                PlatformSupport::default(),
            );
            host.execute(
                &mut app,
                &mut pass,
                &mut active,
                &mut session.routes,
                &mut session.rides,
                &mut session.tracks,
                &mut session.trips,
                session.map.planner_map(),
                &mut *elevation,
                &mut (),
            );
            if let Some(outcome) = host.outcomes().navigator.take() {
                if let Some(path) = replace.filter(|_| matches!(outcome, NavigatorOutcome::Acquired { .. })) {
                    session.map.planner_map().replace_from_file(std::fs::File::open(path).unwrap()).unwrap();
                    replaced = true;
                }
                source_changed |= matches!(
                    outcome,
                    NavigatorOutcome::Failed { error: obc_app::navigator::NavigatorError::SourceChanged, .. }
                );
                host.outcomes().navigator.try_put(outcome).unwrap();
            }
            if step > 2 && !host.is_planning() && session.routes.ids().len() > before {
                let source = session.routes.source(*session.routes.ids().last().unwrap()).unwrap();
                let summary = obc_route::RouteSummary::read(&source).unwrap();
                assert!(summary.climb_m > 0);
                let mut bytes = vec![0; source.len() as usize];
                source.read_at(0, &mut bytes).unwrap();
                return Some(bytes);
            }
            if source_changed && !host.is_planning() {
                assert!(replaced);
                assert_eq!(session.routes.ids().len(), before);
                return None;
            }
        }
        panic!("planner did not reach its acknowledged terminal phase");
    }

    #[test]
    fn native_session_plans_with_the_same_terrain_after_reopen_and_rejects_replacement() {
        let directory = obcm_testkit::scratch::scratch_dir("native-terrain", "planner");
        let input = directory.join("map.obcm");
        let card = directory.join("card.obc");
        std::fs::write(&input, obcm_testkit::terrain::map(0)).unwrap();
        let mut session = Session::load(&mut crate::Args {
            map: input.to_string_lossy().into_owned(),
            create_card: Some(card.to_string_lossy().into_owned()),
            routes_dir: Some(directory.to_string_lossy().into_owned()),
            tracks_dir: Some(directory.to_string_lossy().into_owned()),
            ..crate::Args::default()
        })
        .unwrap();
        let source = session.map.map_source();
        let identity = (source.store_id(), source.id(), source.revision());
        let mut pois = heapless::Vec::new();
        session.map.reader().nearest_pois(obc_reader::PoiCategory::Water, (512, 512), &mut pois).unwrap();
        assert_eq!(pois.len(), 1, "the CLI fixture exposes one real Route here destination");
        assert_eq!(pois[0].name, "Terrain goal");
        let before = plan(&mut session, None).unwrap();
        drop(source);
        drop(session);
        std::fs::remove_file(&input).unwrap();
        let mut session = Session::load(&mut crate::Args {
            card: Some(card.to_string_lossy().into_owned()),
            ..crate::Args::default()
        })
        .unwrap();
        let source = session.map.map_source();
        assert_eq!((source.store_id(), source.id(), source.revision()), identity);
        assert_eq!(plan(&mut session, None).unwrap(), before, "emitted geometry, heights and totals are identical");
        std::fs::write(&input, obcm_testkit::terrain::map(100)).unwrap();
        assert!(plan(&mut session, Some(&input)).is_none());
        drop(source);
        drop(session);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
