//! Peak View over the selected map's terrain and geographic summit records.
use crate::arena::PeakGuard;
use core::{cell::Cell, mem::MaybeUninit, ptr::addr_of_mut};
use embassy_time::Instant;
use obc_app::{
    peak_view::{self, Panorama},
    screen::Screen,
    App, PeakViewProfile,
};
use obc_formats::io::{ByteSource, Error, WindowSource};
use obc_reader::{MapTables, Reader};

struct Source {
    window: WindowSource<'static>,
    reads: Cell<u32>,
    bytes: Cell<u32>,
    read_us: Cell<u64>,
}
static mut SOURCE: MaybeUninit<Source> = MaybeUninit::uninit();
impl ByteSource for Source {
    fn len(&self) -> u64 {
        self.window.len()
    }
    fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        let started = Instant::now();
        let result = self.window.read_at(offset, out);
        self.reads.set(self.reads.get() + 1);
        self.bytes.set(self.bytes.get() + out.len() as u32);
        self.read_us.set(self.read_us.get() + started.elapsed().as_micros());
        result
    }
}

#[inline(never)]
fn open(map: &'static dyn ByteSource, tables: &MapTables) -> Option<&'static Source> {
    let region = tables.terrain()?;
    let window = WindowSource::new(map, region.offset, region.len)?;
    obc_elevation::surface::SurfaceReader::parse(&window).ok()?;
    let source = Source { window, reads: Cell::new(0), bytes: Cell::new(0), read_us: Cell::new(0) };
    // SAFETY: initialized once by the ride-loop constructor, then immutable except Cell counters.
    Some(unsafe { (*addr_of_mut!(SOURCE)).write(source) })
}

pub(crate) struct Runtime {
    source: Option<&'static Source>,
    arm: Option<PeakGuard>,
    started: Instant,
    failed: bool,
    first_presented: bool,
    view_ready: bool,
    view_progress: u8,
    active: bool,
    work_us: u64,
    reported: u8,
    position: Option<(i32, i32)>,
}
impl Runtime {
    #[inline(never)]
    pub fn new(app: &mut App, map: &'static dyn ByteSource, tables: &MapTables) -> Self {
        let source = open(map, tables);
        app.state.peak_view_profile = source.map(|_| PeakViewProfile::at(0, 0, 0));
        #[cfg(feature = "peak-view-demo")]
        {
            // Explicit test observer. Terrain and names still come from the selected map.
            app.state.user_fix =
                Some(obc_ports::Fix { lat: 46_585_000, lon: 7_961_000, course: None, speed_mps: Some(0.0) });
            app.state.compass_deg = Some(141.25);
            app.show_peak_view();
        }
        Self {
            source,
            arm: None,
            started: Instant::now(),
            failed: false,
            first_presented: false,
            view_ready: false,
            view_progress: 0,
            active: false,
            work_us: 0,
            reported: 0,
            position: None,
        }
    }

    /// Release before another arena claimant runs after a screen transition.
    pub fn reconcile(&mut self, app: &App) {
        self.active = matches!(app.top_screen(), Screen::PeakView(_));
        if !app.peak_view_is_base() {
            self.arm = None;
            self.first_presented = false;
            self.view_ready = false;
            self.view_progress = 0;
            self.failed = false;
            self.position = None;
        }
    }
    pub fn panorama(&self) -> Option<&Panorama> {
        self.arm.as_ref().filter(|_| !self.failed).map(|arm| &arm.builder.panorama)
    }
    pub fn busy(&self) -> bool {
        self.active && self.arm.as_ref().is_some_and(|arm| !arm.builder.complete()) && !self.failed
    }

    pub fn note_frame_presented(&mut self, app: &App) {
        if !self.first_presented
            && !self.failed
            && matches!(app.top_screen(), Screen::PeakView(_))
            && self.arm.as_ref().is_some_and(|arm| arm.builder.view_ready(app.peak_view_heading_q4()))
        {
            self.first_presented = true;
            defmt::info!("peak-view: first view in {=u64} ms", self.started.elapsed().as_millis());
        }
    }

    /// Sensors and Browse input can change the displayed heading after the work slice.
    pub fn refresh_view(&mut self, app: &mut App) -> bool {
        if !matches!(app.top_screen(), Screen::PeakView(_)) || self.arm.is_none() {
            return false;
        }
        let heading = app.peak_view_heading_q4();
        let arm = self.arm.as_deref_mut().unwrap();
        arm.builder.set_heading(heading);
        let ready = arm.builder.view_ready(heading) && !self.failed;
        let progress = arm.builder.panorama.view_progress(heading, arm.builder.profile().horizontal_fov_q4());
        let changed = self.view_ready != ready || (self.first_presented && self.view_progress != progress);
        self.view_ready = ready;
        self.view_progress = progress;
        app.set_peak_view_loading(!ready && !self.first_presented && !self.failed, self.failed);
        app.set_peak_view_building(self.busy());
        if changed {
            app.redraw_peak_view();
        }
        changed
    }

    #[inline(never)]
    fn start(&mut self, app: &mut App, reader: &Reader<'_>, position: (i32, i32)) -> bool {
        self.started = Instant::now();
        let Some(source) = self.source else { return false };
        let mut peaks = heapless::Vec::new();
        if peak_view::collect_summits(reader, position, &mut peaks).is_err() {
            return false;
        }
        let mut profile = PeakViewProfile::at(position.0, position.1, 0);
        profile.default_heading_q4 = app.peak_view_heading_q4();
        profile.peaks = &peaks;
        let Some(arm) = crate::arena::claim_peak(&mut profile, source) else { return false };
        app.state.peak_view_profile = Some(profile.detached());
        app.state.peak_view_peak_count = 0;
        source.reads.set(0);
        source.bytes.set(0);
        source.read_us.set(0);
        self.arm = Some(arm);
        self.work_us = 0;
        self.reported = 0;
        true
    }

    #[inline(never)]
    pub fn update(&mut self, app: &mut App, reader: &Reader<'_>) {
        self.reconcile(app);
        if !matches!(app.top_screen(), Screen::PeakView(_)) {
            return;
        }
        let Some(position) = app.state.user_fix.map(|fix| (fix.lat, fix.lon)).or(self.position) else {
            if self.arm.is_none() {
                app.set_peak_view_waiting();
            }
            return;
        };
        if self.position.is_none_or(|old| peak_view::moved(old, position)) && !self.busy() {
            self.arm = None;
            self.first_presented = false;
            self.view_ready = false;
            self.view_progress = 0;
            self.failed = false;
            self.position = Some(position);
            app.state.peak_view_peak_count = 0;
        }
        if self.arm.is_none() && !self.failed {
            self.failed = !self.start(app, reader, self.position.unwrap());
        }
        if self.busy() {
            let step_started = Instant::now();
            let heading = app.peak_view_heading_q4();
            let arm = self.arm.as_deref_mut().unwrap();
            arm.builder.set_heading(heading);
            let was_ready = arm.builder.view_ready(heading);
            while !arm.builder.complete() && step_started.elapsed().as_millis() < 50 {
                arm.builder.step(&mut arm.terrain, 16);
                if !was_ready && arm.builder.view_ready(heading) {
                    break;
                }
            }
            self.work_us += step_started.elapsed().as_micros();
            let progress = arm.builder.progress() / 5;
            if progress > self.reported {
                self.reported = progress;
                let source = self.source.unwrap();
                defmt::info!(
                    "peak-perf: {=u8}% wall {=u64} ms; work {=u64} ms; IO {=u64} ms / {=u32} reads / {=u32} bytes",
                    progress * 5,
                    self.started.elapsed().as_millis(),
                    self.work_us / 1000,
                    source.read_us.get() / 1000,
                    source.reads.get(),
                    source.bytes.get()
                );
            }
            self.failed = arm.terrain.failed();
            for (out, peak) in app.state.peak_view_peaks.iter_mut().zip(arm.builder.display_peaks()) {
                *out = peak;
            }
            app.state.peak_view_peak_count = arm.builder.peaks.len() as u8;
            if arm.builder.complete() {
                defmt::info!(
                    "peak-view: generated in {=u64} ms; {=u32} cells / {=u32} nodes, {=u32} missing; arena {=usize} B",
                    self.started.elapsed().as_millis(),
                    arm.builder.samples,
                    arm.builder.nodes,
                    arm.builder.missing,
                    core::mem::size_of::<crate::arena::PeakArm>()
                );
            }
        }
        if self.failed {
            app.set_peak_view_loading(false, true);
            app.set_peak_view_building(false);
        } else {
            self.refresh_view(app);
        }
    }
}
