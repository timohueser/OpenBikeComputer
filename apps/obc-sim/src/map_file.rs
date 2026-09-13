//! Importing a native OBCM into an owned temporary flat card.
//! The original path remains the anchor for the terrain sidecar and display name.

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
    path: PathBuf,
    map: FlatMap,
}

impl LoadedMap {
    #[cfg(test)]
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
        Ok(Self { name, path: source.path, map })
    }

    pub fn reopen(store: &obc_host_core::flat_store::HostStore) -> Result<Self, MapError> {
        let map = FlatMap::open_only_in(store)?;
        Ok(Self { name: "Card map".into(), path: PathBuf::new(), map })
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
        if self.path.as_os_str().is_empty() {
            Box::new(obc_route::NullElevation)
        } else {
            obc_host_core::terrain::resolve(&self.path)
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

/// Everything a frame draws its map from: the map reader (map plane plus POI/hours) and the active
/// route.
#[derive(Clone, Copy)]
pub struct Scene<'a, 'd> {
    pub reader: &'a obc_reader::Reader<'d>,
    pub route: Option<&'a obc_route::RouteReader<'a>>,
}

/// Draw one whole frame through the app's real generic scene seam. `scratch` is the caller's
/// render scratch — the app borrows it for the call and keeps nothing (#1146).
#[allow(clippy::too_many_arguments)]
pub fn render_frame<D, F>(
    app: &mut obc_app::App,
    scratch: &mut obc_render::RenderScratch,
    target: &mut D,
    scene: Scene<'_, '_>,
    rain: Option<&mut dyn obc_render::RainOverlaySource>,
    weather: Option<&obc_app::WeatherSnapshot>,
    peak_view: Option<&obc_app::peak_view::Panorama>,
    (w, h): (f32, f32),
    color_fn: F,
) -> RenderStats
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    let Scene { reader, route } = scene;
    // A real microsecond clock so the returned stats carry the per-stage map timings (including
    // `rain_us`, the WX10 overlay's own wall time) — the panel and the headless log both read them.
    let clock = StdClock(std::time::Instant::now());
    let stats = app.render_scene_map_rain_timed(
        Some(scratch),
        target,
        Some(reader),
        Some(reader),
        route,
        rain,
        weather,
        peak_view,
        w,
        h,
        &color_fn,
        &clock,
    );
    app.render_overlay(target, w, h, &color_fn);
    stats
}

/// Microsecond [`obc_render::Clock`] over a host `Instant` — the sim's stage-timing source.
struct StdClock(std::time::Instant);

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
