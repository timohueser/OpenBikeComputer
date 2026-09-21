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
    App,
};
use obc_file_source::FileSource;
use obc_formats::io::ByteSource;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU16, Ordering},
        mpsc, Arc,
    },
    time::Instant,
};

fn terrain_root() -> PathBuf {
    std::env::var_os("OBC_PEAK_TERRAIN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| obc_fixtures::root().join("sim-peak-view"))
}

#[derive(Clone)]
enum Input {
    Map { source: obc_host_core::flat_store::ObjectSource, offset: u64, len: u64 },
    Fixture(Preset),
}

impl Input {
    fn selected(map: &crate::map_file::LoadedMap, preset: Option<Preset>) -> Option<Self> {
        if let Some(preset) = preset {
            return Some(Self::Fixture(preset));
        }
        let region = map.tables().terrain()?;
        let source = map.map_source();
        let window = obc_formats::io::WindowSource::new(&source, region.offset, region.len)?;
        let mut header = [0; obc_formats::obct::HEADER_LEN];
        window.read_at(0, &mut header).ok()?;
        obc_formats::obct::validate_header_prefix(&header).ok()?;
        if header[4] != obc_formats::obct::SURFACE_VERSION || header[7] & obc_formats::obct::SURFACE_FLAG == 0 {
            return None;
        }
        Some(Self::Map { source, offset: region.offset, len: region.len })
    }

    fn profile(&self) -> PeakViewProfile<'static> {
        match self {
            Self::Fixture(preset) => preset.profile().detached(),
            Self::Map { .. } => PeakViewProfile::at(0, 0, 0),
        }
    }
}

fn generate(input: Input, position: (i32, i32), worker: &Worker) -> Result<Box<Builder>, String> {
    let profile = input.profile();
    match input {
        Input::Map { source, offset, len } => {
            let tables = obc_reader::MapTables::parse(&source).map_err(|e| format!("map: {e:?}"))?;
            let cache = Box::new(obc_reader::MapCache::new());
            let reader = obc_reader::Reader::new(&source, &tables, &cache);
            let window = obc_formats::io::WindowSource::new(&source, offset, len).ok_or("terrain outside map")?;
            generate_surface(&window, Some(&reader), profile, position, worker)
        }
        Input::Fixture(preset) => {
            let key = match preset {
                Preset::Gornergrat => "gornergrat",
                Preset::KleineScheidegg => "scheidegg",
                Preset::Grossglockner => "glockner",
            };
            let path = terrain_root().join(format!("{key}.obcd"));
            let source = FileSource::open(&path)
                .map_err(|e| format!("{}: {e}. Run obc fixtures sync sim-peak-view first.", path.display()))?;
            generate_surface(&source, None, *preset.profile(), position, worker)
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
        obc_app::peak_view::collect_summits(reader, position, (ground + 2.0).round() as i16, &[], &mut candidates)
            .map_err(|e| format!("summits: {e:?}"))?;
    } else {
        candidates.extend_from_slice(profile.peaks).map_err(|_| "too many fixture summits")?;
        for peak in &mut candidates {
            peak.project(position.0, position.1);
        }
    }
    for peak in &mut candidates {
        peak.score = obc_app::peak_view::apparent_size(peak, (ground + 2.0).round() as i16);
    }
    profile.observer_lat = position.0;
    profile.observer_lon = position.1;
    let mut profile = PeakViewProfile { peaks: &candidates, ..profile };
    if reader.is_some() {
        profile.set_ground(ground);
    } else {
        profile.observer_elevation_m = (ground + 2.0).round() as i16;
    }
    profile.default_heading_q4 = worker.heading.load(Ordering::Relaxed);
    let mut builder = Box::new(Builder::new(&profile));
    let mut published = Instant::now();
    let mut search = obc_app::peak_view::SummitSearch::default();
    let mut picture_reported = false;
    let mut first_ready = false;
    loop {
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
            if published.elapsed().as_millis() >= 500 {
                let preview = Preview::from_builder(&builder);
                published = Instant::now();
                if worker.sender.send(Ok(Frame { preview, complete: false })).is_err() {
                    return Err("cancelled".into());
                }
            }
        }
        if terrain.failed() {
            return Err("terrain read failed".into());
        }
        if builder.complete() {
            if !picture_reported {
                eprintln!("peak-view: generated in {:.0} ms", started.elapsed().as_secs_f64() * 1000.0);
                picture_reported = true;
            }
            if let Some(reader) = reader {
                search.refill(&mut builder, reader).map_err(|e| format!("summits: {e:?}"))?;
            }
            if builder.complete() {
                break;
            }
        }
    }

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

use obc_app::peak_view::runtime::{Failed, Lifecycle, Platform, Progress};

/// The UI owns previews; one cancellable worker owns terrain I/O and the builder.
pub(crate) struct Runtime {
    lifecycle: Lifecycle,
    job: Job,
    clock: Instant,
}
struct Job {
    input: Option<Input>,
    receiver: Option<mpsc::Receiver<Result<Frame, String>>>,
    cancel: Arc<AtomicBool>,
    heading: Arc<AtomicU16>,
    result: Option<Box<Preview>>,
    revision: u64,
    pending: Option<Result<Frame, String>>,
}
impl Platform for Job {
    fn start(&mut self, app: &mut App, position: (i32, i32)) -> bool {
        let Some(input) = self.input.clone() else { return false };
        let heading = app.peak_view_heading_q4();
        self.heading.store(heading, Ordering::Relaxed);
        let mut profile = input.profile();
        profile.observer_lat = position.0;
        profile.observer_lon = position.1;
        profile.default_heading_q4 = heading;
        app.state.peak_view_profile = Some(profile);
        self.result = Some(Box::new(Preview { panorama: Panorama::default(), profile, peaks: Vec::new() }));
        self.revision = 0;
        let (sender, receiver) = mpsc::channel();
        self.cancel = Arc::new(AtomicBool::new(false));
        let worker = Worker { heading: Arc::clone(&self.heading), cancel: Arc::clone(&self.cancel), sender };
        std::thread::spawn(move || {
            let result = generate(input, position, &worker)
                .map(|builder| Frame { preview: Preview::from_builder(&builder), complete: true });
            let _ = worker.sender.send(result);
        });
        self.receiver = Some(receiver);
        true
    }
    fn step(&mut self, app: &mut App) -> Result<Progress, Failed> {
        self.heading.store(app.peak_view_heading_q4(), Ordering::Relaxed);
        if let Some(result) = self.pending.take() {
            self.accept(app, result)?;
        }
        while let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(result) => self.accept(app, result)?,
                Err(mpsc::TryRecvError::Disconnected) => return Err(Failed),
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        Ok(Progress { complete: self.receiver.is_none(), revision: self.revision })
    }
    fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.receiver = None;
        self.result = None;
        self.pending = None;
    }
}
impl Job {
    fn accept(&mut self, app: &mut App, result: Result<Frame, String>) -> Result<(), Failed> {
        match result {
            Ok(frame) => {
                if frame.complete {
                    self.receiver = None;
                }
                app.state.peak_view_profile = Some(frame.preview.profile);
                app.state.peak_view_peaks[..frame.preview.peaks.len()].copy_from_slice(&frame.preview.peaks);
                app.state.peak_view_peak_count = frame.preview.peaks.len() as u8;
                self.result = Some(frame.preview);
                self.revision += 1;
                Ok(())
            }
            Err(error) => {
                self.receiver = None;
                eprintln!("peak-view: {error}");
                Err(Failed)
            }
        }
    }
}
impl Runtime {
    pub fn new(map: &crate::map_file::LoadedMap, preset: Option<Preset>) -> Self {
        Self {
            lifecycle: Lifecycle::default(),
            clock: Instant::now(),
            job: Job {
                input: Input::selected(map, preset),
                receiver: None,
                cancel: Arc::new(AtomicBool::new(false)),
                heading: Arc::new(AtomicU16::new(0)),
                result: None,
                revision: 0,
                pending: None,
            },
        }
    }
    pub fn profile(&self) -> Option<PeakViewProfile<'static>> {
        self.job.input.as_ref().map(Input::profile)
    }
    pub fn panorama(&self) -> Option<&Panorama> {
        self.job.result.as_ref().map(|result| &result.panorama)
    }
    pub fn note_frame_presented(&mut self, app: &App) {
        if let Some(ms) = self.lifecycle.note_presented(app, self.clock.elapsed().as_millis() as u64) {
            eprintln!("peak-view: first view in {ms} ms");
        }
    }
    pub fn update(&mut self, app: &mut App) {
        // Keep Browse priority current even after the picture is complete.
        self.job.heading.store(app.peak_view_heading_q4(), Ordering::Relaxed);
        self.lifecycle.update(app, &mut self.job, self.clock.elapsed().as_millis() as u64);
    }
    pub fn finish(&mut self, app: &mut App) {
        self.update(app);
        if !matches!(app.top_screen(), obc_app::screen::Screen::PeakView(_)) {
            return;
        }
        while let Some(receiver) = &self.job.receiver {
            let result = receiver.recv().unwrap_or_else(|_| Err("terrain worker stopped".into()));
            self.job.pending = Some(result);
            self.update(app);
        }
        self.update(app);
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.job.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_working_set_fits_the_device_arena() {
        assert!(std::mem::size_of::<Builder>() + std::mem::size_of::<Terrain<'_>>() <= 128 * 1024);
        for preset in [Preset::Gornergrat, Preset::KleineScheidegg, Preset::Grossglockner] {
            assert!(preset.profile().peaks.len() <= 64);
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
        assert!(runtime.job.receiver.is_none(), "no fabricated observer before GPS");
        let mut loc = crate::sim_location::SimLocationSource::new(Some(obc_ports::Fix::at(200_000, 200_000)));
        app.tick(obc_ports::RideClock(0), obc_ports::Sensors::new(&mut loc), None);
        runtime.update(&mut app);
        let cancelled = Arc::clone(&runtime.job.cancel);
        assert!(runtime.job.receiver.is_some());
        assert!(app.apply_chord(obc_app::Chord::Quick));
        runtime.finish(&mut app);
        assert!(runtime.job.receiver.is_some(), "a covered headless job stays retained without waiting");
        assert!(!cancelled.load(Ordering::Relaxed));
        assert!(app.apply_chord(obc_app::Chord::Quick));
        app.apply_gesture(obc_app::Gesture::Back);
        runtime.update(&mut app);
        assert!(cancelled.load(Ordering::Relaxed));
        assert!(runtime.job.receiver.is_none() && runtime.job.result.is_none());
        assert!(app.show_peak_view());
        runtime.finish(&mut app);
        assert!(runtime.panorama().is_some(), "embedded terrain generated without any fixture source");
        assert!(runtime.panorama().unwrap().has_incomplete_coverage());
        let completed = Arc::clone(&runtime.job.cancel);
        let panorama = runtime.panorama().unwrap() as *const Panorama;
        app.set_backlight_available(true);
        assert!(app.apply_chord(obc_app::Chord::Quick));
        runtime.update(&mut app);
        assert_eq!(runtime.panorama().unwrap() as *const Panorama, panorama);
        app.apply_gesture(obc_app::Gesture::Press);
        runtime.update(&mut app);
        assert_eq!(runtime.panorama().unwrap() as *const Panorama, panorama, "a drawer page retains its base");
        assert!(app.apply_chord(obc_app::Chord::Quick));
        runtime.update(&mut app);
        assert!(Arc::ptr_eq(&completed, &runtime.job.cancel), "closing the drawer does not start another panorama");
        app.state.compass_deg = Some(210.0);
        runtime.update(&mut app);
        assert!(Arc::ptr_eq(&completed, &runtime.job.cancel));
        assert!(runtime.job.receiver.is_none(), "turning reuses the panorama");
        app.state.user_fix.as_mut().unwrap().lat += 1000;
        runtime.finish(&mut app);
        assert!(!Arc::ptr_eq(&completed, &runtime.job.cancel));
        assert!(runtime.panorama().is_some());
        assert_eq!(app.state.peak_view_profile.unwrap().observer_lat, 201_000);
        app.apply_gesture(obc_app::Gesture::Back);
        runtime.update(&mut app);
        assert!(runtime.panorama().is_none(), "leaving the base releases the panorama");
    }
}
