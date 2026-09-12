//! Simulator lifecycle for runtime panoramas. Terrain I/O runs off the UI thread.

use obc_app::PeakViewProfile;

#[path = "../../../fixtures/sources/peak-view/catalog.rs"]
mod data;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Preset {
    Gornergrat,
    KleineScheidegg,
    Grossglockner,
}

impl Preset {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "gornergrat" => Ok(Self::Gornergrat),
            "scheidegg" | "kleine-scheidegg" => Ok(Self::KleineScheidegg),
            "glockner" | "grossglockner" => Ok(Self::Grossglockner),
            _ => Err("--peak-view needs gornergrat|scheidegg|glockner".into()),
        }
    }

    pub(crate) fn profile(self) -> &'static PeakViewProfile<'static> {
        match self {
            Self::Gornergrat => &data::GORNERGRAT,
            Self::KleineScheidegg => &data::SCHEIDEGG,
            Self::Grossglockner => &data::GLOCKNER,
        }
    }
}

use obc_app::{
    peak_view::{surface::Builder, terrain::Terrain, Panorama},
    screen::Screen,
    App,
};
use obc_formats::io::{ByteSource, Error};
use std::{
    cell::{Cell, RefCell},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU16, Ordering},
        mpsc, Arc,
    },
    time::Instant,
};

struct FileSource {
    file: RefCell<File>,
    len: u64,
    reads: Cell<u32>,
    bytes: Cell<u64>,
}
impl ByteSource for FileSource {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let mut file = self.file.borrow_mut();
        file.seek(SeekFrom::Start(offset)).map_err(|_| Error::Io)?;
        file.read_exact(buf).map_err(|_| Error::Io)?;
        self.reads.set(self.reads.get() + 1);
        self.bytes.set(self.bytes.get() + buf.len() as u64);
        Ok(())
    }
}

fn terrain_root() -> PathBuf {
    std::env::var_os("OBC_PEAK_TERRAIN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| obc_fixtures::root().join("sim-peak-view"))
}

#[derive(Clone, Copy)]
enum Input {
    Map { bytes: &'static [u8], offset: u64, len: u64 },
    Fixture(Preset),
}

impl Input {
    fn selected(map: &crate::map_file::LoadedMap, preset: Option<Preset>) -> Option<Self> {
        if let Some(preset) = preset {
            return Some(Self::Fixture(preset));
        }
        let region = map.tables().terrain()?;
        let source = obc_formats::io::SliceSource(map.bytes());
        let window = obc_formats::io::WindowSource::new(&source, region.offset, region.len)?;
        let mut header = [0; obc_formats::obct::HEADER_LEN];
        window.read_at(0, &mut header).ok()?;
        obc_formats::obct::validate_header_prefix(&header).ok()?;
        if header[4] != obc_formats::obct::SURFACE_VERSION || header[7] & obc_formats::obct::SURFACE_FLAG == 0 {
            return None;
        }
        Some(Self::Map { bytes: map.bytes(), offset: region.offset, len: region.len })
    }

    fn profile(self) -> PeakViewProfile<'static> {
        match self {
            Self::Fixture(preset) => preset.profile().detached(),
            Self::Map { .. } => PeakViewProfile::at(0, 0, 0),
        }
    }
}

fn generate(input: Input, position: (i32, i32), worker: &Worker) -> Result<Box<Builder>, String> {
    match input {
        Input::Map { bytes, offset, len } => {
            let source = obc_formats::io::SliceSource(bytes);
            let tables = obc_reader::MapTables::parse(&source).map_err(|e| format!("map: {e:?}"))?;
            let cache = Box::new(obc_reader::MapCache::new());
            let reader = obc_reader::Reader::new(&source, &tables, &cache);
            let window = obc_formats::io::WindowSource::new(&source, offset, len).ok_or("terrain outside map")?;
            generate_surface(&window, Some(&reader), input.profile(), position, worker)
        }
        Input::Fixture(preset) => {
            let key = match preset {
                Preset::Gornergrat => "gornergrat",
                Preset::KleineScheidegg => "scheidegg",
                Preset::Grossglockner => "glockner",
            };
            let path = terrain_root().join(format!("{key}.obcd"));
            let file = File::open(&path)
                .map_err(|e| format!("{}: {e}. Run obc fixtures sync sim-peak-view first.", path.display()))?;
            let len = file.metadata().map_err(|e| e.to_string())?.len();
            let source = FileSource { file: RefCell::new(file), len, reads: Cell::new(0), bytes: Cell::new(0) };
            let result = generate_surface(&source, None, *preset.profile(), position, worker);
            eprintln!("peak-view: {} terrain reads / {} bytes", source.reads.get(), source.bytes.get());
            result
        }
    }
}

fn generate_surface(
    source: &dyn ByteSource,
    reader: Option<&obc_reader::Reader<'_>>,
    mut profile: PeakViewProfile<'_>,
    position: (i32, i32),
    worker: &Worker,
) -> Result<Box<Builder>, String> {
    let started = Instant::now();
    let mut terrain = Terrain::parse(source).map_err(|e| format!("terrain: {e:?}"))?;
    let ground = terrain.ground_height(position.0, position.1).ok_or("no terrain at observer")?;
    let mut candidates = Default::default();
    if let Some(reader) = reader {
        obc_app::peak_view::collect_summits(reader, position, &mut candidates)
            .map_err(|e| format!("summits: {e:?}"))?;
    } else {
        candidates.extend_from_slice(profile.peaks).map_err(|_| "too many fixture summits")?;
        for peak in &mut candidates {
            peak.project(position.0, position.1);
        }
    }
    profile.observer_lat = position.0;
    profile.observer_lon = position.1;
    let mut profile = PeakViewProfile { peaks: &candidates, ..profile };
    profile.set_ground(ground);
    profile.default_heading_q4 = worker.heading.load(Ordering::Relaxed);
    let mut builder = Box::new(Builder::new(&profile));
    let mut published = 0;
    let mut first_ready = false;
    while !builder.complete() {
        if worker.cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let heading = worker.heading.load(Ordering::Relaxed) % 1440;
        builder.set_heading(heading);
        builder.step(&mut terrain, 64);
        if !terrain.failed() {
            if !first_ready && builder.view_ready(heading) {
                first_ready = true;
                eprintln!("peak-view: view ready in {:.0} ms", started.elapsed().as_secs_f64() * 1000.0);
            }
            let progress = builder.progress();
            if first_ready && !builder.complete() && progress != published {
                let preview = Preview::from_builder(&builder);
                published = progress;
                if worker.sender.send(Ok(Frame { preview, complete: false })).is_err() {
                    return Err("cancelled".into());
                }
            }
        }
    }
    eprintln!(
        "peak-view: generated in {:.0} ms; {} samples, {} missing; core+cache {} bytes",
        started.elapsed().as_secs_f64() * 1000.0,
        builder.samples,
        builder.missing,
        std::mem::size_of::<Builder>() + std::mem::size_of::<Terrain<'_>>()
    );
    if terrain.failed() {
        return Err("terrain read failed".into());
    }
    Ok(builder)
}

struct Preview {
    panorama: Panorama,
    profile: PeakViewProfile<'static>,
    peaks: Vec<obc_app::PeakViewPeak>,
}
impl Preview {
    fn from_builder(builder: &Builder) -> Box<Self> {
        Box::new(Self {
            panorama: builder.panorama.clone(),
            profile: builder.profile(),
            peaks: builder.display_peaks().collect(),
        })
    }
    fn view_ready(&self, heading: u16) -> bool {
        self.panorama.view_ready(heading, self.profile.horizontal_fov_q4())
    }
}
struct Frame {
    preview: Box<Preview>,
    complete: bool,
}
struct Worker {
    heading: Arc<AtomicU16>,
    cancel: Arc<AtomicBool>,
    sender: mpsc::Sender<Result<Frame, String>>,
}

/// One cancellable worker publishes immutable ready views while completing the panorama.
pub(crate) struct Runtime {
    input: Option<Input>,
    receiver: Option<mpsc::Receiver<Result<Frame, String>>>,
    cancel: Arc<AtomicBool>,
    heading: Arc<AtomicU16>,
    result: Option<Box<Preview>>,
    failed: bool,
    position: Option<(i32, i32)>,
    started: Instant,
    first_presented: bool,
}
impl Runtime {
    pub fn new(map: &crate::map_file::LoadedMap, preset: Option<Preset>) -> Self {
        Self {
            input: Input::selected(map, preset),
            receiver: None,
            cancel: Arc::new(AtomicBool::new(false)),
            heading: Arc::new(AtomicU16::new(0)),
            result: None,
            failed: false,
            position: None,
            started: Instant::now(),
            first_presented: false,
        }
    }

    pub fn profile(&self) -> Option<PeakViewProfile<'static>> {
        self.input.map(Input::profile)
    }

    pub fn panorama(&self) -> Option<&Panorama> {
        self.result.as_ref().filter(|_| !self.failed).map(|b| &b.panorama)
    }

    pub fn note_frame_presented(&mut self, app: &App) {
        if !self.first_presented
            && !self.failed
            && matches!(app.top_screen(), Screen::PeakView(_))
            && self.result.as_ref().is_some_and(|result| result.view_ready(app.peak_view_heading_q4()))
        {
            self.first_presented = true;
            eprintln!("peak-view: first view in {:.0} ms", self.started.elapsed().as_secs_f64() * 1000.0);
        }
    }

    pub fn update(&mut self, app: &mut App) {
        let active = matches!(app.top_screen(), Screen::PeakView(_));
        if !active {
            if app.peak_view_is_base() {
                return;
            }
            self.cancel.store(true, Ordering::Relaxed);
            self.receiver = None;
            self.result = None;
            self.failed = false;
            self.position = None;
            self.first_presented = false;
            return;
        }
        let Some(position) = app.state.user_fix.map(|fix| (fix.lat, fix.lon)).or(self.position) else {
            app.set_peak_view_waiting();
            return;
        };
        if self.position.is_none_or(|old| obc_app::peak_view::moved(old, position)) && self.receiver.is_none() {
            self.result = None;
            self.failed = false;
            self.position = Some(position);
            self.first_presented = false;
            app.state.peak_view_peak_count = 0;
        }
        let heading = app.peak_view_heading_q4();
        self.heading.store(heading, Ordering::Relaxed);
        if self.result.is_none() && self.receiver.is_none() && !self.failed {
            self.started = Instant::now();
            let (sender, receiver) = mpsc::channel();
            self.cancel = Arc::new(AtomicBool::new(false));
            let worker = Worker { heading: Arc::clone(&self.heading), cancel: Arc::clone(&self.cancel), sender };
            let Some(input) = self.input else {
                app.set_peak_view_loading(false, true);
                return;
            };
            let position = self.position.unwrap();
            let mut profile = input.profile();
            profile.observer_lat = position.0;
            profile.observer_lon = position.1;
            profile.default_heading_q4 = heading;
            app.state.peak_view_profile = Some(profile);
            std::thread::spawn(move || {
                let result = generate(input, position, &worker)
                    .map(|builder| Frame { preview: Preview::from_builder(&builder), complete: true });
                let _ = worker.sender.send(result);
            });
            self.receiver = Some(receiver);
        }
        while let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(result) => self.accept(app, result),
                Err(mpsc::TryRecvError::Disconnected) => self.accept(app, Err("terrain worker stopped".into())),
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        let view_ready = self.result.as_ref().is_some_and(|result| result.view_ready(heading));
        app.set_peak_view_loading(!view_ready && !self.first_presented && !self.failed, self.failed);
        app.set_peak_view_building(self.receiver.is_some());
    }

    fn accept(&mut self, app: &mut App, result: Result<Frame, String>) {
        match result {
            Ok(frame) => {
                let heading = app.peak_view_heading_q4();
                let progress =
                    |preview: &Preview| preview.panorama.view_progress(heading, preview.profile.horizontal_fov_q4());
                let changed = self.result.as_ref().is_none_or(|old| progress(old) != progress(&frame.preview));
                if frame.complete {
                    self.receiver = None;
                }
                app.state.peak_view_profile = Some(frame.preview.profile);
                app.state.peak_view_peaks[..frame.preview.peaks.len()].copy_from_slice(&frame.preview.peaks);
                app.state.peak_view_peak_count = frame.preview.peaks.len() as u8;
                self.result = Some(frame.preview);
                if changed {
                    app.redraw_peak_view();
                }
            }
            Err(error) => {
                self.receiver = None;
                eprintln!("peak-view: {error}");
                self.failed = true;
            }
        }
    }

    /// Deterministic headless frames finish the full job, not just its first ready view.
    pub fn finish(&mut self, app: &mut App) {
        self.update(app);
        while let Some(receiver) = &self.receiver {
            let result = receiver.recv().unwrap_or_else(|_| Err("terrain worker stopped".into()));
            self.accept(app, result);
        }
        self.update(app);
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_working_set_fits_the_device_arena() {
        assert!(std::mem::size_of::<Builder>() + std::mem::size_of::<Terrain<'_>>() <= 128 * 1024);
        for preset in [Preset::Gornergrat, Preset::KleineScheidegg, Preset::Grossglockner] {
            assert!(preset.profile().peaks.len() <= 32);
        }
    }

    #[test]
    fn selected_map_terrain_waits_for_gps_cancels_and_reloads_only_after_movement() {
        use obc_formats::{obcm, obct};
        use obcm_testkit::{build_file, pack_line, seal, LodSpec};
        let cell = 19;
        let layout = obct::SurfaceLayout::new(15, cell).unwrap();
        let prefix = obct::CellIndexLayout::new(1, 1, 32).unwrap().end() as usize;
        let mut terrain = vec![0; prefix + layout.cell_bytes() as usize];
        terrain[..4].copy_from_slice(&obct::MAGIC);
        terrain[4..8].copy_from_slice(&[obct::SURFACE_VERSION, 15, cell, obct::SURFACE_FLAG | obct::CELL_INDEX_FLAG]);
        terrain[8..12].copy_from_slice(&512u32.to_le_bytes());
        terrain[12..16].copy_from_slice(&512u32.to_le_bytes());
        terrain[16..18].copy_from_slice(&1u16.to_le_bytes());
        terrain[18..20].copy_from_slice(&1u16.to_le_bytes());
        terrain[20..24].copy_from_slice(&32u32.to_le_bytes());
        terrain[32..36].copy_from_slice(&(prefix as u32).to_le_bytes());
        let chunk = seal(pack_line(1, 100, 100, &[(50, 50)]), 4096);
        let mut bytes = build_file(
            (0, 0, 1 << cell, 1 << cell),
            &[(1, 0, 0x07e0, 1, 1, false, None)],
            &[LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![chunk], chunk_size: 4096 }],
        );
        let offset = (bytes.len() + 511) & !511;
        bytes.resize(offset, 0);
        let shift = bytes[40];
        bytes[41..45].copy_from_slice(&((offset >> shift) as u32).to_le_bytes());
        bytes[45..49].copy_from_slice(&((terrain.len() >> shift) as u32).to_le_bytes());
        bytes.extend_from_slice(&terrain);
        assert!(bytes.len() >= obcm::HEADER_LEN);
        let dir = obcm_testkit::scratch::scratch_dir("obc-sim-peak", "map-runtime");
        let path = dir.join("region.obcm");
        std::fs::write(&path, bytes).unwrap();
        let map =
            crate::map_file::LoadedMap::open(crate::map_file::MapSource::load_single(path.to_str().unwrap()).unwrap())
                .unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        let mut runtime = Runtime::new(&map, None);
        let mut app = App::new(obc_app::AppState::new(200_000, 200_000, 1.0));
        app.state.peak_view_profile = runtime.profile();
        assert!(app.show_peak_view());
        runtime.update(&mut app);
        assert!(runtime.receiver.is_none(), "no fabricated observer before GPS");
        app.state.user_fix = Some(obc_ports::Fix { lat: 200_000, lon: 200_000, course: None, speed_mps: Some(0.0) });
        runtime.update(&mut app);
        let cancelled = Arc::clone(&runtime.cancel);
        assert!(runtime.receiver.is_some());
        app.apply_gesture(obc_app::Gesture::Back);
        runtime.update(&mut app);
        assert!(cancelled.load(Ordering::Relaxed));
        assert!(runtime.receiver.is_none() && runtime.result.is_none());
        assert!(app.show_peak_view());
        runtime.finish(&mut app);
        assert!(runtime.panorama().is_some(), "embedded terrain generated without any fixture source");
        assert!(runtime.panorama().unwrap().has_incomplete_coverage());
        let completed = Arc::clone(&runtime.cancel);
        let panorama = runtime.panorama().unwrap() as *const Panorama;
        assert!(app.apply_chord(obc_app::Chord::Quick));
        runtime.update(&mut app);
        assert_eq!(runtime.panorama().unwrap() as *const Panorama, panorama);
        app.apply_gesture(obc_app::Gesture::Press);
        runtime.update(&mut app);
        assert_eq!(runtime.panorama().unwrap() as *const Panorama, panorama, "a drawer page retains its base");
        assert!(app.apply_chord(obc_app::Chord::Quick));
        runtime.update(&mut app);
        assert!(Arc::ptr_eq(&completed, &runtime.cancel), "closing the drawer does not start another panorama");
        app.state.compass_deg = Some(210.0);
        runtime.update(&mut app);
        assert!(Arc::ptr_eq(&completed, &runtime.cancel));
        assert!(runtime.receiver.is_none(), "turning reuses the panorama");
        app.state.user_fix.as_mut().unwrap().lat += 1000;
        runtime.finish(&mut app);
        assert!(!Arc::ptr_eq(&completed, &runtime.cancel));
        assert!(runtime.panorama().is_some());
        assert_eq!(app.state.peak_view_profile.unwrap().observer_lat, 201_000);
        app.apply_gesture(obc_app::Gesture::Back);
        runtime.update(&mut app);
        assert!(runtime.panorama().is_none(), "leaving the base releases the panorama");
    }
}
