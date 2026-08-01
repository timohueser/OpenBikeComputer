//! Opening a map from disk: one `.obcm` file, or a whole OBCA **volume set** (`OBCA_Spec.md` §5).
//!
//! A set is one logical map spread over `1..=32` OBCM shards plus an `MS<id>.OBS` manifest that
//! names them. [`MapSource`] holds either shape as the same thing — a list of files in manifest
//! index order — so the rest of the simulator is written once: index `core_index()` for the shard
//! nav, POI and routing read (§5.1), and hand the whole list to [`obc_reader::MountedSet`] when a
//! manifest is present.
//!
//! §5.4 admits no partial mount. Every failure below is fatal at load time and the caller exits
//! non-zero — a missing shard, a shard that is not the recorded size, a manifest that does not
//! validate all read as *map incomplete*, never as a smaller map.
//!
//! `obc-app` consumes the mounted set directly through its generic `MapScene` map-plane seam. POI,
//! hours and routing still receive [`LoadedMap::reader`], which is the core shard by construction.

use std::fmt;
use std::path::{Path, PathBuf};

use embedded_graphics::prelude::*;
use obc_formats::io::ByteSource;
use obc_formats::obcs::{self, ManifestError, Role, SetManifest};
use obc_reader::{MapCache, MapTables, MountError, MountedSet, Reader, SetShards, SliceSource};
use obc_render::RenderStats;

/// Why a map (single file or set) could not be opened. Every variant is fatal — see §5.4.
#[derive(Debug)]
pub enum LoadError {
    /// A file named by the manifest (or the manifest itself) could not be read.
    Read(PathBuf, std::io::Error),
    /// `--set` was not pointed at an `MS<id>.OBS` manifest name (§5.2).
    NotAManifestName(PathBuf),
    /// The manifest itself failed §5.3.
    Manifest(ManifestError),
    /// The manifest's own id/shard count cannot express a derived §5.2 filename.
    UnnameableShard(u16, usize),
    /// The core file (a single map, or a set's core shard) is not a parseable OBCM.
    NotObcm(obc_reader::Error),
    /// The set did not mount (§5.3's reader half); the message names the offending file.
    Mount(MountError, String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Read(path, err) => write!(f, "cannot read {}: {err}", path.display()),
            LoadError::NotAManifestName(path) => write!(
                f,
                "--set wants a volume-set manifest named MS<id>.OBS (OBCA §5.2), not {}",
                path.file_name().unwrap_or(path.as_os_str()).to_string_lossy()
            ),
            LoadError::Manifest(err) => write!(f, "invalid OBCS manifest: {err:?} (OBCA §5.3)"),
            LoadError::UnnameableShard(id, index) => {
                write!(f, "set {id} cannot name shard {index}: no derived 8.3 filename (OBCA §5.2)")
            }
            LoadError::NotObcm(err) => write!(f, "invalid OBCM file: {err:?}"),
            LoadError::Mount(err, what) => write!(f, "map incomplete: {what} ({err:?}, OBCA §5.3/§5.4)"),
        }
    }
}

/// A map's bytes: a single `.obcm`, or every shard of a set in manifest index order.
pub struct MapSource {
    /// One entry per file, in manifest index order. A single map is a one-element list.
    files: Vec<Vec<u8>>,
    /// The filename each entry was read from — for diagnostics and honest error text.
    names: Vec<String>,
    /// The parsed manifest and the set id its filename carried; `None` for a single map.
    set: Option<(u16, SetManifest)>,
}

impl MapSource {
    /// Read a single `.obcm` map — the pre-set path, unchanged.
    pub fn load_single(path: &str) -> Result<MapSource, LoadError> {
        let path = Path::new(path);
        let bytes = std::fs::read(path).map_err(|err| LoadError::Read(path.to_path_buf(), err))?;
        Ok(MapSource { files: vec![bytes], names: vec![file_name(path)], set: None })
    }

    /// Read a whole volume set from the `MS<id>.OBS` manifest at `path`: parse the manifest, derive
    /// every shard's §5.2 filename, and read them from the manifest's own directory.
    ///
    /// This is presence + readability only; the §5.3 size/header/bbox/style obligations are
    /// [`mount`](MapSource::mount)'s, because only a parsed core can check them.
    pub fn load_set(path: &str) -> Result<MapSource, LoadError> {
        let path = Path::new(path);
        let name = file_name(path);
        let id = obcs::parse_manifest_name(name.as_bytes())
            .ok_or_else(|| LoadError::NotAManifestName(path.to_path_buf()))?;
        let raw = std::fs::read(path).map_err(|err| LoadError::Read(path.to_path_buf(), err))?;
        let manifest = obcs::parse(&raw).map_err(LoadError::Manifest)?;

        let dir = path.parent().unwrap_or(Path::new("."));
        let mut files = Vec::with_capacity(manifest.shard_count());
        let mut names = Vec::with_capacity(manifest.shard_count());
        for index in 0..manifest.shard_count() {
            let derived = obcs::shard_name(id, index).ok_or(LoadError::UnnameableShard(id, index))?;
            let shard = derived.as_str().to_string();
            let bytes = read_shard(dir, &shard)?;
            files.push(bytes);
            names.push(shard);
        }
        Ok(MapSource { files, names, set: Some((id, manifest)) })
    }

    /// The manifest, or `None` for a single-file map.
    pub fn manifest(&self) -> Option<&SetManifest> {
        self.set.as_ref().map(|(_, manifest)| manifest)
    }

    /// Index of the file nav, POI, hours and routing read (§5.1) — the core shard, or the only
    /// file of a single map.
    pub fn core_index(&self) -> usize {
        self.manifest().map_or(0, |manifest| manifest.core_shard())
    }

    /// A [`SliceSource`] per file, in manifest index order. Keep the returned vector alive for as
    /// long as the readers built over it — `MountedSet` borrows the sources, not the bytes.
    pub fn sources(&self) -> Vec<SliceSource<'_>> {
        self.files.iter().map(|bytes| SliceSource(bytes)).collect()
    }

    /// The name to show as "the map": the set's display name (falling back to the manifest's own
    /// filename stem) or the single file's stem.
    pub fn display_name(&self) -> String {
        match &self.set {
            Some((id, manifest)) => manifest.name().map(str::to_string).unwrap_or_else(|| format!("MS{id}")),
            None => Path::new(&self.names[0])
                .file_stem()
                .map_or_else(|| self.names[0].clone(), |s| s.to_string_lossy().into_owned()),
        }
    }

    /// Human text for a [`MountError`], naming the file it is about — the difference between
    /// "map incomplete" and a user who knows which shard to re-copy.
    fn explain(&self, err: MountError) -> String {
        let named = |index: u8| self.names.get(index as usize).cloned().unwrap_or_else(|| format!("shard {index}"));
        match err {
            MountError::Manifest(inner) => format!("the manifest does not validate ({inner:?})"),
            MountError::ShardCount => "the manifest names more shards than were read".to_string(),
            MountError::Handles(cap) => {
                format!("the set names {} shards; this host mounts at most {cap}", self.files.len())
            }
            MountError::Size(at) => {
                let want = self.manifest().and_then(|m| m.shards().get(at as usize)).map_or(0, |s| s.bytes);
                let got = self.files.get(at as usize).map_or(0, |f| f.len());
                format!("{} is {got} B, the manifest records {want} B (still being copied?)", named(at))
            }
            MountError::Header(at) => format!("{} does not open as OBCM at the set's version", named(at)),
            MountError::Bbox(at) => format!("{}'s header bbox is not the bbox the manifest records", named(at)),
            MountError::Ladder(at) => {
                format!("{}'s LOD ladder is not the core's (OBCA §5.1: every shard lists the full ladder)", named(at))
            }
            MountError::Styles(at) => {
                format!("{} carries a different style table than the core (OBCA §4.7)", named(at))
            }
        }
    }

    /// The startup banner's set half: what mounted, from how many files, and what each one is.
    pub fn describe_set(&self, mounted: &MountedSet<'_>) -> String {
        let Some((id, manifest)) = &self.set else { return String::new() };
        let mut out = format!(
            "OBCA set MS{id} \"{}\" | {} shard(s), {} B total | core {} | schema rev {}{}",
            manifest.name().unwrap_or("(unnamed)"),
            mounted.shard_count(),
            mounted.total_bytes(),
            self.names[self.core_index()],
            manifest.schema_revision,
            if manifest.is_single_file() { " | single-file fast path (§5.5)" } else { "" },
        );
        for (index, shard) in manifest.shards().iter().enumerate() {
            let role = match mounted.role_of(index) {
                Some(Role::Core) => "core",
                Some(Role::Geometry) => "geometry",
                Some(Role::Coarse) => "coarse",
                None => "?",
            };
            out.push_str(&format!(
                "\n  [{index:02}] {:<12} {role:<8} {:>10} B | bbox {},{} .. {},{}",
                self.names[index],
                shard.bytes,
                shard.bbox.min_lon,
                shard.bbox.min_lat,
                shard.bbox.max_lon,
                shard.bbox.max_lat
            ));
        }
        out
    }
}

/// Read one derived shard name from `dir`.
///
/// The §5.2 name is **exact**: upper-case 8.3, derived and never stored. There is deliberately no
/// case-insensitive fallback — the device's FAT layer creates short names only, so a lower-cased
/// `ms7s00.obm` is a file the firmware would not find, and a simulator that opened it anyway would
/// report a set works when on glass it does not.
fn read_shard(dir: &Path, name: &str) -> Result<Vec<u8>, LoadError> {
    let path = dir.join(name);
    std::fs::read(&path).map_err(|err| LoadError::Read(path, err))
}

/// A map opened once and held for the process lifetime: its bytes, the readers' inputs, and — for
/// a volume set — the **mount**.
///
/// The sim mounts a set exactly once, at startup, like the device does at boot. That matters
/// beyond the wasted work: §5.3's checks stream each shard's whole style region, and holding the
/// mount for the session is precisely the shape a device must use (its shard file handles are open
/// for the mount's lifetime). A simulator that re-mounted per frame would quietly model a pattern
/// no device can afford.
///
/// Everything here is leaked on purpose. The simulator owns its map for the whole run either way,
/// and `'static` is what lets a [`MountedSet`] live in a struct field rather than be rebuilt inside
/// every borrow — the borrow chain (bytes → sources → tables → mount) is otherwise self-referential.
pub struct LoadedMap {
    pub source: &'static MapSource,
    sources: &'static [SliceSource<'static>],
    tables: &'static MapTables,
    cache: &'static MapCache,
    set: Option<MountedSet<'static>>,
}

impl LoadedMap {
    /// Parse the core's tables and, for a set, mount it — §5.4's "no partial mount" applies here,
    /// before a single frame renders.
    pub fn open(source: MapSource) -> Result<LoadedMap, LoadError> {
        let source: &'static MapSource = Box::leak(Box::new(source));
        let sources: &'static [SliceSource<'static>] = Box::leak(source.sources().into_boxed_slice());
        let tables: &'static MapTables =
            Box::leak(Box::new(MapTables::parse(&sources[source.core_index()]).map_err(LoadError::NotObcm)?));
        let cache: &'static MapCache = Box::leak(Box::new(MapCache::new()));
        let set = match source.manifest() {
            Some(manifest) => {
                let refs: &'static [&'static dyn ByteSource] =
                    Box::leak(sources.iter().map(|s| s as &dyn ByteSource).collect::<Vec<_>>().into_boxed_slice());
                let store: &'static mut SetShards<'static> = Box::leak(Box::new(SetShards::new()));
                Some(
                    MountedSet::mount(store, manifest, refs, tables, cache)
                        .map_err(|err| LoadError::Mount(err, source.explain(err)))?,
                )
            }
            None => None,
        };
        Ok(LoadedMap { source, sources, tables, cache, set })
    }

    /// The core's parsed tables — one style table for the whole set (§4.7).
    pub fn tables(&self) -> &'static MapTables {
        self.tables
    }

    /// The mounted set, or `None` for a single map.
    pub fn set(&self) -> Option<&MountedSet<'static>> {
        self.set.as_ref()
    }

    /// The reader the whole app path takes: a set's **core** shard (§5.1 — nav, POI, hours and
    /// routing read it and only it), or the single map's one file.
    pub fn reader(&self) -> Reader<'_> {
        match &self.set {
            Some(set) => set.core_reader(),
            None => Reader::new(&self.sources[0], self.tables, self.cache),
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned()
}

/// Everything a frame draws its map from: the scene (a mounted set when present), the core reader
/// for POI/hours, and the active route.
pub struct Scene<'a, 'm, 'd> {
    pub set: Option<&'a MountedSet<'m>>,
    pub reader: &'a obc_reader::Reader<'d>,
    pub route: Option<&'a obc_route::RouteReader<'a>>,
}

/// Draw one whole frame through the app's real generic scene seam.
pub fn render_frame<D, F>(
    app: &mut obc_app::App,
    target: &mut D,
    scene: Scene<'_, '_, '_>,
    (w, h): (f32, f32),
    color_fn: F,
) -> RenderStats
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    let Scene { set, reader, route } = scene;
    match set {
        Some(set) => app.render_scene_frame(target, set, reader, route, w, h, color_fn),
        None => app.render_frame(target, reader, route, w, h, color_fn),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::io::ByteSink;
    use obc_formats::obcs::Role;
    use obc_route::{RouteIndex, RouteReader};
    use obcm_testkit::set::{build_set, empty_lod, ShardSpec};
    use obcm_testkit::{pack_line, seal, LodSpec, Style};

    const ASSEMBLY: (i32, i32, i32, i32) = (0, 0, 4000, 4000);
    const STYLES: &[Style] = &[(1, 0, 0x07E0, 1, 1, false, None)];

    /// A scratch directory of this test's own, removed by `Dir`'s drop.
    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let path = std::env::temp_dir().join(format!("obc-sim-set-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("scratch dir");
            Dir(path)
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

    fn chunk() -> Vec<u8> {
        seal(pack_line(1, 100, 100, &[(50, 50), (50, -50)]), 4096)
    }

    /// The smallest true multi-file set §5.3 admits: a core carrying no ladder, plus one shard of
    /// each other role spanning the whole assembly. Every shard lists the full two-rung ladder with
    /// the rungs it does not carry written empty (§5.1).
    fn fixture() -> obcm_testkit::set::SetFixture {
        let rung =
            |chunk: Vec<u8>, max_mpp: f32| LodSpec { max_mpp, index: vec![0], chunks: vec![chunk], chunk_size: 4096 };
        build_set(
            ASSEMBLY,
            STYLES,
            0,
            &[
                ShardSpec { role: Role::Core, bbox: ASSEMBLY, lods: vec![empty_lod(f32::INFINITY), empty_lod(4.0)] },
                ShardSpec {
                    role: Role::Coarse,
                    bbox: ASSEMBLY,
                    lods: vec![rung(chunk(), f32::INFINITY), empty_lod(4.0)],
                },
                ShardSpec {
                    role: Role::Geometry,
                    bbox: ASSEMBLY,
                    lods: vec![empty_lod(f32::INFINITY), rung(chunk(), 4.0)],
                },
            ],
        )
    }

    /// Lay a set down under its §5.2 derived names and mount it exactly as the simulator does.
    fn stage(dir: &Dir, id: u16, fixture: &obcm_testkit::set::SetFixture) {
        dir.write(obcs::manifest_name(id).unwrap().as_str(), &fixture.manifest);
        for (index, bytes) in fixture.shards.iter().enumerate() {
            dir.write(obcs::shard_name(id, index).unwrap().as_str(), bytes);
        }
    }

    /// Open the map exactly as `main` does — parse the core's tables and mount the set once.
    fn mounts(source: MapSource) -> Result<(), LoadError> {
        LoadedMap::open(source).map(|_| ())
    }

    #[test]
    fn a_complete_set_loads_from_its_derived_filenames_and_mounts() {
        let dir = Dir::new("complete");
        let fixture = fixture();
        stage(&dir, 7, &fixture);
        let source = MapSource::load_set(&dir.path("MS7.OBS")).expect("a complete set loads");
        assert_eq!(source.manifest().expect("a set").shard_count(), 3);
        assert_eq!(source.core_index(), 0);
        let map = LoadedMap::open(source).expect("a complete set mounts");
        let set = map.set().expect("a set");
        assert_eq!(set.shard_count(), 3);
        // The reader the app path gets is the **core**'s (§5.1), never a geometry shard's.
        assert!(!map.reader().is_set_shard());
    }

    // (There is no test for the removed lower-case filename fallback: §5.2's names are exact, but
    // the two dominant dev filesystems — APFS and NTFS — are case-insensitive, so a test that
    // staged `ms7s01.obm` would pass on Linux and fail on a Mac for reasons unrelated to the code.
    // The property is structural instead: `read_shard` joins the derived name and nothing else.)

    /// §5.4: a shard that is not there yet is *map incomplete*, and the message names the file.
    #[test]
    fn a_missing_shard_refuses_to_load_and_names_the_file() {
        let dir = Dir::new("missing");
        let fixture = fixture();
        stage(&dir, 7, &fixture);
        std::fs::remove_file(dir.0.join("MS7S01.OBM")).expect("drop a shard");
        let Err(err) = MapSource::load_set(&dir.path("MS7.OBS")) else { panic!("an incomplete set is refused") };
        assert!(matches!(err, LoadError::Read(..)), "{err}");
        assert!(err.to_string().contains("MS7S01.OBM"), "{err}");
    }

    /// A shard still being written has the right name and the wrong length — §5.3's `Bytes` check,
    /// and the one failure a naive loader would silently render as a smaller map.
    #[test]
    fn a_short_shard_refuses_to_mount() {
        let dir = Dir::new("short");
        let fixture = fixture();
        stage(&dir, 7, &fixture);
        let truncated = &fixture.shards[1][..fixture.shards[1].len() / 2];
        dir.write("MS7S01.OBM", truncated);
        let source = MapSource::load_set(&dir.path("MS7.OBS")).expect("the files are all present");
        let err = mounts(source).expect_err("a short shard does not mount");
        assert!(matches!(err, LoadError::Mount(MountError::Size(1), _)), "{err}");
        assert!(err.to_string().contains("MS7S01.OBM"), "{err}");
    }

    #[test]
    fn a_corrupt_manifest_refuses_to_load() {
        let dir = Dir::new("badmanifest");
        let fixture = fixture();
        stage(&dir, 7, &fixture);
        let mut manifest = fixture.manifest.clone();
        manifest[0] = b'X'; // magic
        dir.write("MS7.OBS", &manifest);
        let Err(err) = MapSource::load_set(&dir.path("MS7.OBS")) else { panic!("a bad manifest is refused") };
        assert!(matches!(err, LoadError::Manifest(_)), "{err}");
    }

    /// `--set` names the manifest, never a shard: `MS7S01.OBM` is not a map on its own (§5.4).
    #[test]
    fn a_shard_path_is_not_a_manifest_path() {
        let dir = Dir::new("shardpath");
        let fixture = fixture();
        stage(&dir, 7, &fixture);
        let Err(err) = MapSource::load_set(&dir.path("MS7S01.OBM")) else { panic!("a shard is not a manifest") };
        assert!(matches!(err, LoadError::NotAManifestName(_)), "{err}");
    }

    // ---------------------------------------------------------------------------------------
    // The differential: a set must draw the frame the monolith it was split from draws.
    // ---------------------------------------------------------------------------------------

    /// The fine rung's ceiling. At ≈0.111 m per µdeg of latitude `mpp = 0.111 / zoom`, so the zooms
    /// below straddle it: 0.2 selects the split (fine) rung, 0.02 falls back to the whole-assembly
    /// coarse shard.
    const FINE_MPP: f32 = 1.5;

    /// Four distinguishable colours — a mis-dispatch that served the wrong shard's chunk lands the
    /// wrong colour in the frame, not merely a differently-shaped one.
    const DIFF_STYLES: &[Style] = &[
        (1, 0, 0x07E0, 1, 1, false, None),
        (2, 0, 0xF800, 1, 1, false, None),
        (3, 0, 0x001F, 1, 1, false, None),
        (4, 0, 0xFFE0, 1, 1, false, None),
        (5, 0, 0x07FF, 3, 1, false, None),
    ];

    /// A monolith and the byte-level split of the same data into a core + coarse + four-quadrant
    /// set. Each quadrant carries a filled triangle plus a line that overhangs its own edge — the
    /// case where the owning shard and the viewport disagree.
    fn pair() -> (Vec<u8>, obcm_testkit::set::SetFixture) {
        let mut fine = Vec::new();
        for (style, over) in [(1u8, (900i16, 900i16)), (2, (-900, 900)), (3, (900, -900)), (4, (-900, -900))] {
            let mut chunk = obcm_testkit::pack_poly16(style, 400, 400, &[(1200, 0), (0, 1200), (-1200, 0)]);
            chunk.extend_from_slice(&obcm_testkit::pack_line16(style, 1000, 1000, &[(over.0, 0), (0, over.1)]));
            fine.push(seal(chunk, 4096));
        }
        let mut coarse = obcm_testkit::pack_line16(5, 200, 200, &[(3600, 0), (0, 3600), (-3600, 0), (0, -3600)]);
        coarse.extend_from_slice(&obcm_testkit::pack_line16(5, 200, 3800, &[(3600, -3600)]));
        obcm_testkit::set::matched_pair(
            ASSEMBLY,
            DIFF_STYLES,
            (f32::INFINITY, seal(coarse, 4096), 4096),
            (FINE_MPP, [fine[0].clone(), fine[1].clone(), fine[2].clone(), fine[3].clone()], 4096),
        )
    }

    #[derive(Default)]
    struct VecSink(Vec<u8>);

    impl ByteSink for VecSink {
        fn write(&mut self, bytes: &[u8]) -> Result<(), obc_formats::io::Error> {
            self.0.extend_from_slice(bytes);
            Ok(())
        }

        fn patch_at(&mut self, offset: u32, bytes: &[u8]) -> Result<(), obc_formats::io::Error> {
            let start = offset as usize;
            self.0[start..start + bytes.len()].copy_from_slice(bytes);
            Ok(())
        }
    }

    /// A route that crosses the four-shard junction at the fixture's centre.
    fn seam_route() -> Vec<u8> {
        let gpx = br#"<gpx><trk><trkseg>
            <trkpt lat="0.0010" lon="0.0005"><ele>100</ele></trkpt>
            <trkpt lat="0.0015" lon="0.0010"><ele>110</ele></trkpt>
            <trkpt lat="0.0020" lon="0.0020"><ele>120</ele></trkpt>
            <trkpt lat="0.0025" lon="0.0030"><ele>130</ele></trkpt>
            <trkpt lat="0.0030" lon="0.0035"><ele>140</ele></trkpt>
        </trkseg></trk></gpx>"#;
        let mut sink = VecSink::default();
        obc_route::gpx_to_obcr(&SliceSource(gpx), "Seam", &mut sink).expect("route fixture converts");
        sink.0
    }

    /// One whole app frame at `cam`, through exactly the path the simulator renders with: the
    /// mounted set passed through the app's generic scene seam, or plain `App::render_frame` for a
    /// single file. `press` taps Select first, so the caller can compare a frame whose base screen
    /// is *not* the map.
    fn app_frame_after(map: &LoadedMap, cam: (i32, i32, f32), press: bool, route_bytes: Option<&[u8]>) -> Vec<u8> {
        let (w, h) = (obc_display::ls021::FRAME_W as u32, obc_display::ls021::FRAME_H as u32);
        let reader = map.reader();
        let mut app = obc_app::App::new(obc_app::AppState::new(cam.0, cam.1, cam.2));
        if press {
            use obc_ports::{Button, ButtonEvent, InputEvent};
            crate::feed(&mut app, 500_000, vec![InputEvent::Button(ButtonEvent::Down(Button::Select))]);
            crate::feed(&mut app, 500_080, vec![InputEvent::Button(ButtonEvent::Up(Button::Select))]);
        }
        let route_source = route_bytes.map(SliceSource);
        let route_index = route_source.as_ref().map(|source| RouteIndex::read(source).expect("valid route fixture"));
        let route =
            route_source.as_ref().zip(route_index.as_ref()).map(|(source, index)| RouteReader::new(index, source));
        let mut fb = crate::framebuffer::Framebuffer::new(w, h);
        let set = map.set();
        let scene = Scene { set, reader: &reader, route: route.as_ref() };
        render_frame(&mut app, &mut fb, scene, (w as f32, h as f32), |c| crate::color_of(c, true));
        fb.as_rgb888().to_vec()
    }

    /// The headline. The camera sits on the assembly centre — the corner where all four geometry
    /// shards meet — so a shard skipped, a shard served another's chunks, or a plane spliced in at
    /// the wrong moment all show up as a different frame. Run at a zoom that selects the split rung
    /// and at one that falls back to the single whole-assembly coarse shard.
    #[test]
    fn a_mounted_set_draws_the_frame_its_monolith_draws() {
        let dir = Dir::new("diff");
        let (monolith, fixture) = pair();
        dir.write("MAP.OBCM", &monolith);
        stage(&dir, 3, &fixture);
        let single = LoadedMap::open(MapSource::load_single(&dir.path("MAP.OBCM")).expect("the monolith loads"))
            .expect("the monolith opens");
        let set =
            LoadedMap::open(MapSource::load_set(&dir.path("MS3.OBS")).expect("the set loads")).expect("the set mounts");
        assert_eq!(set.set().expect("a set").shard_count(), 6, "core + coarse + four quadrants");

        for (zoom, what) in [(0.2f32, "the split fine rung"), (0.02, "the whole-assembly coarse shard")] {
            let cam = (2000, 2000, zoom);
            let mono = app_frame_after(&single, cam, false, None);
            let split = app_frame_after(&set, cam, false, None);
            // A frame of pure chrome would pass trivially; both sides must actually draw map ink.
            let ink = |frame: &[u8]| frame.chunks_exact(3).filter(|px| px != &[0xFF, 0xFF, 0xFF]).count();
            assert!(ink(&mono) > 500, "{what}: the monolith's frame must carry map ink to be meaningful");
            assert_eq!(mono, split, "{what}: the set's frame differs from the monolith's, pixel for pixel");
        }
    }

    /// The on-glass acceptance shape in pixels: map geometry dispatches across the four-shard
    /// junction while the ordinary core route remains overlaid through the same app frame.
    #[test]
    fn a_route_crossing_the_shard_junction_matches_the_monolith() {
        let dir = Dir::new("route-diff");
        let (monolith, fixture) = pair();
        dir.write("MAP.OBCM", &monolith);
        stage(&dir, 13, &fixture);
        let single = LoadedMap::open(MapSource::load_single(&dir.path("MAP.OBCM")).expect("the monolith loads"))
            .expect("the monolith opens");
        let set = LoadedMap::open(MapSource::load_set(&dir.path("MS13.OBS")).expect("the set loads"))
            .expect("the set mounts");
        let route = seam_route();
        let cam = (2000, 2000, 0.2);
        let without_route = app_frame_after(&single, cam, false, None);
        let mono = app_frame_after(&single, cam, false, Some(&route));
        let split = app_frame_after(&set, cam, false, Some(&route));
        assert_ne!(mono, without_route, "the route must contribute pixels for the differential to be meaningful");
        assert_eq!(mono, split, "mounted-set geometry + core route must match the monolith frame pixel for pixel");
    }

    /// The other half of the splice: a frame whose base screen is **not** the map (Select opens the
    /// Ride menu) draws no map plane at all, so a set must not paint one under it. The monolith is
    /// the oracle — it is what the sim drew before `--set` existed.
    #[test]
    fn a_set_paints_no_map_under_a_screen_that_draws_none() {
        let dir = Dir::new("overlay");
        let (monolith, fixture) = pair();
        dir.write("MAP.OBCM", &monolith);
        stage(&dir, 4, &fixture);
        let single = LoadedMap::open(MapSource::load_single(&dir.path("MAP.OBCM")).expect("the monolith loads"))
            .expect("the monolith opens");
        let set =
            LoadedMap::open(MapSource::load_set(&dir.path("MS4.OBS")).expect("the set loads")).expect("the set mounts");
        let cam = (2000, 2000, 0.2);
        assert_eq!(
            app_frame_after(&single, cam, true, None),
            app_frame_after(&set, cam, true, None),
            "a non-map base screen must render identically from a set and from its monolith"
        );
    }

    /// The §5.5 fast path: a set of one is the core carrying everything, and it mounts the same way.
    #[test]
    fn the_single_file_fast_path_loads() {
        let dir = Dir::new("single");
        let fixture = build_set(
            ASSEMBLY,
            STYLES,
            0,
            &[ShardSpec {
                role: Role::Core,
                bbox: ASSEMBLY,
                lods: vec![LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![chunk()], chunk_size: 4096 }],
            }],
        );
        stage(&dir, 12, &fixture);
        let source = MapSource::load_set(&dir.path("MS12.OBS")).expect("a set of one loads");
        assert!(source.manifest().expect("a set").is_single_file());
        mounts(source).expect("a set of one mounts");
    }
}
