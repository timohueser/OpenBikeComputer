//! Opening a map from disk: one `.obcm` file, read once and held for the process lifetime.
//!
//! One map is one file. [`MapSource`] is its bytes plus the path it came from — the anchor the
//! `.obcd` terrain sidecar is resolved against (EL7) — and [`LoadedMap`] pairs that file with the
//! tables, chunk cache and reader parsed once at startup, exactly as the device parses them once
//! at boot.
//!
//! Failure is fatal at load time: a file that cannot be read, or that does not parse as OBCM, exits
//! the caller non-zero rather than rendering a partial map.
//!
//! `obc-app` consumes the map through its generic `MapScene` map-plane seam; POI, hours and routing
//! take the same [`LoadedMap::reader`].

use std::fmt;
use std::path::{Path, PathBuf};

use embedded_graphics::prelude::*;
use obc_reader::{MapCache, MapTables, Reader, SliceSource};
use obc_render::RenderStats;

/// Why a map could not be opened. Both variants are fatal.
#[derive(Debug)]
pub enum LoadError {
    /// The map file could not be read.
    Read(PathBuf, std::io::Error),
    /// The file is not a parseable OBCM.
    NotObcm(obc_reader::Error),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Read(path, err) => write!(f, "cannot read {}: {err}", path.display()),
            LoadError::NotObcm(err) => write!(f, "invalid OBCM file: {err:?}"),
        }
    }
}

/// A map's bytes: one `.obcm` file, read whole.
pub struct MapSource {
    /// The file's bytes.
    file: Vec<u8>,
    /// The filename it was read from — for the display name and honest error text.
    name: String,
    /// The path the map was opened from — what [`terrain`](obc_host_core::terrain) resolves the
    /// `.obcd` sidecar against (EL7).
    path: PathBuf,
}

impl MapSource {
    /// Read a `.obcm` map.
    pub fn load_single(path: &str) -> Result<MapSource, LoadError> {
        let path = Path::new(path);
        let bytes = std::fs::read(path).map_err(|err| LoadError::Read(path.to_path_buf(), err))?;
        Ok(MapSource { file: bytes, name: file_name(path), path: path.to_path_buf() })
    }

    /// The path this map was opened from — the anchor the terrain sidecar is resolved against.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The map's bytes as a [`SliceSource`]. Keep it alive for as long as the readers built over
    /// it — they borrow the source, not the bytes.
    pub fn source(&self) -> SliceSource<'_> {
        SliceSource(&self.file)
    }

    /// The name to show as "the map": the file's stem.
    pub fn display_name(&self) -> String {
        Path::new(&self.name).file_stem().map_or_else(|| self.name.clone(), |s| s.to_string_lossy().into_owned())
    }
}

/// A map opened once and held for the process lifetime: its bytes and the readers' inputs.
///
/// Everything here is leaked on purpose. The simulator owns its map for the whole run either way,
/// and `'static` is what lets the parsed tables and the session-long chunk cache live in struct
/// fields rather than be rebuilt inside every borrow — the borrow chain (bytes → source → tables)
/// is otherwise self-referential.
pub struct LoadedMap {
    pub source: &'static MapSource,
    slice: &'static SliceSource<'static>,
    tables: &'static MapTables,
    cache: &'static MapCache,
}

impl LoadedMap {
    /// Parse the map's tables — before a single frame renders.
    pub fn open(source: MapSource) -> Result<LoadedMap, LoadError> {
        let source: &'static MapSource = Box::leak(Box::new(source));
        let slice: &'static SliceSource<'static> = Box::leak(Box::new(source.source()));
        let tables: &'static MapTables = Box::leak(Box::new(MapTables::parse(slice).map_err(LoadError::NotObcm)?));
        let cache: &'static MapCache = Box::leak(Box::new(MapCache::new()));
        Ok(LoadedMap { source, slice, tables, cache })
    }

    /// The map's parsed tables — the style table and the LOD pyramid.
    pub fn tables(&self) -> &'static MapTables {
        self.tables
    }

    /// The map's terrain (EL7): its `.obcd` sidecar mounted through the shared host resolver, or
    /// the null source when there is none. Called **once** per run, right after the map opens —
    /// the simulator holds one elevation source for the session exactly as the device does.
    pub fn elevation(&self) -> Box<dyn obc_route::ElevationSource> {
        obc_host_core::terrain::resolve(self.source.path())
    }

    /// The reader the whole app path takes — map plane, nav, POI, hours and routing alike.
    pub fn reader(&self) -> Reader<'_> {
        Reader::new(self.slice, self.tables, self.cache)
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
